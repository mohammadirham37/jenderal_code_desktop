//! Google Gemini adapter (v1beta streamGenerateContent with SSE).

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::channel::mpsc;
use futures::stream::BoxStream;
use futures::StreamExt;
use jcode_common::{
    ModelCapabilities, ModelInfo, ModelRequest, Role, StopReason, StreamEvent,
    TokenUsage,
};
use jcode_provider_core::{
    http_client, parse_sse_data_line, AiProvider, CredentialStore, SseLineBuffer,
    NON_STREAMING_TIMEOUT,
};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct GoogleProvider {
    id: String,
    name: String,
    base_url: String,
    creds: Arc<dyn CredentialStore>,
    credential_ref: Option<String>,
    http: reqwest::Client,
}

impl GoogleProvider {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        base_url: impl Into<String>,
        creds: Arc<dyn CredentialStore>,
        credential_ref: Option<String>,
    ) -> Self {
        let base_url: String = base_url.into();
        Self {
            id: id.into(),
            name: name.into(),
            base_url: base_url.trim().trim_end_matches('/').to_string(),
            creds,
            credential_ref,
            http: http_client(),
        }
    }

    async fn api_key(&self) -> Result<Option<String>> {
        match &self.credential_ref {
            Some(r) => self.creds.get(r).await,
            None => Ok(None),
        }
    }

    fn convert_contents(request: &ModelRequest) -> Result<Vec<Value>> {
        let mut out: Vec<Value> = Vec::new();
        for m in &request.messages {
            match m.role {
                Role::System => continue, // handled via systemInstruction
                Role::User => out.push(json!({
                    "role": "user",
                    "parts": [{"text": m.content}]
                })),
                Role::Assistant => {
                    if m.tool_calls.is_empty() {
                        out.push(json!({
                            "role": "model",
                            "parts": [{"text": m.content}]
                        }));
                    } else {
                        let mut parts = Vec::new();
                        if !m.content.is_empty() {
                            parts.push(json!({"text": m.content}));
                        }
                        for c in &m.tool_calls {
                            let args: Value = if c.arguments.trim().is_empty() {
                                json!({})
                            } else {
                                serde_json::from_str(&c.arguments).with_context(|| {
                                    format!("parsing tool arguments for {}", c.name)
                                })?
                            };
                            parts.push(json!({
                                "functionCall": {"name": c.name, "args": args}
                            }));
                        }
                        if parts.is_empty() {
                            parts.push(json!({"text": ""}));
                        }
                        out.push(json!({"role": "model", "parts": parts}));
                    }
                }
                Role::Tool => {
                    let response = if m.content.trim().is_empty() {
                        json!({"result": ""})
                    } else if let Ok(v) = serde_json::from_str::<Value>(&m.content) {
                        v
                    } else {
                        json!({"result": m.content})
                    };
                    let part = json!({
                        "functionResponse": {
                            "name": m.tool_name.clone().unwrap_or_default(),
                            "response": response,
                        }
                    });
                    // Merge consecutive function responses into one user turn.
                    if let Some(last) = out.last_mut() {
                        if last["role"] == "user" && last["parts"][0].get("functionResponse").is_some()
                        {
                            last["parts"].as_array_mut().unwrap().push(part);
                            continue;
                        }
                    }
                    out.push(json!({"role": "user", "parts": [part]}));
                }
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl AiProvider for GoogleProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let key = self
            .api_key()
            .await?
            .filter(|k| !k.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Google Gemini requires an API key"))?;
        let resp = self
            .http
            .get(format!("{}/v1beta/models?pageSize=1000", self.base_url))
            .header("x-goog-api-key", &key)
            .timeout(NON_STREAMING_TIMEOUT)
            .send()
            .await
            .context("connecting to Google")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Google list models failed ({status}): {}",
                body.chars().take(300).collect::<String>()
            ));
        }
        let v: Value = serde_json::from_str(&body).context("parsing /v1beta/models response")?;
        let mut models = Vec::new();
        if let Some(list) = v["models"].as_array() {
            for m in list {
                let Some(full_name) = m["name"].as_str() else { continue };
                let methods = m["supportedGenerationMethods"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .any(|s| s == "generateContent")
                    })
                    .unwrap_or(true);
                if !methods {
                    continue;
                }
                let name = full_name.strip_prefix("models/").unwrap_or(full_name);
                models.push(ModelInfo {
                    id: format!("{}/{}", self.id, name),
                    provider_id: self.id.clone(),
                    name: name.to_string(),
                    display_name: m["displayName"].as_str().unwrap_or(name).to_string(),
                    capabilities: ModelCapabilities {
                        streaming: Some(true),
                        tool_calling: Some(true),
                        ..Default::default()
                    },
                    input_price_per_mtok: None,
                    output_price_per_mtok: None,
                    custom: false,
                });
            }
        }
        Ok(models)
    }

    async fn stream(&self, request: ModelRequest) -> Result<BoxStream<'static, StreamEvent>> {
        let key = self
            .api_key()
            .await?
            .filter(|k| !k.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Google Gemini requires an API key"))?;

        let model = request.model.trim();
        let url = format!(
            "{}/v1beta/models/{model}:streamGenerateContent?alt=sse",
            self.base_url
        );

        let mut body = json!({
            "contents": Self::convert_contents(&request)?,
        });
        if let Some(sys) = &request.system {
            body["systemInstruction"] = json!({"parts": [{"text": sys}]});
        }
        if !request.tools.is_empty() {
            body["tools"] = json!([{
                "functionDeclarations": request.tools.iter().map(|t| json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })).collect::<Vec<_>>()
            }]);
        }
        let mut generation = json!({});
        if let Some(t) = request.temperature {
            generation["temperature"] = json!(t);
        }
        if let Some(m) = request.max_tokens {
            generation["maxOutputTokens"] = json!(m);
        }
        if !generation.as_object().unwrap().is_empty() {
            body["generationConfig"] = generation;
        }

        let resp = self
            .http
            .post(url)
            .header("x-goog-api-key", &key)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .context("connecting to Google")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Google error ({status}): {}",
                text.chars().take(500).collect::<String>()
            ));
        }

        let byte_stream = resp.bytes_stream();
        let (tx, rx) = mpsc::unbounded::<StreamEvent>();
        tokio::spawn(pump_sse(byte_stream, tx));
        Ok(rx.boxed())
    }
}

