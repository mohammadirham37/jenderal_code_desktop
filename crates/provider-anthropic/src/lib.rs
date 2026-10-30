//! Anthropic Messages API adapter (streaming + tool use).

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
use std::collections::HashMap;
use std::sync::Arc;

pub const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u64 = 8_192;

pub struct AnthropicProvider {
    id: String,
    name: String,
    base_url: String,
    creds: Arc<dyn CredentialStore>,
    credential_ref: Option<String>,
    http: reqwest::Client,
}

impl AnthropicProvider {
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

    fn convert_messages(request: &ModelRequest) -> Result<Vec<Value>> {
        let mut out: Vec<Value> = Vec::new();
        for m in &request.messages {
            match m.role {
                Role::System => {
                    // Anthropic takes system separately; if one sneaks into
                    // the history, fold it into the next user turn.
                    if let Some(last) = out.last_mut() {
                        if last["role"] == "user" {
                            last["content"]
                                .as_array_mut()
                                .unwrap()
                                .push(json!({"type": "text", "text": m.content}));
                            continue;
                        }
                    }
                    out.push(json!({
                        "role": "user",
                        "content": [{"type": "text", "text": m.content}]
                    }));
                }
                Role::User => {
                    // A user message right after tool results (steering)
                    // joins their turn: roles must alternate.
                    if let Some(last) = out.last_mut() {
                        if last["role"] == "user" && last["content"].is_array() {
                            last["content"]
                                .as_array_mut()
                                .unwrap()
                                .push(json!({"type": "text", "text": m.content}));
                            continue;
                        }
                    }
                    out.push(json!({
                        "role": "user",
                        "content": [{"type": "text", "text": m.content}]
                    }));
                }
                Role::Assistant => {
                    let mut blocks = Vec::new();
                    if !m.content.is_empty() {
                        blocks.push(json!({"type": "text", "text": m.content}));
                    }
                    for c in &m.tool_calls {
                        let input: Value = if c.arguments.trim().is_empty() {
                            json!({})
                        } else {
                            serde_json::from_str(&c.arguments)
                                .with_context(|| format!("parsing tool arguments for {}", c.name))?
                        };
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": c.id,
                            "name": c.name,
                            "input": input
                        }));
                    }
                    if blocks.is_empty() {
                        blocks.push(json!({"type": "text", "text": ""}));
                    }
                    out.push(json!({"role": "assistant", "content": blocks}));
                }
                Role::Tool => {
                    // Tool results are user-role tool_result blocks. Merge
                    // consecutive tool results into a single user turn
                    // (Anthropic requires one turn per assistant response).
                    let block = json!({
                        "type": "tool_result",
                        "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                        "content": m.content,
                    });
                    if let Some(last) = out.last_mut() {
                        if last["role"] == "user"
                            && last["content"].is_array()
                            && last["content"][0]["type"] == "tool_result"
                        {
                            last["content"].as_array_mut().unwrap().push(block);
                            continue;
                        }
                    }
                    out.push(json!({"role": "user", "content": [block]}));
                }
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl AiProvider for AnthropicProvider {
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
            .ok_or_else(|| anyhow::anyhow!("Anthropic requires an API key"))?;
        let resp = self
            .http
            .get(format!("{}/v1/models?limit=100", self.base_url))
            .header("x-api-key", &key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .timeout(NON_STREAMING_TIMEOUT)
            .send()
            .await
            .context("connecting to Anthropic")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Anthropic list models failed ({status}): {}",
                body.chars().take(300).collect::<String>()
            ));
        }
        let v: Value = serde_json::from_str(&body).context("parsing /v1/models response")?;
        let mut models = Vec::new();
        if let Some(data) = v["data"].as_array() {
            for m in data {
                let Some(name) = m["id"].as_str() else { continue };
                models.push(ModelInfo {
                    id: format!("{}/{}", self.id, name),
                    provider_id: self.id.clone(),
                    name: name.to_string(),
                    display_name: m["display_name"].as_str().unwrap_or(name).to_string(),
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
            .ok_or_else(|| anyhow::anyhow!("Anthropic requires an API key"))?;

        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            "messages": Self::convert_messages(&request)?,
            "stream": true,
        });
        if let Some(sys) = &request.system {
            body["system"] = json!(sys);
        }
        if !request.tools.is_empty() {
            body["tools"] = Value::Array(
                request
                    .tools
                    .iter()
                    .map(|t| {
                        json!({
                            "name": t.name,
                            "description": t.description,
                            "input_schema": t.parameters,
                        })
                    })
                    .collect(),
            );
        }
        if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }

        let resp = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .context("connecting to Anthropic")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Anthropic error ({status}): {}",
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

enum Flow {
    Continue,
    /// Stream finished or receiver dropped (cancel): stop reading.
    Stop,
}

/// Parser state for one Anthropic Messages SSE stream.
#[derive(Default)]
struct SseState {
    /// Anthropic content-block index -> tool-call ordinal.
    block_to_ordinal: HashMap<usize, usize>,
    next_ordinal: usize,
    input_tokens: u64,
    output_tokens: u64,
    usage_sent: bool,
    done_sent: bool,
}

impl SseState {
    fn send_usage(&mut self, tx: &mpsc::UnboundedSender<StreamEvent>) -> bool {
        if self.usage_sent {
            return true;
        }
        self.usage_sent = true;
        send(tx, StreamEvent::Usage(TokenUsage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
        }))
    }

    fn send_done(&mut self, tx: &mpsc::UnboundedSender<StreamEvent>, reason: Option<StopReason>) -> bool {
        if self.done_sent {
            return true;
        }
        self.done_sent = true;
        send(tx, StreamEvent::Done { stop_reason: reason })
    }

    fn handle_line(&mut self, line: &str, tx: &mpsc::UnboundedSender<StreamEvent>) -> Flow {
        let Some(data) = parse_sse_data_line(line) else {
            return Flow::Continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return Flow::Continue;
        };
        let mut ok = true;
        match v["type"].as_str().unwrap_or("") {
            "error" => {
                let msg = v["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown Anthropic stream error");
                ok &= send(tx, StreamEvent::TextDelta(format!("\n[provider error: {msg}]")));
            }
            "message_start" => {
                self.input_tokens = v["message"]["usage"]["input_tokens"].as_u64().unwrap_or(0);
            }
            "content_block_start" => {
                let index = v["index"].as_u64().unwrap_or(0) as usize;
                let block = &v["content_block"];
                if block["type"] == "tool_use" {
                    let ordinal = self.next_ordinal;
                    self.next_ordinal += 1;
                    self.block_to_ordinal.insert(index, ordinal);
                    ok &= send(tx, StreamEvent::ToolCallDelta {
                        index: ordinal,
                        id: block["id"].as_str().map(String::from),
                        name: block["name"].as_str().map(String::from),
                        arguments_delta: String::new(),
                    });
                }
            }
            "content_block_delta" => {
                let index = v["index"].as_u64().unwrap_or(0) as usize;
                let delta = &v["delta"];
                match delta["type"].as_str().unwrap_or("") {
                    "text_delta" => {
                        if let Some(text) = delta["text"].as_str() {
                            if !text.is_empty() {
                                ok &= send(tx, StreamEvent::TextDelta(text.to_string()));
                            }
                        }
                    }
                    "input_json_delta" => {
                        if let Some(part) = delta["partial_json"].as_str() {
                            if let Some(&ordinal) = self.block_to_ordinal.get(&index) {
                                ok &= send(tx, StreamEvent::ToolCallDelta {
                                    index: ordinal,
                                    id: None,
                                    name: None,
                                    arguments_delta: part.to_string(),
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                self.output_tokens = v["usage"]["output_tokens"]
                    .as_u64()
                    .unwrap_or(self.output_tokens);
                if let Some(reason) = v["delta"]["stop_reason"].as_str() {
                    ok &= self.send_usage(tx);
                    ok &= self.send_done(tx, Some(match reason {
                        "tool_use" => StopReason::ToolCalls,
                        "max_tokens" => StopReason::MaxTokens,
                        "refusal" => StopReason::ContentFilter,
                        _ => StopReason::Stop,
                    }));
                }
            }
            "message_stop" => {
                self.send_usage(tx);
                self.send_done(tx, Some(StopReason::Stop));
                return Flow::Stop;
            }
            _ => {}
        }
        if ok && !tx.is_closed() {
            Flow::Continue
        } else {
            Flow::Stop
        }
    }
}

/// Read an SSE byte stream and forward normalized events until
/// `message_stop`, end of stream, a transport error, or cancellation.
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
            if let Flow::Stop = state.handle_line(&line, &tx) {
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
        state.send_done(&tx, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_common::ChatMessage;
    use jcode_common::{ToolCall, ToolSpec};

    #[test]
    fn converts_full_conversation_shape() {
        let req = ModelRequest {
            model: "claude-test".into(),
            messages: vec![
                ChatMessage::new(Role::User, "hi"),
                ChatMessage {
                    role: Role::Assistant,
                    content: "let me check".into(),
                    tool_calls: vec![
                        ToolCall {
                            id: "t1".into(),
                            name: "fs_read".into(),
                            arguments: "{\"path\":\"a\"}".into(),
                        },
                        ToolCall {
                            id: "t2".into(),
                            name: "fs_read".into(),
                            arguments: "{\"path\":\"b\"}".into(),
                        },
                    ],
                    tool_call_id: None,
                    tool_name: None,
                    reasoning: None,
                },
                ChatMessage {
                    role: Role::Tool,
                    content: "A".into(),
                    tool_calls: vec![],
                    tool_call_id: Some("t1".into()),
                    tool_name: Some("fs_read".into()),
                    reasoning: None,
                },
                ChatMessage {
                    role: Role::Tool,
                    content: "B".into(),
                    tool_calls: vec![],
                    tool_call_id: Some("t2".into()),
                    tool_name: Some("fs_read".into()),
                    reasoning: None,
                },
            ],
            tools: vec![ToolSpec {
                name: "fs_read".into(),
                description: "read a file".into(),
                parameters: json!({"type":"object"}),
            }],
            system: Some("sys".into()),
            temperature: None,
            max_tokens: Some(100),
        };
        let msgs = AnthropicProvider::convert_messages(&req).unwrap();
        // user, assistant(text+2 tool_use), merged user with 2 tool_result
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1]["content"].as_array().unwrap().len(), 3);
        assert_eq!(msgs[1]["content"][1]["type"], "tool_use");
        assert_eq!(msgs[2]["role"], "user");
        let blocks = msgs[2]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "tool_result");
        assert_eq!(blocks[0]["tool_use_id"], "t1");
        assert_eq!(blocks[1]["tool_use_id"], "t2");
    }

    #[test]
    fn user_message_after_tool_results_joins_their_turn() {
        let req = ModelRequest {
            model: "claude-test".into(),
            messages: vec![
                ChatMessage::new(Role::User, "hi"),
                ChatMessage {
                    role: Role::Assistant,
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "t1".into(),
                        name: "fs_read".into(),
                        arguments: "{}".into(),
                    }],
                    tool_call_id: None,
                    tool_name: None,
                    reasoning: None,
                },
                ChatMessage {
                    role: Role::Tool,
                    content: "A".into(),
                    tool_calls: vec![],
                    tool_call_id: Some("t1".into()),
                    tool_name: Some("fs_read".into()),
                    reasoning: None,
                },
                ChatMessage::new(Role::User, "also check b"),
            ],
            tools: vec![],
            system: None,
            temperature: None,
            max_tokens: Some(100),
        };
        let msgs = AnthropicProvider::convert_messages(&req).unwrap();
        assert_eq!(msgs.len(), 3, "roles keep alternating");
        let blocks = msgs[2]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "tool_result");
        assert_eq!(blocks[1]["type"], "text");
        assert_eq!(blocks[1]["text"], "also check b");
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
    async fn sse_stream_with_split_utf8_and_tool_use() {
        let body = [
            r#"event: message_start"#,
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":7}}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"héllo €"}}"#,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"fs_read"}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a\"}"}}"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}"#,
            r#"data: {"type":"message_stop"}"#,
            "",
        ]
        .join("\n")
        .into_bytes();
        let euro = body.iter().position(|&b| b == 0xE2).unwrap();
        let events = collect(vec![
            body[..euro + 2].to_vec(),
            body[euro + 2..].to_vec(),
        ])
        .await;
        let mut text = String::new();
        let mut args = String::new();
        let mut dones = 0;
        let mut usage = None;
        for ev in &events {
            match ev {
                StreamEvent::TextDelta(t) => text.push_str(t),
                StreamEvent::ToolCallDelta { arguments_delta, .. } => args.push_str(arguments_delta),
                StreamEvent::Usage(u) => usage = Some((u.input_tokens, u.output_tokens)),
                StreamEvent::Done { stop_reason } => {
                    dones += 1;
                    assert_eq!(*stop_reason, Some(StopReason::ToolCalls));
                }
                _ => {}
            }
        }
        assert_eq!(text, "héllo €");
        assert_eq!(args, "{\"path\":\"a\"}");
        assert_eq!(usage, Some((7, 3)));
        assert_eq!(dones, 1);
    }

    #[tokio::test]
    async fn stops_reading_when_receiver_dropped() {
        let (tx, rx) = mpsc::unbounded::<StreamEvent>();
        drop(rx);
        let polled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let p = polled.clone();
        let stream = futures::stream::iter(0..100).map(move |_| {
            p.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok::<Vec<u8>, std::io::Error>(
                br#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}
"#
                .to_vec(),
            )
        });
        pump_sse(Box::pin(stream), tx).await;
        assert_eq!(polled.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
