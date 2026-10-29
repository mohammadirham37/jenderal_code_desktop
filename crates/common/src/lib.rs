//! Shared types and events for JenderalCode Desktop.
//!
//! This crate has no dependencies on other workspace crates so that every
//! layer (storage, providers, tools, runtime, UI backend) can share the same
//! vocabulary without cycles.

pub mod events;

pub use events::*;

use serde::{Deserialize, Serialize};

/// Adapter kind used to route a provider configuration to a concrete
/// `AiProvider` implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Anything speaking the OpenAI chat-completions protocol
    /// (OpenAI, DeepSeek, Mistral, xAI, Groq, Ollama, LM Studio, gateways...).
    /// NOTE: explicit rename — serde's snake_case would split the acronym
    /// into `open_ai_compatible`, but `as_str()`/`parse()`/the frontend all
    /// use `openai_compatible`.
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    Anthropic,
    Google,
    /// Built-in deterministic provider used for demos and automated tests.
    Mock,
    /// External coding agent CLI (experimental).
    ExternalCli,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::OpenAiCompatible => "openai_compatible",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Google => "google",
            ProviderKind::Mock => "mock",
            ProviderKind::ExternalCli => "external_cli",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "openai_compatible" | "openai" => Some(ProviderKind::OpenAiCompatible),
            "anthropic" => Some(ProviderKind::Anthropic),
            "google" | "gemini" => Some(ProviderKind::Google),
            "mock" => Some(ProviderKind::Mock),
            "external_cli" => Some(ProviderKind::ExternalCli),
            _ => None,
        }
    }
}

/// A provider configuration as stored in SQLite. Secrets are never part of
/// this struct — only references into the OS credential store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
    /// Provider-specific parameters (e.g. API version, custom headers).
    #[serde(default)]
    pub extra: serde_json::Value,
    /// Credential reference in the OS keychain (account name), if a key is set.
    #[serde(default)]
    pub credential_ref: Option<String>,
}

/// Model capabilities. `None` means "unknown" — never guessed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calling: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Record id (`provider_id/model_name`) — stable across refreshes.
    pub id: String,
    pub provider_id: String,
    /// The name sent to the provider API.
    pub name: String,
    pub display_name: String,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    /// Optional pricing (USD per 1M tokens) used for cost estimates.
    #[serde(default)]
    pub input_price_per_mtok: Option<f64>,
    #[serde(default)]
    pub output_price_per_mtok: Option<f64>,
    /// True when the model was added manually rather than discovered.
    #[serde(default)]
    pub custom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub last_opened_at: i64,
    #[serde(default)]
    pub is_git_repo: bool,
}

/// Agent operating mode — controls tool availability and system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    Plan,
    Build,
    Review,
    Custom,
}

impl AgentMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentMode::Plan => "plan",
            AgentMode::Build => "build",
            AgentMode::Review => "review",
            AgentMode::Custom => "custom",
        }
    }
}

/// Permission policy preset (PRD §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum PolicyMode {
    /// Ask before every write, delete, and command.
    #[default]
    Safe,
    /// File edits inside the workspace are auto-applied; commands and
    /// deletions still ask.
    Balanced,
    /// Everything inside the workspace is auto-approved. Paths outside the
    /// workspace remain denied.
    Yolo,
}


impl PolicyMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            PolicyMode::Safe => "safe",
            PolicyMode::Balanced => "balanced",
            PolicyMode::Yolo => "yolo",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "balanced" => PolicyMode::Balanced,
            "yolo" => PolicyMode::Yolo,
            _ => PolicyMode::Safe,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub id: String,
    pub name: String,
    pub mode: AgentMode,
    pub system_prompt: String,
    /// `None` means all tools allowed.
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    pub policy_mode: PolicyMode,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub built_in: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    #[serde(default)]
    pub agent_profile_id: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub model_name: Option<String>,
    /// Per-session permission policy override (e.g. "yolo" full access).
    /// `None` = use the agent profile's default.
    #[serde(default)]
    pub policy_mode: Option<PolicyMode>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub message_count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON arguments as produced by the model.
    pub arguments: String,
}

/// A chat message in the internal (provider-neutral) format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Set for `Tool` role messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

impl ChatMessage {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
        }
    }
}

/// Persisted message row (DB shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub id: String,
    pub session_id: String,
    pub role: Role,
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRunStatus {
    Pending,
    AwaitingApproval,
    Running,
    Success,
    Denied,
    Error,
    Cancelled,
}

impl ToolRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolRunStatus::Pending => "pending",
            ToolRunStatus::AwaitingApproval => "awaiting_approval",
            ToolRunStatus::Running => "running",
            ToolRunStatus::Success => "success",
            ToolRunStatus::Denied => "denied",
            ToolRunStatus::Error => "error",
            ToolRunStatus::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRunRecord {
    pub id: String,
    pub session_id: String,
    #[serde(default)]
    pub message_id: Option<String>,
    pub tool_name: String,
    pub status: ToolRunStatus,
    #[serde(default)]
    pub args: serde_json::Value,
    #[serde(default)]
    pub result_summary: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub approval_id: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
}

impl ApprovalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalStatus::Pending => "pending",
            ApprovalStatus::Approved => "approved",
            ApprovalStatus::Denied => "denied",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub id: String,
    pub session_id: String,
    pub tool_run_id: String,
    /// Machine-readable action kind (see permissions crate).
    pub kind: String,
    /// Human-readable description of what will happen.
    pub description: String,
    /// Optional preview (unified diff, command line, ...).
    #[serde(default)]
    pub preview: Option<String>,
    pub status: ApprovalStatus,
    pub created_at: i64,
    #[serde(default)]
    pub decided_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    pub id: String,
    pub session_id: String,
    pub provider_id: String,
    pub model_name: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cost_estimate: Option<f64>,
    pub created_at: i64,
}

