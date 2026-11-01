//! Deterministic mock provider for demos and automated tests
//! (PRD §13: "Gunakan mock provider untuk pengujian otomatis").
//!
//! Behavior, based on the last message in the request:
//! - if the last message is a tool result -> produce a short final summary;
//! - if the user message asks for a "demo"/"test"/write-like task ->
//!   emit a `fs_write` tool call (which goes through the real permission
//!   engine, so approval flows can be exercised);
//! - otherwise -> stream a friendly text answer.

use anyhow::Result;
use async_trait::async_trait;
use futures::channel::mpsc;
use futures::stream::BoxStream;
use futures::StreamExt;
use jcode_common::{ModelInfo, ModelRequest, Role, StreamEvent, TokenUsage};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct MockProvider {
    id: String,
    name: String,
    turn_counter: Arc<AtomicUsize>,
}

impl MockProvider {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            turn_counter: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn plan_events(&self, request: &ModelRequest) -> Vec<StreamEvent> {
        let last = request.messages.last();
        let is_tool_result = last
            .map(|m| m.role == Role::Tool)
            .unwrap_or(false);

        let events = if is_tool_result {
            vec![
                StreamEvent::TextDelta("Done. ".into()),
                StreamEvent::TextDelta(
                    "The mock agent finished its scripted step; check the tool result above. \
                     Connect a real provider in Settings → Providers for actual model responses."
                        .into(),
                ),
                StreamEvent::Usage(TokenUsage {
                    input_tokens: 120,
                    output_tokens: 45,
                }),
                StreamEvent::Done {
                    stop_reason: Some(jcode_common::StopReason::Stop),
                },
            ]
        } else {
            let text = last
                .map(|m| m.content.to_lowercase())
                .unwrap_or_default();
            let wants_tool = ["demo", "test", "write", "create", "build", "buat", "tulis"]
                .iter()
                .any(|k| text.contains(k));
            if wants_tool {
                let args = serde_json::json!({
                    "path": "mock-demo.txt",
                    "content": "Hello from the JenderalCode mock provider!\nThis file was written through the real tool pipeline (permission engine included).\n"
                })
                .to_string();
                vec![
                    StreamEvent::TextDelta(
                        "Sure — I'll demonstrate the agent loop by writing a file. ".into(),
                    ),
                    StreamEvent::ToolCallDelta {
                        index: 0,
                        id: Some("mock-call-1".into()),
                        name: Some("fs_write".into()),
                        arguments_delta: args,
                    },
                    StreamEvent::Done {
                        stop_reason: Some(jcode_common::StopReason::ToolCalls),
                    },
                ]
            } else {
                vec![
                    StreamEvent::TextDelta("Hello! ".into()),
                    StreamEvent::TextDelta(
                        "I'm the built-in **mock provider** — no API key, no network. \
                         I stream responses token-by-token so you can try the chat UI, and if \
                         your message mentions writing/creating something I'll exercise a real \
                         tool call with the permission flow.\n\nTry: `demo` or `create a file`."
                            .into(),
                    ),
                    StreamEvent::Usage(TokenUsage {
                        input_tokens: 42,
                        output_tokens: 87,
                    }),
                    StreamEvent::Done {
                        stop_reason: Some(jcode_common::StopReason::Stop),
                    },
                ]
            }
        };
        events
    }
}

#[async_trait]
impl super::AiProvider for MockProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![ModelInfo {
            id: format!("{}/mock-standard", self.id),
            provider_id: self.id.clone(),
            name: "mock-standard".into(),
            display_name: "Mock Standard".into(),
            capabilities: jcode_common::ModelCapabilities {
                streaming: Some(true),
                tool_calling: Some(true),
                vision: Some(true),
                structured_output: Some(true),
                context_window: Some(128_000),
                max_output_tokens: Some(4_096),
            },
            input_price_per_mtok: Some(0.0),
            output_price_per_mtok: Some(0.0),
            custom: false,
        }])
    }

    async fn stream(&self, request: ModelRequest) -> Result<BoxStream<'static, StreamEvent>> {
        self.turn_counter.fetch_add(1, Ordering::SeqCst);
        let events = self.plan_events(&request);
        let (tx, rx) = mpsc::unbounded();
        tokio::spawn(async move {
            for event in events {
                // Small delay so the UI exercises real streaming behavior.
                tokio::time::sleep(Duration::from_millis(25)).await;
                if tx.unbounded_send(event).is_err() {
                    break; // receiver dropped (cancelled)
                }
            }
        });
        Ok(rx.boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AiProvider;
    use jcode_common::ChatMessage;

    #[tokio::test]
    async fn mock_streams_text_for_plain_greeting() {
        let p = MockProvider::new("mock", "Mock");
        let req = ModelRequest {
            model: "mock-standard".into(),
            messages: vec![ChatMessage::new(Role::User, "hello")],
            tools: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
        };
        let mut stream = p.stream(req).await.unwrap();
        let mut text = String::new();
        let mut saw_done = false;
        while let Some(ev) = stream.next().await {
            match ev {
                StreamEvent::TextDelta(t) => text.push_str(&t),
                StreamEvent::Done { .. } => saw_done = true,
                _ => {}
            }
        }
        assert!(text.contains("mock provider"));
        assert!(saw_done);
    }

    #[tokio::test]
    async fn mock_emits_tool_call_for_write_request() {
        let p = MockProvider::new("mock", "Mock");
        let req = ModelRequest {
            model: "mock-standard".into(),
            messages: vec![ChatMessage::new(Role::User, "please create a demo file")],
            tools: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
        };
        let mut stream = p.stream(req).await.unwrap();
        let mut tool_call = false;
        while let Some(ev) = stream.next().await {
            if let StreamEvent::ToolCallDelta { name, .. } = ev {
                if name.as_deref() == Some("fs_write") {
                    tool_call = true;
                }
            }
        }
        assert!(tool_call);
    }
}
