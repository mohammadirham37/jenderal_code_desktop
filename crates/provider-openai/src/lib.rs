//! OpenAI-compatible chat-completions adapter.
//!
//! Covers OpenAI and every endpoint speaking the same protocol: DeepSeek,
//! Mistral, xAI, Groq, OpenRouter-style gateways, internal proxies, Ollama
//! and LM Studio local servers, and user-configured custom endpoints.

use anyhow::{anyhow, Context, Result};
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

pub struct OpenAiProvider {
    id: String,
    name: String,
    base_url: String,
    creds: Arc<dyn CredentialStore>,
    credential_ref: Option<String>,
    http: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        base_url: impl Into<String>,
        creds: Arc<dyn CredentialStore>,
        credential_ref: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            base_url: normalize_base_url(base_url.into().as_str()),
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

    fn convert_messages(request: &ModelRequest) -> Vec<Value> {
        let mut out = Vec::new();
        if let Some(sys) = &request.system {
            out.push(json!({"role": "system", "content": sys}));
        }
        for m in &request.messages {
            match m.role {
                Role::System => out.push(json!({"role": "system", "content": m.content})),
                Role::User if m.images.is_empty() => out.push(json!({"role": "user", "content": m.content})),
                Role::User => {
                    let mut parts = vec![json!({"type": "text", "text": m.content})];
                    for img in &m.images {
                        parts.push(json!({
                            "type": "image_url",
                            "image_url": {"url": format!("data:{};base64,{}", img.mime, img.data)}
                        }));
                    }
                    out.push(json!({"role": "user", "content": parts}));
                }
                Role::Assistant => {
                    let mut msg = if m.tool_calls.is_empty() {
                        json!({"role": "assistant", "content": m.content})
                    } else {
                        let calls: Vec<Value> = m
                            .tool_calls
                            .iter()
                            .map(|c| {
                                json!({
                                    "id": c.id,
                                    "type": "function",
                                    "function": {"name": c.name, "arguments": c.arguments}
                                })
                            })
                            .collect();
                        let content = if m.content.is_empty() {
                            Value::Null
                        } else {
                            json!(m.content)
                        };
                        json!({
                            "role": "assistant",
                            "content": content,
                            "tool_calls": calls
                        })
                    };
                    // Interleaved-thinking models (GLM, DeepSeek, Kimi) read
                    // their earlier reasoning back between tool calls. The
                    // runtime only keeps it for the turn in progress.
                    if let Some(r) = m.reasoning.as_deref().filter(|r| !r.is_empty()) {
                        msg["reasoning_content"] = json!(r);
                    }
                    out.push(msg);
                }
                Role::Tool => {
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                        "content": m.content,
                    }));
                }
            }
        }
        out
    }

    fn convert_tools(request: &ModelRequest) -> Vec<Value> {
        request
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect()
    }
}

fn normalize_base_url(url: &str) -> String {
    let mut u = url.trim().trim_end_matches('/').to_string();
    if !u.starts_with("http://") && !u.starts_with("https://") {
        u = format!("https://{u}");
    }
    u
}