/// Tool definition shared with providers (JSON-schema parameters).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments object.
    pub parameters: serde_json::Value,
}

/// Provider-neutral streaming events emitted by adapters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamEvent {
    TextDelta(String),
    /// Deltas for a tool call, accumulated by `index` in the runtime.
    ToolCallDelta {
        index: usize,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        arguments_delta: String,
    },
    /// A complete tool call emitted at once (Gemini-style APIs).
    ToolCallComplete {
        index: usize,
        id: String,
        name: String,
        arguments: String,
    },
    Usage(TokenUsage),
    Done {
        #[serde(default)]
        stop_reason: Option<StopReason>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Stop,
    ToolCalls,
    MaxTokens,
    ContentFilter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub default_policy_mode: PolicyMode,
    #[serde(default = "default_editor_font")]
    pub editor_font_size: u32,
    #[serde(default = "default_terminal_font")]
    pub terminal_font_size: u32,
    /// Maximum agent tool-loop iterations per user turn.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    /// Retention for session history in days; `None` keeps history forever.
    #[serde(default)]
    pub retention_days: Option<u32>,
    #[serde(default)]
    pub show_hidden_files: bool,
    /// Where API keys are stored: "file" (0600 file in the app data dir, no
    /// OS password prompts — default) or "keychain" (OS credential vault;
    /// unsigned builds may prompt for the login password on first access).
    #[serde(default = "default_credential_storage")]
    pub credential_storage: String,
    /// Wrap long editor lines to the window width instead of scrolling
    /// horizontally. Default on.
    #[serde(default = "default_editor_word_wrap")]
    pub editor_word_wrap: bool,
}

fn default_theme() -> String {
    "dark".into()
}
fn default_editor_font() -> u32 {
    13
}
fn default_terminal_font() -> u32 {
    12
}
fn default_max_iterations() -> u32 {
    100
}
fn default_credential_storage() -> String {
    "file".into()
}
fn default_editor_word_wrap() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            default_policy_mode: PolicyMode::default(),
            editor_font_size: default_editor_font(),
            terminal_font_size: default_terminal_font(),
            max_iterations: default_max_iterations(),
            retention_days: None,
            show_hidden_files: false,
            credential_storage: default_credential_storage(),
            editor_word_wrap: default_editor_word_wrap(),
        }
    }
}

/// Result of a provider connection test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderTestResult {
    pub ok: bool,
    pub latency_ms: u64,
    #[serde(default)]
    pub model_count: Option<usize>,
    pub error: Option<String>,
}

/// File entry as shown in the explorer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    #[serde(default)]
    pub size: Option<u64>,
}

/// Search hit from the filesystem search tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub path: String,
    #[serde(default)]
    pub line_number: Option<u64>,
    pub line: String,
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every enum that crosses the IPC boundary must serialize to exactly
    /// what `as_str()` returns (and what the TypeScript types declare).
    /// Catches serde `rename_all` drift like `open_ai_compatible` vs
    /// `openai_compatible`.
    #[test]
    fn provider_kind_wire_format_matches_as_str() {
        for kind in [
            ProviderKind::OpenAiCompatible,
            ProviderKind::Anthropic,
            ProviderKind::Google,
            ProviderKind::Mock,
            ProviderKind::ExternalCli,
        ] {
            let wire = serde_json::to_string(&kind).unwrap();
            assert_eq!(wire, format!("\"{}\"", kind.as_str()));
            let parsed: ProviderKind = serde_json::from_str(&wire).unwrap();
            assert_eq!(parsed, kind);
            assert_eq!(ProviderKind::parse(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn provider_config_roundtrips_through_json() {
        let cfg = ProviderConfig {
            id: "p1".into(),
            name: "Custom Gateway".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: "https://gw.example/v1".into(),
            enabled: true,
            created_at: 1,
            updated_at: 2,
            extra: serde_json::json!({"api_version": 2}),
            credential_ref: None,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"openai_compatible\""), "got: {json}");
        let back: ProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, ProviderKind::OpenAiCompatible);

        // The exact payload shape the frontend sends for a custom provider.
        let frontend_json = r#"{
            "id": "p2", "name": "Custom", "kind": "openai_compatible",
            "base_url": "http://localhost:1234/v1", "enabled": true,
            "created_at": 0, "updated_at": 0, "extra": {}
        }"#;
        let from_ui: ProviderConfig = serde_json::from_str(frontend_json).unwrap();
        assert_eq!(from_ui.kind, ProviderKind::OpenAiCompatible);
    }

    #[test]
    fn multi_word_wire_variants_use_snake_case_matching_frontend() {
        // ToolRunStatus / ChatStatus multi-word variants vs the TS union types.
        assert_eq!(
            serde_json::to_string(&ToolRunStatus::AwaitingApproval).unwrap(),
            "\"awaiting_approval\""
        );
        assert_eq!(
            serde_json::to_string(&ToolRunStatus::AwaitingApproval).unwrap(),
            format!("\"{}\"", ToolRunStatus::AwaitingApproval.as_str())
        );
        assert_eq!(
            serde_json::to_string(&ChatStatus::RunningTool).unwrap(),
            "\"running_tool\""
        );
        assert_eq!(
            serde_json::to_string(&ChatStatus::WaitingApproval).unwrap(),
            "\"waiting_approval\""
        );
        assert_eq!(
            serde_json::to_string(&PolicyMode::Balanced).unwrap(),
            "\"balanced\""
        );
        assert_eq!(serde_json::to_string(&AgentMode::Custom).unwrap(), "\"custom\"");
    }
}
