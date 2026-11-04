//! Event payloads sent from the Rust core to the frontend over Tauri.

use serde::{Deserialize, Serialize};

use crate::{
    ApprovalRecord, MessageRecord, TokenUsage, ToolRunRecord, ToolRunStatus,
};

/// Tauri event names (channel names). Keep in sync with `frontend/src/lib/ipc.ts`.
pub const EVENT_CHAT_STATUS: &str = "chat://status";
pub const EVENT_CHAT_DELTA: &str = "chat://delta";
pub const EVENT_CHAT_MESSAGE: &str = "chat://message";
pub const EVENT_CHAT_TOOL: &str = "chat://tool";
pub const EVENT_CHAT_APPROVAL: &str = "chat://approval";
pub const EVENT_CHAT_USAGE: &str = "chat://usage";
pub const EVENT_CHAT_ERROR: &str = "chat://error";
pub const EVENT_TERMINAL_DATA: &str = "terminal://data";
pub const EVENT_TERMINAL_EXIT: &str = "terminal://exit";
pub const EVENT_FS_CHANGED: &str = "fs://changed";
pub const EVENT_SESSIONS_CHANGED: &str = "sessions://changed";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatStatus {
    Idle,
    Thinking,
    Streaming,
    RunningTool,
    WaitingApproval,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatStatusEvent {
    pub session_id: String,
    pub status: ChatStatus,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatDeltaEvent {
    pub session_id: String,
    pub message_id: String,
    pub delta: String,
    /// The delta is model reasoning, not answer text.
    #[serde(default)]
    pub reasoning: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEvent {
    pub session_id: String,
    pub message: MessageRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRunEvent {
    pub session_id: String,
    pub tool_run: ToolRunRecord,
}

impl ToolRunEvent {
    pub fn status(&self) -> ToolRunStatus {
        self.tool_run.status
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalEvent {
    pub session_id: String,
    pub approval: ApprovalRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    pub session_id: String,
    pub usage: TokenUsage,
    #[serde(default)]
    pub cost_estimate: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatErrorEvent {
    pub session_id: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalDataEvent {
    pub terminal_id: String,
    pub data: String,
    /// Bytes of output so far, including this chunk; lets a late viewer
    /// join a snapshot (`terminal_snapshot`) without gaps or repeats.
    #[serde(default)]
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalExitEvent {
    pub terminal_id: String,
    pub code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsChangedEvent {
    pub root: String,
    /// Relative paths that changed (best effort, from the OS watcher).
    #[serde(default)]
    pub paths: Vec<String>,
}