#[async_trait]
impl AiProvider for OpenAiProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let mut req = self
            .http
            .get(format!("{}/models", self.base_url))
            .timeout(NON_STREAMING_TIMEOUT);
        if let Some(key) = self.api_key().await? {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.context("connecting to provider")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(extract_api_error(&body, status.as_u16(), "list models"));
        }
        let v: Value = serde_json::from_str(&body).context("parsing /models response")?;
        let mut models = Vec::new();
        if let Some(data) = v["data"].as_array() {
            for m in data {
                let Some(name) = m["id"].as_str() else { continue };
                models.push(ModelInfo {
                    id: format!("{}/{}", self.id, name),
                    provider_id: self.id.clone(),
                    name: name.to_string(),
                    display_name: m
                        .get("display_name")
                        .and_then(|d| d.as_str())
                        .unwrap_or(name)
                        .to_string(),
                    // Unknown capabilities are reported as unknown (PRD §6.3).
                    capabilities: ModelCapabilities {
                        streaming: Some(true),
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
        let mut body = json!({
            "model": request.model,
            "messages": Self::convert_messages(&request),
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        let tools = Self::convert_tools(&request);
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        if let Some(t) = request.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(m) = request.max_tokens {
            body["max_tokens"] = json!(m);
        }

        let mut req = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .header("content-type", "application/json")
            .body(body.to_string());
        if let Some(key) = self.api_key().await? {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.context("connecting to provider")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(extract_api_error(&text, status.as_u16(), "chat completions"));
        }

        let byte_stream = resp.bytes_stream();
        let (tx, rx) = mpsc::unbounded::<StreamEvent>();
        tokio::spawn(pump_sse(byte_stream, tx));
        Ok(rx.boxed())
    }
}

/// Parser state for one OpenAI-compatible SSE stream.
#[derive(Default)]
struct SseState {
    /// Stop reason from `finish_reason`; `Done` is deferred until `[DONE]`
    /// (or end of stream) so the trailing usage-only chunk is not lost.
    stop_reason: Option<StopReason>,
}

enum Flow {
    Continue,
    /// `[DONE]` seen or receiver dropped: stop reading the network.
    Stop,
}

fn send(tx: &mpsc::UnboundedSender<StreamEvent>, ev: StreamEvent) -> bool {
    tx.unbounded_send(ev).is_ok()
}

impl SseState {
    fn handle_line(&mut self, line: &str, tx: &mpsc::UnboundedSender<StreamEvent>) -> Flow {
        let Some(data) = parse_sse_data_line(line) else {
            return Flow::Continue;
        };
        if data == "[DONE]" {
            let _ = send(tx, StreamEvent::Done {
                stop_reason: Some(self.stop_reason.unwrap_or(StopReason::Stop)),
            });
            return Flow::Stop;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return Flow::Continue;
        };
        let mut ok = true;
        if let Some(err) = v["error"]["message"].as_str() {
            ok = send(tx, StreamEvent::TextDelta(format!("\n[provider error: {err}]")));
            return if ok { Flow::Continue } else { Flow::Stop };
        }
        if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
            ok &= send(tx, StreamEvent::Usage(TokenUsage {
                input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
                output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
            }));
        }
        if let Some(choice) = v["choices"].get(0) {
            if let Some(delta) = choice.get("delta") {
                // `reasoning_content` (DeepSeek, GLM, Qwen, vLLM) or
                // `reasoning` (OpenRouter, Ollama).
                let reasoning = delta["reasoning_content"].as_str().or_else(|| delta["reasoning"].as_str());
                if let Some(text) = reasoning.filter(|t| !t.is_empty()) {
                    ok &= send(tx, StreamEvent::ReasoningDelta(text.to_string()));
                }
                if let Some(text) = delta["content"].as_str() {
                    if !text.is_empty() {
                        ok &= send(tx, StreamEvent::TextDelta(text.to_string()));
                    }
                }
                if let Some(calls) = delta["tool_calls"].as_array() {
                    for call in calls {
                        let index = call["index"].as_u64().unwrap_or(0) as usize;
                        ok &= send(tx, StreamEvent::ToolCallDelta {
                            index,
                            id: call["id"].as_str().map(String::from),
                            name: call["function"]["name"].as_str().map(String::from),
                            arguments_delta: call["function"]["arguments"]
                                .as_str()
                                .unwrap_or("")
                                .to_string(),
                        });
                    }
                }
            }
            if let Some(reason) = choice["finish_reason"].as_str() {
                self.stop_reason = Some(match reason {
                    "tool_calls" | "function_call" => StopReason::ToolCalls,
                    "length" => StopReason::MaxTokens,
                    "content_filter" => StopReason::ContentFilter,
                    _ => StopReason::Stop,
                });
            }
        }
        if ok && !tx.is_closed() {
            Flow::Continue
        } else {
            Flow::Stop
        }
    }
}

/// Read an SSE byte stream and forward normalized events until `[DONE]`,
/// end of stream, a transport error, or the receiver being dropped (cancel).
async fn pump_sse<S, B, E>(mut stream: S, tx: mpsc::UnboundedSender<StreamEvent>)
where
    S: futures::Stream<Item = std::result::Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut lines = SseLineBuffer::new();
    let mut state = SseState::default();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                // The runtime retries the step; no Done after an error.
                let _ = send(&tx, StreamEvent::Error(e.to_string()));
                return;
            }
        };
        for line in lines.push(chunk.as_ref()) {
            if let Flow::Stop = state.handle_line(&line, &tx) {
                return;
            }
        }
    }
    if let Some(line) = lines.finish() {
        if let Flow::Stop = state.handle_line(&line, &tx) {
            return;
        }
    }
    let _ = send(&tx, StreamEvent::Done { stop_reason: state.stop_reason });
}

