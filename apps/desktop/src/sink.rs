//! EventSink implementation that forwards runtime events to the frontend
//! via Tauri `emit`.
//!
//! Text deltas are coalesced: emitting one IPC event per model token floods
//! the webview. Deltas are buffered per session (and message) and flushed
//! every [`FLUSH_INTERVAL`] or once [`FLUSH_BYTES`] accumulate. Any other
//! event for the same session flushes that session's buffer first, so the
//! frontend always sees deltas before the message/tool/status that follows.

use async_trait::async_trait;
use jcode_agent_runtime::EventSink;
use jcode_common::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const FLUSH_INTERVAL: Duration = Duration::from_millis(30);
const FLUSH_BYTES: usize = 2048;

struct PendingDelta {
    message_id: String,
    text: String,
    /// A background flush task is already scheduled for this session.
    scheduled: bool,
}

/// Thread-safe delta coalescer, independent of Tauri so it can be tested.
/// `emit` is always invoked while the internal lock is held, which keeps the
/// relative order of flushed deltas and other events deterministic.
pub struct DeltaCoalescer {
    pending: Mutex<HashMap<String, PendingDelta>>,
    emit_delta: Box<dyn Fn(ChatDeltaEvent) + Send + Sync>,
}

pub enum DeltaAction {
    /// Nothing more to do; a flush is already scheduled.
    None,
    /// Caller should schedule a flush for this session after the interval.
    ScheduleFlush,
}

impl DeltaCoalescer {
    pub fn new(emit_delta: impl Fn(ChatDeltaEvent) + Send + Sync + 'static) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            emit_delta: Box::new(emit_delta),
        }
    }

    fn emit_pending(&self, session_id: &str, p: PendingDelta) {
        if !p.text.is_empty() {
            (self.emit_delta)(ChatDeltaEvent {
                session_id: session_id.to_string(),
                message_id: p.message_id,
                delta: p.text,
            });
        }
    }

    pub fn push(&self, session_id: &str, message_id: &str, delta: &str) -> DeltaAction {
        let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        // A delta for a different message flushes the previous one first.
        if let Some(p) = map.get_mut(session_id) {
            if p.message_id != message_id {
                let text = std::mem::take(&mut p.text);
                let old_id = std::mem::replace(&mut p.message_id, message_id.to_string());
                if !text.is_empty() {
                    (self.emit_delta)(ChatDeltaEvent {
                        session_id: session_id.to_string(),
                        message_id: old_id,
                        delta: text,
                    });
                }
            }
        }
        let entry = map.entry(session_id.to_string()).or_insert_with(|| PendingDelta {
            message_id: message_id.to_string(),
            text: String::new(),
            scheduled: false,
        });
        entry.text.push_str(delta);
        if entry.text.len() >= FLUSH_BYTES {
            let text = std::mem::take(&mut entry.text);
            (self.emit_delta)(ChatDeltaEvent {
                session_id: session_id.to_string(),
                message_id: entry.message_id.clone(),
                delta: text,
            });
            return DeltaAction::None;
        }
        if entry.scheduled {
            DeltaAction::None
        } else {
            entry.scheduled = true;
            DeltaAction::ScheduleFlush
        }
    }

    /// Flush any buffered text for `session_id`, then run `then` (the
    /// follow-up emit) under the same lock so ordering is preserved.
    pub fn flush_then(&self, session_id: &str, then: impl FnOnce()) {
        let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = map.remove(session_id) {
            self.emit_pending(session_id, p);
        }
        then();
    }

    /// Timer-driven flush.
    pub fn flush(&self, session_id: &str) {
        self.flush_then(session_id, || {});
    }
}

pub struct TauriEventSink {
    app: AppHandle,
    deltas: Arc<DeltaCoalescer>,
}

impl TauriEventSink {
    pub fn new(app: AppHandle) -> Self {
        let emit_app = app.clone();
        let deltas = Arc::new(DeltaCoalescer::new(move |ev| {
            let _ = emit_app.emit(EVENT_CHAT_DELTA, ev);
        }));
        Self { app, deltas }
    }