fn send(tx: &mpsc::UnboundedSender<StreamEvent>, ev: StreamEvent) -> bool {
    tx.unbounded_send(ev).is_ok()
}

/// Parser state for one Gemini SSE stream.
#[derive(Default)]
struct SseState {
    tool_ordinal: usize,
    usage: TokenUsage,
    usage_sent: bool,
    done_sent: bool,
}

impl SseState {
    /// Returns `false` when the stream is finished or the receiver is gone.
    fn handle_line(&mut self, line: &str, tx: &mpsc::UnboundedSender<StreamEvent>) -> bool {
        let Some(data) = parse_sse_data_line(line) else {
            return true;
        };
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return true;
        };
        if let Some(err) = v["error"]["message"].as_str() {
            return send(tx, StreamEvent::TextDelta(format!("\n[provider error: {err}]")));
        }
        let mut ok = true;
        if let Some(meta) = v.get("usageMetadata") {
            self.usage.input_tokens = meta["promptTokenCount"]
                .as_u64()
                .unwrap_or(self.usage.input_tokens);
            self.usage.output_tokens = meta["candidatesTokenCount"]
                .as_u64()
                .unwrap_or(self.usage.output_tokens);
        }
        if let Some(candidate) = v["candidates"].get(0) {
            if let Some(parts) = candidate["content"]["parts"].as_array() {
                for part in parts {
                    if let Some(text) = part["text"].as_str() {
                        if !text.is_empty() {
                            ok &= send(tx, StreamEvent::TextDelta(text.to_string()));
                        }
                    }
                    if let Some(fc) = part.get("functionCall") {
                        let name = fc["name"].as_str().unwrap_or("unknown").to_string();
                        let args = fc.get("args").cloned().unwrap_or(json!({}));
                        let index = self.tool_ordinal;
                        self.tool_ordinal += 1;
                        ok &= send(tx, StreamEvent::ToolCallComplete {
                            index,
                            id: format!("gcall-{index}"),
                            name,
                            arguments: args.to_string(),
                        });
                    }
                }
            }
            if let Some(reason) = candidate["finishReason"].as_str() {
                self.finish(tx, Some(match reason {
                    "MAX_TOKENS" => StopReason::MaxTokens,
                    "SAFETY" | "RECITATION" | "PROHIBITED_CONTENT" => StopReason::ContentFilter,
                    _ => StopReason::Stop,
                }));
                // Gemini's final chunk carries finishReason (and usage);
                // nothing useful follows.
                return false;
            }
        }
        ok && !tx.is_closed()
    }

    fn finish(&mut self, tx: &mpsc::UnboundedSender<StreamEvent>, reason: Option<StopReason>) -> bool {
        let mut ok = true;
        if !self.usage_sent {
            self.usage_sent = true;
            ok &= send(tx, StreamEvent::Usage(self.usage));
        }
        if !self.done_sent {
            self.done_sent = true;
            ok &= send(tx, StreamEvent::Done { stop_reason: reason });
        }
        ok
    }
}