/// Pull a human-readable message out of an error response body.
pub fn extract_api_error(body: &str, status: u16, ctx: &str) -> anyhow::Error {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        if let Some(msg) = v["error"]["message"].as_str() {
            let code = v["error"]["code"].as_i64().unwrap_or(status as i64);
            return anyhow!("provider error ({code}) while calling {ctx}: {msg}");
        }
        if let Some(msg) = v["message"].as_str() {
            return anyhow!("provider error ({status}) while calling {ctx}: {msg}");
        }
    }
    anyhow!("provider error ({status}) while calling {ctx}: {}", truncate(body, 300))
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_images_become_image_url_parts() {
        let mut m = ChatMessage::new(Role::User, "what is this?");
        m.images.push(jcode_common::ImageAttachment { mime: "image/png".into(), data: "AAAA".into() });
        let req = ModelRequest {
            model: "m".into(),
            messages: vec![m],
            tools: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
        };
        let msgs = OpenAiProvider::convert_messages(&req);
        let parts = msgs.last().unwrap()["content"].as_array().unwrap().clone();
        assert_eq!(parts[0]["text"], "what is this?");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,AAAA");
    }
    use jcode_common::ChatMessage;
    use jcode_common::{ToolCall, ToolSpec};

    fn sample_req() -> ModelRequest {
        ModelRequest {
            model: "gpt-test".into(),
            messages: vec![
                ChatMessage::new(Role::User, "hi"),
                ChatMessage {
                    images: vec![],
                    role: Role::Assistant,
                    content: "".into(),
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "fs_read".into(),
                        arguments: "{\"path\":\"a.txt\"}".into(),
                    }],
                    tool_call_id: None,
                    tool_name: None,
                    reasoning: None,
                },
                ChatMessage {
                    images: vec![],
                    role: Role::Tool,
                    content: "file body".into(),
                    tool_calls: vec![],
                    tool_call_id: Some("call_1".into()),
                    tool_name: Some("fs_read".into()),
                    reasoning: None,
                },
            ],
            tools: vec![ToolSpec {
                name: "fs_read".into(),
                description: "read".into(),
                parameters: json!({"type":"object","properties":{"path":{"type":"string"}}}),
            }],
            system: Some("You are a helper.".into()),
            temperature: Some(0.2),
            max_tokens: Some(1024),
        }
    }

    #[test]
    fn message_conversion_includes_system_tools_and_results() {
        let converted = OpenAiProvider::convert_messages(&sample_req());
        assert_eq!(converted[0]["role"], "system");
        assert_eq!(converted[1]["role"], "user");
        assert_eq!(converted[2]["tool_calls"][0]["function"]["name"], "fs_read");
        assert_eq!(converted[3]["role"], "tool");
        assert_eq!(converted[3]["tool_call_id"], "call_1");

        let tools = OpenAiProvider::convert_tools(&sample_req());
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn assistant_reasoning_is_sent_back_as_reasoning_content() {
        let mut req = sample_req();
        req.messages[1].reasoning = Some("look at a.txt first".into());
        let converted = OpenAiProvider::convert_messages(&req);
        assert_eq!(converted[2]["reasoning_content"], "look at a.txt first");
        assert!(OpenAiProvider::convert_messages(&sample_req())[2].get("reasoning_content").is_none());
    }

    #[tokio::test]
    async fn reasoning_deltas_are_streamed_separately() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning\":\"more\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"answer\"}}]}\n\n",
            "data: [DONE]\n\n",
        );
        let events = collect(vec![body.as_bytes().to_vec()]).await;
        let reasoning: String = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ReasoningDelta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(reasoning, "think more");
        assert!(events.iter().any(|e| matches!(e, StreamEvent::TextDelta(t) if t == "answer")));
    }

    #[test]
    fn base_url_normalization() {
        assert_eq!(
            normalize_base_url("https://api.openai.com/v1/"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_base_url("localhost:11434/v1"),
            "https://localhost:11434/v1"
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
    async fn sse_chunks_are_parsed_into_events() {
        // finish_reason arrives before the usage-only chunk (stream_options
        // include_usage); usage must still be emitted before Done.
        let lines = [
            r#"data: {"choices":[{"delta":{"role":"assistant","content":"Hel"}}]}"#,
            r#"data: {"choices":[{"delta":{"content":"lo €"}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"fs_read","arguments":"{\"pa"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"x\"}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            r#"data: {"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
            "data: [DONE]",
            "",
        ]
        .join("\n");
        let bytes = lines.into_bytes();
        // Split in the middle of the 3-byte euro sign.
        let euro = bytes.iter().position(|&b| b == 0xE2).unwrap();
        let chunks = vec![
            bytes[..euro + 1].to_vec(),
            bytes[euro + 1..euro + 2].to_vec(),
            bytes[euro + 2..].to_vec(),
        ];
        let events = collect(chunks).await;

        let mut text = String::new();
        let mut args = String::new();
        let mut usage: Option<TokenUsage> = None;
        let mut dones = Vec::new();
        for ev in &events {
            match ev {
                StreamEvent::TextDelta(t) => text.push_str(t),
                StreamEvent::ToolCallDelta { arguments_delta, .. } => args.push_str(arguments_delta),
                StreamEvent::Usage(u) => usage = Some(*u),
                StreamEvent::Done { stop_reason } => dones.push(*stop_reason),
                _ => {}
            }
        }
        assert_eq!(text, "Hello €");
        assert_eq!(args, "{\"path\":\"x\"}");
        assert_eq!(usage.map(|u| (u.input_tokens, u.output_tokens)), Some((10, 5)));
        assert_eq!(dones, vec![Some(StopReason::ToolCalls)]);
        assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
    }

    #[tokio::test]
    async fn done_emitted_at_end_of_stream_without_done_marker() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}";
        let events = collect(vec![body.as_bytes().to_vec()]).await;
        assert!(matches!(events.as_slice(), [
            StreamEvent::TextDelta(_),
            StreamEvent::Done { stop_reason: Some(StopReason::Stop) }
        ]));
    }

    #[tokio::test]
    async fn stops_reading_when_receiver_dropped() {
        let (tx, rx) = mpsc::unbounded::<StreamEvent>();
        drop(rx);
        let polled = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let p = polled.clone();
        let stream = futures::stream::iter(0..100).map(move |_| {
            p.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok::<Vec<u8>, std::io::Error>(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n".to_vec(),
            )
        });
        pump_sse(Box::pin(stream), tx).await;
        assert_eq!(polled.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_broken_body_ends_with_an_error_event_not_text() {
        let (tx, rx) = mpsc::unbounded::<StreamEvent>();
        let chunks: Vec<Result<Vec<u8>, std::io::Error>> = vec![
            Ok(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n".to_vec()),
            Err(std::io::Error::other("error decoding response body")),
            Ok(b"data: {\"choices\":[{\"delta\":{\"content\":\"never\"}}]}\n".to_vec()),
        ];
        pump_sse(Box::pin(futures::stream::iter(chunks)), tx).await;
        let events: Vec<StreamEvent> = rx.collect().await;
        assert!(matches!(&events[0], StreamEvent::TextDelta(t) if t == "Hel"));
        assert!(matches!(&events[1], StreamEvent::Error(e) if e.contains("decoding response body")));
        assert_eq!(events.len(), 2, "nothing after the error, and no [stream error] text");
    }
}