    /// Flush buffered deltas for the session, then emit `event`.
    fn emit_ordered<T: serde::Serialize + Clone>(&self, session_id: &str, name: &str, event: T) {
        self.deltas.flush_then(session_id, || {
            let _ = self.app.emit(name, event);
        });
    }
}

#[async_trait]
impl EventSink for TauriEventSink {
    async fn status(&self, session_id: &str, status: ChatStatus, detail: Option<String>) {
        self.emit_ordered(
            session_id,
            EVENT_CHAT_STATUS,
            ChatStatusEvent {
                session_id: session_id.into(),
                status,
                detail,
            },
        );
    }

    async fn delta(&self, session_id: &str, message_id: &str, delta: &str) {
        if let DeltaAction::ScheduleFlush = self.deltas.push(session_id, message_id, delta) {
            let deltas = self.deltas.clone();
            let sid = session_id.to_string();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(FLUSH_INTERVAL).await;
                deltas.flush(&sid);
            });
        }
    }

    async fn message(&self, session_id: &str, message: &MessageRecord) {
        self.emit_ordered(
            session_id,
            EVENT_CHAT_MESSAGE,
            MessageEvent {
                session_id: session_id.into(),
                message: message.clone(),
            },
        );
    }

    async fn tool_run(&self, session_id: &str, run: &ToolRunRecord) {
        self.emit_ordered(
            session_id,
            EVENT_CHAT_TOOL,
            ToolRunEvent {
                session_id: session_id.into(),
                tool_run: run.clone(),
            },
        );
    }

    async fn approval(&self, session_id: &str, approval: &ApprovalRecord) {
        self.emit_ordered(
            session_id,
            EVENT_CHAT_APPROVAL,
            ApprovalEvent {
                session_id: session_id.into(),
                approval: approval.clone(),
            },
        );
    }

    async fn usage(&self, session_id: &str, usage: TokenUsage, cost_estimate: Option<f64>) {
        self.emit_ordered(
            session_id,
            EVENT_CHAT_USAGE,
            UsageEvent {
                session_id: session_id.into(),
                usage,
                cost_estimate,
            },
        );
    }

    async fn error(&self, session_id: &str, error: &str) {
        self.emit_ordered(
            session_id,
            EVENT_CHAT_ERROR,
            ChatErrorEvent {
                session_id: session_id.into(),
                error: error.into(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collector() -> (Arc<Mutex<Vec<ChatDeltaEvent>>>, DeltaCoalescer) {
        let out = Arc::new(Mutex::new(Vec::new()));
        let o = out.clone();
        let c = DeltaCoalescer::new(move |ev| o.lock().unwrap().push(ev));
        (out, c)
    }

    #[test]
    fn deltas_are_coalesced_until_flush() {
        let (out, c) = collector();
        assert!(matches!(c.push("s", "m", "a"), DeltaAction::ScheduleFlush));
        assert!(matches!(c.push("s", "m", "b"), DeltaAction::None));
        assert!(out.lock().unwrap().is_empty());
        c.flush("s");
        let evs = out.lock().unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].delta, "ab");
        assert_eq!(evs[0].message_id, "m");
    }

    #[test]
    fn other_event_flushes_first_and_size_cap_flushes() {
        let (out, c) = collector();
        c.push("s", "m", "x");
        c.push("t", "m2", "other session");
        let order = Arc::new(Mutex::new(Vec::new()));
        let o2 = order.clone();
        let out2 = out.clone();
        c.flush_then("s", move || {
            o2.lock().unwrap().push(out2.lock().unwrap().len());
        });
        // The delta was emitted before the follow-up event ran.
        assert_eq!(*order.lock().unwrap(), vec![1]);
        let big = "y".repeat(FLUSH_BYTES);
        c.push("s", "m", &big);
        assert_eq!(out.lock().unwrap().len(), 2);
        // Message change flushes previous message's text.
        c.push("s", "m", "1");
        c.push("s", "n", "2");
        c.flush("s");
        let evs = out.lock().unwrap();
        assert_eq!(evs[2].message_id, "m");
        assert_eq!(evs[3].message_id, "n");
        // Other session untouched until flushed.
        assert!(evs.iter().all(|e| e.session_id == "s"));
    }
}