/// Read an SSE byte stream and forward normalized events until the final
/// candidate, end of stream, a transport error, or cancellation.
async fn pump_sse<S, B, E>(mut stream: S, tx: mpsc::UnboundedSender<StreamEvent>)
where
    S: futures::Stream<Item = std::result::Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut lines = SseLineBuffer::new();
    let mut state = SseState::default();
    'read: while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = send(&tx, StreamEvent::TextDelta(format!("\n[stream error: {e}]")));
                break;
            }
        };
        for line in lines.push(chunk.as_ref()) {
            if !state.handle_line(&line, &tx) {
                break 'read;
            }
        }
    }
    if !state.done_sent {
        if let Some(line) = lines.finish() {
            let _ = state.handle_line(&line, &tx);
        }
    }
    if !tx.is_closed() {
        state.finish(&tx, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_common::ChatMessage;
    use jcode_common::{ToolCall, ToolSpec};

    #[test]
    fn converts_function_call_roundtrip() {
        let req = ModelRequest {
            model: "gemini-test".into(),
            messages: vec![
                ChatMessage::new(Role::User, "go"),
                ChatMessage {
                    role: Role::Assistant,
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "c1".into(),
                        name: "fs_read".into(),
                        arguments: "{\"path\":\"x\"}".into(),
                    }],
                    tool_call_id: None,
                    tool_name: None,
                },
                ChatMessage {
                    role: Role::Tool,
                    content: "file content here".into(),
                    tool_calls: vec![],
                    tool_call_id: Some("c1".into()),
                    tool_name: Some("fs_read".into()),
                },
            ],
            tools: vec![ToolSpec {
                name: "fs_read".into(),
                description: "read".into(),
                parameters: json!({"type":"object"}),
            }],
            system: Some("sys".into()),
            temperature: None,
            max_tokens: None,
        };
        let contents = GoogleProvider::convert_contents(&req).unwrap();
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[1]["parts"][0]["functionCall"]["name"], "fs_read");
        assert_eq!(contents[2]["role"], "user");
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["name"], "fs_read"
        );
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["response"]["result"],
            "file content here"
        );
    }

    async fn collect(chunks: Vec<Vec<u8>>) -> Vec<StreamEvent> {
        let stream = futures::stream::iter(
            chunks.into_iter().map(Ok::<Vec<u8>, std::io::Error>),
        );
        let (tx, rx) = mpsc::unbounded::<StreamEvent>();
        pump_sse(stream, tx).await;
        rx.collect().await
    }

    #[tokio::test]
    async fn sse_stream_with_split_utf8_and_function_call() {
        let body = [
            r#"data: {"candidates":[{"content":{"parts":[{"text":"こんにちは"}]}}]}"#,
            "",
            r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"fs_read","args":{"path":"a"}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":2}}"#,
            "",
        ]
        .join("\r\n")
        .into_bytes();
        // Split inside the first 3-byte Japanese character.
        let first = body.iter().position(|&b| b >= 0x80).unwrap();
        let events = collect(vec![
            body[..first + 1].to_vec(),
            body[first + 1..].to_vec(),
        ])
        .await;
        let mut text = String::new();
        let mut call = None;
        let mut usage = None;
        let mut dones = 0;
        for ev in &events {
            match ev {
                StreamEvent::TextDelta(t) => text.push_str(t),
                StreamEvent::ToolCallComplete { name, arguments, .. } => {
                    call = Some((name.clone(), arguments.clone()))
                }
                StreamEvent::Usage(u) => usage = Some((u.input_tokens, u.output_tokens)),
                StreamEvent::Done { .. } => dones += 1,
                _ => {}
            }
        }
        assert_eq!(text, "こんにちは");
        assert_eq!(call, Some(("fs_read".to_string(), "{\"path\":\"a\"}".to_string())));
        assert_eq!(usage, Some((4, 2)));
        assert_eq!(dones, 1);
        assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
    }
}
