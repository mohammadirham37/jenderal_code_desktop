//! Agent runtime: the tool-calling loop, permission orchestration,
//! approvals, cancellation, usage tracking, and event emission (PRD §8, §12).

pub mod context;
pub mod executor;

use anyhow::{bail, Context as _, Result};
use async_trait::async_trait;
use futures::{FutureExt, StreamExt};
use jcode_common::*;
use jcode_permissions::{Action, CheckOutcome, Grant, PolicyEngine};
use jcode_provider_anthropic::AnthropicProvider;
use jcode_provider_core::{AiProvider, CredentialStore, MockProvider};
use jcode_provider_google::GoogleProvider;
use jcode_provider_openai::OpenAiProvider;
use jcode_storage::Db;
use jcode_tool_filesystem::{CheckpointStore, Workspace};
use jcode_tool_terminal::CancelToken;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Prefix of the assistant note written when a turn pauses (step limit or a
/// stuck tool loop). The UI shows a "Continue" button after such a message.
pub const PAUSE_MARKER: &str = "⏸ **Paused.**";

/// Identical failing tool calls in a row before the turn is paused.
const REPEATED_FAILURE_LIMIT: u32 = 3;
use tokio::sync::oneshot;

pub use jcode_permissions::PolicyMode;

/// Runtime events forwarded to the UI layer.
#[async_trait]
pub trait EventSink: Send + Sync {
    async fn status(&self, session_id: &str, status: ChatStatus, detail: Option<String>);
    async fn delta(&self, session_id: &str, message_id: &str, delta: &str);
    /// Model reasoning streamed separately from the answer text.
    async fn reasoning_delta(&self, _session_id: &str, _message_id: &str, _delta: &str) {}
    async fn message(&self, session_id: &str, message: &MessageRecord);
    async fn tool_run(&self, session_id: &str, run: &ToolRunRecord);
    async fn approval(&self, session_id: &str, approval: &ApprovalRecord);
    async fn usage(&self, session_id: &str, usage: TokenUsage, cost_estimate: Option<f64>);
    async fn error(&self, session_id: &str, error: &str);
}

/// A no-op sink (used by tools and tests that don't care about events).
pub struct NoopSink;

#[async_trait]
impl EventSink for NoopSink {
    async fn status(&self, _: &str, _: ChatStatus, _: Option<String>) {}
    async fn delta(&self, _: &str, _: &str, _: &str) {}
    async fn message(&self, _: &str, _: &MessageRecord) {}
    async fn tool_run(&self, _: &str, _: &ToolRunRecord) {}
    async fn approval(&self, _: &str, _: &ApprovalRecord) {}
    async fn usage(&self, _: &str, _: TokenUsage, _: Option<f64>) {}
    async fn error(&self, _: &str, _: &str) {}
}

/// Create the adapter for a stored provider configuration.
pub fn create_provider(
    cfg: &ProviderConfig,
    creds: Arc<dyn CredentialStore>,
) -> Result<Arc<dyn AiProvider>> {
    match cfg.kind {
        ProviderKind::OpenAiCompatible => Ok(Arc::new(OpenAiProvider::new(
            cfg.id.clone(),
            cfg.name.clone(),
            cfg.base_url.clone(),
            creds,
            cfg.credential_ref.clone(),
        ))),
        ProviderKind::Anthropic => Ok(Arc::new(AnthropicProvider::new(
            cfg.id.clone(),
            cfg.name.clone(),
            cfg.base_url.clone(),
            creds,
            cfg.credential_ref.clone(),
        ))),
        ProviderKind::Google => Ok(Arc::new(GoogleProvider::new(
            cfg.id.clone(),
            cfg.name.clone(),
            cfg.base_url.clone(),
            creds,
            cfg.credential_ref.clone(),
        ))),
        ProviderKind::Mock => Ok(Arc::new(MockProvider::new(cfg.id.clone(), cfg.name.clone()))),
        ProviderKind::ExternalCli => bail!(
            "External CLI agents run through the agent-cli adapter, not the provider factory"
        ),
    }
}

/// Built-in agent profiles (PRD §8.2).
pub fn built_in_profiles() -> Vec<AgentProfile> {
    let now = now_ms();
    vec![
        AgentProfile {
            id: "plan".into(),
            name: "Plan".into(),
            mode: AgentMode::Plan,
            system_prompt: "You are a senior software architect in planning mode. \
Explore the codebase with the read-only tools, then produce a concrete implementation plan: \
goal summary, affected files, ordered steps, risks, and how to verify each step. \
Do not attempt to modify anything — write access is disabled."
                .into(),
            allowed_tools: Some(vec![
                "fs_read".into(), "fs_list".into(), "fs_search".into(), "glob_files".into(),
                "git_status".into(), "git_diff".into(), "git_log".into(),
            ]),
            policy_mode: PolicyMode::Safe,
            provider_id: None,
            model_id: None,
            built_in: true,
            created_at: now,
        },
        AgentProfile {
            id: "build".into(),
            name: "Build".into(),
            mode: AgentMode::Build,
            system_prompt: "You are an expert software engineer. Implement the requested change \
end-to-end: explore the relevant code, make focused edits, run the relevant tests or builds, \
and fix what breaks. Keep diffs minimal and consistent with the project's style. \
Never invent APIs that are not in the codebase — read the code first."
                .into(),
            allowed_tools: None,
            policy_mode: PolicyMode::Safe,
            provider_id: None,
            model_id: None,
            built_in: true,
            created_at: now,
        },
        AgentProfile {
            id: "review".into(),
            name: "Review".into(),
            mode: AgentMode::Review,
            system_prompt: "You are a meticulous code reviewer. Inspect the codebase or the \
described change for bugs, security issues, performance problems, readability, and missing tests. \
Write access is disabled — investigate only. Report findings as a prioritized list, each with \
`path:line`, severity, a one-paragraph explanation, and a concrete suggested fix."
                .into(),
            allowed_tools: Some(vec![
                "fs_read".into(), "fs_list".into(), "fs_search".into(), "glob_files".into(),
                "terminal_run".into(), "git_status".into(), "git_diff".into(), "git_log".into(),
            ]),
            policy_mode: PolicyMode::Safe,
            provider_id: None,
            model_id: None,
            built_in: true,
            created_at: now,
        },
    ]
}

/// Seed the mock provider, its model, and the built-in agent profiles.
/// Idempotent — called once at app startup.
pub async fn seed_defaults(db: &Db) -> Result<()> {
    for p in built_in_profiles() {
        if db.list_agent_profiles()?.iter().all(|e| e.id != p.id) {
            db.save_agent_profile(&p)?;
        }
    }
    if db.get_provider("mock")?.is_none() {
        let now = now_ms();
        db.save_provider(&ProviderConfig {
            id: "mock".into(),
            name: "Mock (demo)".into(),
            kind: ProviderKind::Mock,
            base_url: String::new(),
            enabled: true,
            created_at: now,
            updated_at: now,
            extra: json!({}),
            credential_ref: None,
        })?;
    }
    let mock = MockProvider::new("mock", "Mock (demo)");
    if let Ok(models) = mock.list_models().await {
        db.replace_discovered_models("mock", &models)?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
enum Decision {
    Approved { grant: Option<Grant> },
    Denied { reason: String },
}

struct SessionHandle {
    cancel: CancelToken,
    policy: Arc<Mutex<PolicyEngine>>,
    busy: bool,
}

/// A pending approval waiting for the user's decision.
struct PendingApproval {
    session_id: String,
    /// The action being approved, so a later policy change can re-check it
    /// (and only auto-resolve it when the new policy actually allows it).
    action: Action,
    tx: oneshot::Sender<Decision>,
}

/// Clears the session's `busy` flag when dropped — including when the turn
/// task panics — so a crashed turn never wedges the session.
struct BusyGuard {
    runtime: Arc<AgentRuntime>,
    session_id: String,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        let mut sessions = self
            .runtime
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(h) = sessions.get_mut(&self.session_id) {
            h.busy = false;
        }
    }
}

/// The agent runtime. Owns per-session state (cancel tokens, policies,
/// pending approvals) and spawns one task per user turn.
pub struct AgentRuntime {
    db: Arc<Db>,
    creds: Arc<dyn CredentialStore>,
    events: Arc<dyn EventSink>,
    checkpoint_base: PathBuf,
    sessions: Mutex<HashMap<String, SessionHandle>>,
    pending: Mutex<HashMap<String, PendingApproval>>,
}

impl AgentRuntime {
    pub fn new(
        db: Arc<Db>,
        creds: Arc<dyn CredentialStore>,
        events: Arc<dyn EventSink>,
        checkpoint_base: impl Into<PathBuf>,
    ) -> Self {
        Self {
            db,
            creds,
            events,
            checkpoint_base: checkpoint_base.into(),
            sessions: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Register (or refresh) the session's policy engine. Called when a
    /// session opens or the user changes its policy mode. Existing
    /// session grants are preserved across the swap.
    pub fn ensure_session_policy(&self, session_id: &str, workspace_root: &str, mode: PolicyMode) {
        let canonical = canonical_root(workspace_root);
        let mut sessions = self.sessions.lock().unwrap();
        let entry = sessions.entry(session_id.to_string()).or_insert_with(|| SessionHandle {
            cancel: CancelToken::new(),
            policy: Arc::new(Mutex::new(PolicyEngine::new(mode, &canonical))),
            busy: false,
        });
        let mut p = entry.policy.lock().unwrap();
        let grants: Vec<Grant> = p.grants().iter().copied().collect();
        let mut next = PolicyEngine::new(mode, canonical_root(workspace_root));
        for g in grants {
            next.grant(g);
        }
        *p = next;
    }

    /// Change the session's permission policy. Persists the choice so it
    /// survives restarts, and re-checks any approval the session is
    /// currently waiting on against the new policy: approvals whose action
    /// the new policy allows outright are resolved immediately; the rest
    /// (remote git, sensitive reads, sudo, ...) keep waiting for the user.
    pub async fn set_policy_mode(&self, session_id: &str, mode: PolicyMode) -> Result<()> {
        self.db.set_session_policy_mode(session_id, mode.as_str())?;

        let engine = {
            let sessions = self.sessions.lock().unwrap();
            sessions.get(session_id).map(|h| {
                let mut p = h.policy.lock().unwrap();
                let grants: Vec<Grant> = p.grants().iter().copied().collect();
                let mut next = PolicyEngine::new(mode, p.workspace_root());
                for g in grants {
                    next.grant(g);
                }
                *p = next.clone();
                next
            })
        };

        if let Some(engine) = engine {
            let targets: Vec<String> = {
                let pending = self.pending.lock().unwrap();
                pending
                    .iter()
                    .filter(|(_, p)| {
                        p.session_id == session_id
                            && matches!(engine.check(&p.action), CheckOutcome::Allowed)
                    })
                    .map(|(k, _)| k.clone())
                    .collect()
            };
            for approval_id in targets {
                if let Some(p) = self.pending.lock().unwrap().remove(&approval_id) {
                    let _ = p.tx.send(Decision::Approved { grant: None });
                }
            }
        }
        Ok(())
    }

    pub fn policy_mode(&self, session_id: &str) -> Option<PolicyMode> {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .get(session_id)
            .map(|h| h.policy.lock().unwrap().mode())
    }

    pub fn is_busy(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|h| h.busy)
            .unwrap_or(false)
    }

    /// Answer a pending approval request.
    pub async fn decide_approval(&self, approval_id: &str, approved: bool, grant: Option<Grant>) -> Result<()> {
        let pending = self.pending.lock().unwrap().remove(approval_id);
        match pending {
            Some(p) => {
                let decision = if approved {
                    Decision::Approved { grant }
                } else {
                    Decision::Denied {
                        reason: "denied by user".into(),
                    }
                };
                let _ = p.tx.send(decision);
                Ok(())
            }
            None => bail!("no pending approval with id {approval_id}"),
        }
    }

    /// Cancel the running turn of a session.
    pub async fn cancel(&self, session_id: &str) -> Result<()> {
        let handle_cancel = {
            let sessions = self.sessions.lock().unwrap();
            sessions.get(session_id).map(|h| h.cancel.clone())
        };
        match handle_cancel {
            Some(c) => {
                c.cancel();
                Ok(())
            }
            None => Ok(()), // nothing running
        }
    }

    /// Send a user message and run the agent turn in the background.
    /// Returns immediately; progress arrives via the EventSink.
    pub async fn send_message(
        self: &Arc<Self>,
        session_id: &str,
        content: &str,
        attachments: &[String],
    ) -> Result<()> {
        let session = self
            .db
            .get_session(session_id)?
            .ok_or_else(|| anyhow::anyhow!("session {session_id} not found"))?;
        let workspace = self
            .db
            .get_workspace(&session.workspace_id)?
            .ok_or_else(|| anyhow::anyhow!("workspace not found for session"))?;
        let profile = session
            .agent_profile_id
            .as_deref()
            .and_then(|pid| {
                self.db
                    .list_agent_profiles()
                    .ok()
                    .and_then(|mut l| l.drain(..).find(|p| p.id == pid))
            })
            .unwrap_or_else(default_build_profile);

        let provider_cfg = session
            .provider_id
            .as_deref()
            .and_then(|pid| self.db.get_provider(pid).ok().flatten())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no provider selected — pick a model for this session first (Provider Center)"
                )
            })?;
        if !provider_cfg.enabled {
            bail!("provider {} is disabled", provider_cfg.name);
        }
        let model_name = session
            .model_name
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no model selected for this session"))?;

        let ws = Workspace::open(&workspace.root_path)?;
        // Per-session override (e.g. full access chosen in the chat header)
        // wins over the profile default.
        let policy_mode = session.policy_mode.unwrap_or(profile.policy_mode);
        {
            let mut sessions = self.sessions.lock().unwrap();
            let canonical = ws
                .root()
                .to_string_lossy()
                .to_string();
            let entry = sessions
                .entry(session_id.to_string())
                .or_insert_with(|| SessionHandle {
                    cancel: CancelToken::new(),
                    policy: Arc::new(Mutex::new(PolicyEngine::new(
                        policy_mode,
                        &canonical,
                    ))),
                    busy: false,
                });
            // Keep an existing engine as-is: policy changes made at runtime
            // (ensure_session_policy / set_policy_mode / session grants)
            // must survive across turns.
            if entry.busy {
                bail!("a turn is already running for this session");
            }
            entry.cancel = CancelToken::new();
            entry.busy = true;
        }

        let runtime = self.clone();
        let session_id = session_id.to_string();
        let content = content.to_string();
        let attachments = attachments.to_vec();
        tokio::spawn(async move {
            let _busy = BusyGuard {
                runtime: runtime.clone(),
                session_id: session_id.clone(),
            };
            let turn = std::panic::AssertUnwindSafe(runtime.run_turn(
                &session_id,
                &content,
                &attachments,
                &profile,
                &provider_cfg,
                &model_name,
                &ws,
            ))
            .catch_unwind()
            .await;
            let failure = match turn {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(format!("{e:#}")),
                Err(_) => Some("internal error: the agent turn crashed".to_string()),
            };
            if let Some(msg) = failure {
                runtime.events.error(&session_id, &msg).await;
                runtime
                    .events
                    .status(&session_id, ChatStatus::Idle, None)
                    .await;
            }
        });
        Ok(())
    }

    /// Restore all checkpointed files of a session (undo agent edits).
    pub fn restore_checkpoints(&self, session_id: &str, workspace_root: &str) -> Result<usize> {
        let store = CheckpointStore::new(&self.checkpoint_base)?;
        store.restore_all(session_id, PathBuf::from(workspace_root).as_path())
    }

    pub fn list_checkpoints(&self, session_id: &str) -> Vec<jcode_tool_filesystem::CheckpointEntry> {
        CheckpointStore::new(&self.checkpoint_base)
            .map(|s| s.list(session_id))
            .unwrap_or_default()
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_turn(
        &self,
        session_id: &str,
        content: &str,
        attachments: &[String],
        profile: &AgentProfile,
        provider_cfg: &ProviderConfig,
        model_name: &str,
        ws: &Workspace,
    ) -> Result<()> {
        let sink = self.events.clone();
        let db = self.db.clone();
        let (cancel, policy) = {
            let sessions = self.sessions.lock().unwrap();
            let h = sessions.get(session_id);
            (
                h.map(|h| h.cancel.clone()).unwrap_or_default(),
                h.map(|h| h.policy.clone()).unwrap_or_else(|| {
                    Arc::new(Mutex::new(PolicyEngine::new(profile.policy_mode, ws.root())))
                }),
            )
        };

        // ------------------------------------------------------------------
        // Load history once per turn; new messages are appended in memory.
        // ------------------------------------------------------------------
        let prior = db.list_messages(session_id)?;
        let existing_count = prior.len();
        let mut history: Vec<ChatMessage> = prior
            .into_iter()
            .filter(|m| m.role != Role::System)
            .map(to_chat_message)
            .collect();

        // ------------------------------------------------------------------
        // Persist the user message (with packed attachments).
        // ------------------------------------------------------------------
        let packed = {
            let ws = ws.clone();
            let content = content.to_string();
            let attachments = attachments.to_vec();
            tokio::task::spawn_blocking(move || context::pack_attachments(&ws, &content, &attachments))
                .await
                .context("packing attachments")?
        };
        let user_msg = MessageRecord {
            id: new_id(),
            session_id: session_id.into(),
            role: Role::User,
            content: packed,
            tool_calls: vec![],
            tool_call_id: None,
            tool_name: None,
            reasoning: None,
            created_at: now_ms(),
        };
        db.append_message(&user_msg)?;
        sink.message(session_id, &user_msg).await;
        history.push(to_chat_message(user_msg));
        if existing_count == 0 {
            let title: String = content.trim().chars().take(60).collect();
            if !title.is_empty() {
                let _ = db.rename_session(session_id, &title);
            }
        }

        let provider = create_provider(provider_cfg, self.creds.clone())?;
        let read_only = matches!(profile.mode, AgentMode::Plan | AgentMode::Review);
        let mut tools = executor::tool_specs(read_only, profile.allowed_tools.as_deref());
        tools.retain(|t| executor::tool_rejection(profile, &t.name).is_none());
        let system = {
            let profile = profile.clone();
            let ws = ws.clone();
            tokio::task::spawn_blocking(move || context::build_system_prompt(&profile, &ws))
                .await
                .context("building system prompt")?
        };
        let checkpoints = CheckpointStore::new(&self.checkpoint_base).ok();
        let tool_ctx = executor::ToolContext::new(session_id, ws.clone(), checkpoints);
        let settings = db.get_settings().unwrap_or_default();
        let max_iterations = settings.max_iterations.max(1);

        // Model pricing for cost estimates + context window for the budget.
        let model_info = db.get_model(&format!("{}/{}", provider_cfg.id, model_name)).ok().flatten();
        let history_budget = context::history_budget(
            model_info.as_ref().and_then(|m| m.capabilities.context_window),
            model_info.as_ref().and_then(|m| m.capabilities.max_output_tokens),
            system.len(),
        );

        let mut iteration = 0u32;
        // Consecutive failures of the exact same tool call: a model that
        // keeps retrying an identical failing call is stuck, so pause the
        // turn instead of burning the remaining iterations.
        let mut repeated_failures: HashMap<String, u32> = HashMap::new();
        loop {
            iteration += 1;
            if iteration > max_iterations {
                self.pause_turn(
                    session_id,
                    &format!(
                        "I used all {max_iterations} steps allowed for one turn. \
                         Press **Continue** to keep going (the step limit can be raised in Settings → Agent)."
                    ),
                    &sink,
                    &db,
                    &mut history,
                )
                .await?;
                break;
            }

            let request = ModelRequest {
                model: model_name.to_string(),
                messages: context::prepare_history(&history, history_budget),
                tools: tools.clone(),
                system: Some(system.clone()),
                temperature: None,
                // Without it OpenAI-compatible endpoints apply their own
                // (often small) default, which cuts off large file writes.
                max_tokens: model_info.as_ref().and_then(|m| m.capabilities.max_output_tokens),
            };

            sink.status(session_id, ChatStatus::Thinking, None).await;
            let mut stream = match provider.stream(request).await {
                Ok(s) => s,
                Err(e) => {
                    sink.error(session_id, &format!("{e:#}")).await;
                    break;
                }
            };

            // --------------------------------------------------------------
            // Stream the response.
            // --------------------------------------------------------------
            sink.status(session_id, ChatStatus::Streaming, None).await;
            let assistant_id = new_id();
            let mut text = String::new();
            let mut reasoning = String::new();
            let mut stop_reason: Option<StopReason> = None;
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut usage = TokenUsage::default();
            let mut cancelled = false;

            loop {
                tokio::select! {
                    ev = stream.next() => {
                        match ev {
                            Some(StreamEvent::TextDelta(t)) => {
                                text.push_str(&t);
                                sink.delta(session_id, &assistant_id, &t).await;
                            }
                            Some(StreamEvent::ReasoningDelta(t)) => {
                                reasoning.push_str(&t);
                                sink.reasoning_delta(session_id, &assistant_id, &t).await;
                            }
                            Some(StreamEvent::ToolCallDelta { index, id, name, arguments_delta }) => {
                                while tool_calls.len() <= index {
                                    tool_calls.push(ToolCall { id: String::new(), name: String::new(), arguments: String::new() });
                                }
                                let tc = &mut tool_calls[index];
                                if let Some(id) = id { if !id.is_empty() { tc.id = id; } }
                                if let Some(n) = name { if !n.is_empty() { tc.name = n; } }
                                if !arguments_delta.is_empty() { tc.arguments.push_str(&arguments_delta); }
                            }
                            Some(StreamEvent::ToolCallComplete { index, id, name, arguments }) => {
                                while tool_calls.len() <= index {
                                    tool_calls.push(ToolCall { id: String::new(), name: String::new(), arguments: String::new() });
                                }
                                tool_calls[index] = ToolCall { id, name, arguments };
                            }
                            Some(StreamEvent::Usage(u)) => usage = u,
                            Some(StreamEvent::Done { stop_reason: r }) => {
                                stop_reason = r;
                                break;
                            }
                            None => break,
                        }
                    }
                    _ = cancel.cancelled() => {
                        cancelled = true;
                        break;
                    }
                }
            }
            drop(stream);

            for (i, tc) in tool_calls.iter_mut().enumerate() {
                if tc.id.is_empty() {
                    let short_id = &assistant_id[..8];
                    tc.id = format!("call-{short_id}-{i}");
                }
            }
            tool_calls.retain(|tc| !tc.name.is_empty());
            if cancelled {
                // Tool calls from an interrupted stream are incomplete and
                // will never get results: don't persist them (an assistant
                // tool call without a result makes the next request fail).
                tool_calls.clear();
            }

            // Persist the assistant message (skip a cancelled, empty one).
            if !(cancelled && text.is_empty()) {
                let assistant_msg = MessageRecord {
                    id: assistant_id.clone(),
                    session_id: session_id.into(),
                    role: Role::Assistant,
                    content: text.clone(),
                    tool_calls: tool_calls.clone(),
                    tool_call_id: None,
                    tool_name: None,
                    reasoning: (!reasoning.is_empty()).then(|| reasoning.clone()),
                    created_at: now_ms(),
                };
                db.append_message(&assistant_msg)?;
                sink.message(session_id, &assistant_msg).await;
                history.push(to_chat_message(assistant_msg));
            }

            // Usage + cost estimate.
            if usage.input_tokens > 0 || usage.output_tokens > 0 {
                let cost = estimate_cost(&model_info, usage);
                let recorded = db.insert_usage_record(&UsageRecord {
                    id: new_id(),
                    session_id: session_id.into(),
                    provider_id: provider_cfg.id.clone(),
                    model_name: model_name.to_string(),
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cost_estimate: cost,
                    created_at: now_ms(),
                });
                if recorded.is_ok() {
                    sink.usage(session_id, usage, cost).await;
                }
            }

            if cancelled {
                sink.status(session_id, ChatStatus::Idle, Some("cancelled".into()))
                    .await;
                return Ok(());
            }

            if tool_calls.is_empty() {
                break; // final answer
            }

            // --------------------------------------------------------------
            // Execute tool calls (with permission flow). Every tool call of
            // the persisted assistant message gets exactly one tool result —
            // also on cancellation and on errors — so the history stays
            // valid for providers that require call/result pairing.
            // --------------------------------------------------------------
            sink.status(session_id, ChatStatus::RunningTool, None).await;
            for call in &tool_calls {
                let outcome_content = if cancel.is_cancelled() {
                    "Cancelled by the user before this tool ran.".to_string()
                } else {
                    match self
                        .process_call(
                            session_id, call, profile, read_only, &tool_ctx, &policy, &cancel,
                            &sink, &db, &assistant_id, stop_reason == Some(StopReason::MaxTokens),
                        )
                        .await
                    {
                        Ok(c) => c,
                        Err(e) => format!("Error: {e:#}"),
                    }
                };

                let tool_msg = MessageRecord {
                    id: new_id(),
                    session_id: session_id.into(),
                    role: Role::Tool,
                    content: outcome_content,
                    tool_calls: vec![],
                    tool_call_id: Some(call.id.clone()),
                    tool_name: Some(call.name.clone()),
                    reasoning: None,
                    created_at: now_ms(),
                };
                let key = format!("{}\u{0}{}", call.name, call.arguments);
                if tool_msg.content.starts_with("Error:") {
                    *repeated_failures.entry(key).or_insert(0) += 1;
                } else {
                    repeated_failures.remove(&key);
                }
                db.append_message(&tool_msg)?;
                sink.message(session_id, &tool_msg).await;
                history.push(to_chat_message(tool_msg));
            }

            if let Some((key, _)) = repeated_failures.iter().find(|(_, n)| **n >= REPEATED_FAILURE_LIMIT) {
                let tool = key.split('\u{0}').next().unwrap_or("a tool");
                self.pause_turn(
                    session_id,
                    &format!(
                        "I stopped because the same `{tool}` call failed {REPEATED_FAILURE_LIMIT} times in a row. \
                         Check the last error above, then press **Continue** or tell me how to proceed."
                    ),
                    &sink,
                    &db,
                    &mut history,
                )
                .await?;
                break;
            }

            if cancel.is_cancelled() {
                sink.status(session_id, ChatStatus::Idle, Some("cancelled".into()))
                    .await;
                return Ok(());
            }
            // Loop: feed tool results back to the model.
        }

        let _ = db.touch_session(session_id);
        sink.status(session_id, ChatStatus::Idle, None).await;
        Ok(())
    }

    /// End the turn early with a visible assistant note (rather than an
    /// error) so the user can resume with "Continue". The note is persisted
    /// after the last tool results, which keeps the history valid.
    async fn pause_turn(
        &self,
        session_id: &str,
        reason: &str,
        sink: &Arc<dyn EventSink>,
        db: &Arc<Db>,
        history: &mut Vec<ChatMessage>,
    ) -> Result<()> {
        let msg = MessageRecord {
            id: new_id(),
            session_id: session_id.into(),
            role: Role::Assistant,
            content: format!("{PAUSE_MARKER} {reason}"),
            tool_calls: vec![],
            tool_call_id: None,
            tool_name: None,
            reasoning: None,
            created_at: now_ms(),
        };
        db.append_message(&msg)?;
        sink.message(session_id, &msg).await;
        history.push(to_chat_message(msg));
        Ok(())
    }

    /// Run one tool call through mode/allowlist checks, the permission
    /// engine and (if needed) the approval flow. Returns the tool-result
    /// content for the model.
    #[allow(clippy::too_many_arguments)]
    async fn process_call(
        &self,
        session_id: &str,
        call: &ToolCall,
        profile: &AgentProfile,
        read_only: bool,
        tool_ctx: &executor::ToolContext,
        policy: &Arc<Mutex<PolicyEngine>>,
        cancel: &CancelToken,
        sink: &Arc<dyn EventSink>,
        db: &Arc<Db>,
        assistant_id: &str,
        output_truncated: bool,
    ) -> Result<String> {
        // Some providers send "" for a call without arguments.
        let raw = if call.arguments.trim().is_empty() { "{}" } else { call.arguments.as_str() };
        let parsed = serde_json::from_str::<Value>(raw);
        let args = parsed
            .as_ref()
            .ok()
            .filter(|v| v.is_object())
            .cloned()
            .unwrap_or_else(|| json!({"_raw": call.arguments}));

        let run = ToolRunRecord {
            id: new_id(),
            session_id: session_id.into(),
            message_id: Some(assistant_id.to_string()),
            tool_name: call.name.clone(),
            status: ToolRunStatus::Pending,
            args: args.clone(),
            result_summary: None,
            error: None,
            approval_id: None,
            created_at: now_ms(),
            finished_at: None,
        };
        db.insert_tool_run(&run)?;
        sink.tool_run(session_id, &run).await;

        if args.get("_raw").is_some() {
            let msg = invalid_arguments_message(&call.name, parsed.err(), output_truncated);
            db.update_tool_run(&run.id, ToolRunStatus::Error, None, Some(&msg))?;
            emit_tool_update(sink, session_id, &run.id, ToolRunStatus::Error, db).await;
            return Ok(msg);
        }

        // Agent mode + profile allowlist are enforced again at execution
        // time (the model may call tools that were not offered to it).
        let rejection = executor::tool_rejection(profile, &call.name).or_else(|| {
            (read_only && !executor::is_read_only_tool(&call.name)).then(|| {
                format!(
                    "Tool `{}` is not available in {} mode (write access disabled).",
                    call.name,
                    profile.mode.as_str()
                )
            })
        });
        if let Some(msg) = rejection {
            db.update_tool_run(&run.id, ToolRunStatus::Error, None, Some(&msg))?;
            emit_tool_update(sink, session_id, &run.id, ToolRunStatus::Error, db).await;
            return Ok(msg);
        }

        let action = match executor::classify(&call.name, &args, tool_ctx) {
            Ok(a) => a,
            Err(e) => {
                let msg = format!("Could not classify tool call: {e:#}");
                db.update_tool_run(&run.id, ToolRunStatus::Error, None, Some(&msg))?;
                emit_tool_update(sink, session_id, &run.id, ToolRunStatus::Error, db).await;
                return Ok(msg);
            }
        };

        let check = policy.lock().unwrap().check(&action);
        match check {
            CheckOutcome::Allowed => Ok(self
                .run_tool(session_id, &call.name, &args, &run.id, tool_ctx, cancel, sink, db, assistant_id)
                .await),
            CheckOutcome::Denied(reason) => {
                let msg = format!("Blocked by permission policy: {reason}");
                db.update_tool_run(&run.id, ToolRunStatus::Denied, None, Some(&reason))?;
                emit_tool_update(sink, session_id, &run.id, ToolRunStatus::Denied, db).await;
                Ok(msg)
            }
            CheckOutcome::NeedsApproval(mut req) => {
                // Attach a concrete preview when we can compute one.
                if let Some(pv) = executor::preview(&call.name, &args, tool_ctx) {
                    req.preview = Some(pv);
                }
                let approval = ApprovalRecord {
                    id: new_id(),
                    session_id: session_id.into(),
                    tool_run_id: run.id.clone(),
                    kind: req.kind.clone(),
                    description: req.description.clone(),
                    preview: req.preview.clone(),
                    status: ApprovalStatus::Pending,
                    created_at: now_ms(),
                    decided_at: None,
                };
                db.update_tool_run(&run.id, ToolRunStatus::AwaitingApproval, None, None)?;
                emit_tool_update(sink, session_id, &run.id, ToolRunStatus::AwaitingApproval, db).await;
                db.insert_approval(&approval)?;

                // Register before announcing, so a fast decision always
                // finds the pending entry.
                let (tx, rx) = oneshot::channel::<Decision>();
                self.pending.lock().unwrap().insert(
                    approval.id.clone(),
                    PendingApproval {
                        session_id: session_id.to_string(),
                        action,
                        tx,
                    },
                );
                sink.approval(session_id, &approval).await;
                sink.status(session_id, ChatStatus::WaitingApproval, Some(req.description.clone()))
                    .await;

                let decision = tokio::select! {
                    d = rx => d.unwrap_or(Decision::Denied { reason: "approval channel closed".into() }),
                    _ = cancel.cancelled() => Decision::Denied { reason: "cancelled while awaiting approval".into() },
                };
                self.pending.lock().unwrap().remove(&approval.id);
                db.decide_approval(&approval.id, matches!(decision, Decision::Approved { .. }))?;

                match decision {
                    Decision::Approved { grant } => {
                        if let Some(g) = grant {
                            policy.lock().unwrap().grant(g);
                        }
                        Ok(self
                            .run_tool(session_id, &call.name, &args, &run.id, tool_ctx, cancel, sink, db, assistant_id)
                            .await)
                    }
                    Decision::Denied { reason } => {
                        let msg = format!("User denied this action ({reason}).");
                        db.update_tool_run(&run.id, ToolRunStatus::Denied, None, Some(&reason))?;
                        emit_tool_update(sink, session_id, &run.id, ToolRunStatus::Denied, db).await;
                        Ok(msg)
                    }
                }
            }
        }
    }

    /// Execute an approved/allowed tool call and persist its outcome.
    /// Returns the tool-result content for the model.
    #[allow(clippy::too_many_arguments)]
    async fn run_tool(
        &self,
        session_id: &str,
        name: &str,
        args: &Value,
        run_id: &str,
        tool_ctx: &executor::ToolContext,
        cancel: &CancelToken,
        sink: &Arc<dyn EventSink>,
        db: &Db,
        assistant_id: &str,
    ) -> String {
        let _ = self;
        db.update_tool_run(run_id, ToolRunStatus::Running, None, None)
            .ok();
        emit_tool_update(sink, session_id, run_id, ToolRunStatus::Running, db).await;
        match executor::execute(name, args, tool_ctx, cancel).await {
            Ok(outcome) => {
                db.update_tool_run(run_id, ToolRunStatus::Success, Some(&outcome.summary), None)
                    .ok();
                emit_tool_update(sink, session_id, run_id, ToolRunStatus::Success, db).await;
                let _ = assistant_id;
                outcome.result_for_model
            }
            Err(e) => {
                let msg = format!("Error: {e:#}");
                db.update_tool_run(run_id, ToolRunStatus::Error, None, Some(&msg))
                    .ok();
                emit_tool_update(sink, session_id, run_id, ToolRunStatus::Error, db).await;
                msg
            }
        }
    }
}

/// Canonicalize a workspace root so policy prefix checks match the
/// canonicalized paths produced by the filesystem layer (macOS aliases
/// /var -> /private/var, among others).
fn canonical_root(path: &str) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| PathBuf::from(path))
        .to_string_lossy()
        .to_string()
}

fn to_chat_message(m: MessageRecord) -> ChatMessage {
    ChatMessage {
        role: m.role,
        content: m.content,
        tool_calls: m.tool_calls,
        tool_call_id: m.tool_call_id,
        tool_name: m.tool_name,
        reasoning: m.reasoning,
    }
}

fn default_build_profile() -> AgentProfile {
    built_in_profiles()
        .into_iter()
        .find(|p| p.id == "build")
        .expect("build profile exists")
}

async fn emit_tool_update(
    sink: &Arc<dyn EventSink>,
    session_id: &str,
    run_id: &str,
    status: ToolRunStatus,
    db: &Db,
) {
    if let Ok(Some(run)) = db.get_tool_run(run_id) {
        let mut run = run;
        run.status = status;
        sink.tool_run(session_id, &run).await;
    }
}

/// Tool-result text for a call whose arguments are not a JSON object.
/// Output cut off by the token limit is the usual cause for large writes,
/// so say so instead of reporting a confusing "missing argument".
fn invalid_arguments_message(tool: &str, err: Option<serde_json::Error>, output_truncated: bool) -> String {
    let detail = err.map(|e| format!(" ({e})")).unwrap_or_default();
    let mut msg = format!("Error: the arguments for `{tool}` are not a valid JSON object{detail}.");
    if output_truncated {
        msg.push_str(
            " Your response hit the output token limit, so the arguments were cut off. \
             Split large content: write a smaller file first, then add the rest with further fs_edit or fs_write calls.",
        );
    }
    msg
}

fn estimate_cost(model: &Option<ModelInfo>, usage: TokenUsage) -> Option<f64> {
    let m = model.as_ref()?;
    let in_price = m.input_price_per_mtok?;
    let out_price = m.output_price_per_mtok.unwrap_or(0.0);
    Some(
        usage.input_tokens as f64 / 1_000_000.0 * in_price
            + usage.output_tokens as f64 / 1_000_000.0 * out_price,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct RecordingSink {
        idle: tokio::sync::Notify,
        saw_approval: std::sync::Mutex<Vec<ApprovalRecord>>,
        texts: std::sync::Mutex<Vec<String>>,
        done: AtomicBool,
    }

    impl RecordingSink {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                idle: tokio::sync::Notify::new(),
                saw_approval: std::sync::Mutex::new(Vec::new()),
                texts: std::sync::Mutex::new(Vec::new()),
                done: AtomicBool::new(false),
            })
        }
    }

    #[async_trait]
    impl EventSink for RecordingSink {
        async fn status(&self, _s: &str, status: ChatStatus, _d: Option<String>) {
            if matches!(status, ChatStatus::Idle) {
                self.done.store(true, Ordering::SeqCst);
                self.idle.notify_waiters();
            }
        }
        async fn delta(&self, _s: &str, _m: &str, delta: &str) {
            self.texts.lock().unwrap().push(delta.to_string());
        }
        async fn message(&self, _s: &str, _m: &MessageRecord) {}
        async fn tool_run(&self, _s: &str, _r: &ToolRunRecord) {}
        async fn approval(&self, _s: &str, a: &ApprovalRecord) {
            self.saw_approval.lock().unwrap().push(a.clone());
        }
        async fn usage(&self, _s: &str, _u: TokenUsage, _c: Option<f64>) {}
        async fn error(&self, _s: &str, e: &str) {
            panic!("unexpected runtime error: {e}");
        }
    }

    async fn setup() -> (Arc<AgentRuntime>, Arc<RecordingSink>, tempfile::TempDir, String) {
        let db = Arc::new(Db::open_memory().unwrap());
        seed_defaults(&db).await.unwrap();
        let sink = RecordingSink::new();
        let dir = tempfile::tempdir().unwrap();
        let ws = db.upsert_workspace(dir.path().to_str().unwrap(), "test-ws").unwrap();
        let session = db
            .create_session(&ws.id, "New session", Some("build"), Some("mock"), Some("mock-standard"))
            .unwrap();
        let ckpt = tempfile::tempdir().unwrap();
        let rt = Arc::new(AgentRuntime::new(
            db.clone(),
            Arc::new(jcode_provider_core::FileCredentialStore::new(ckpt.path().join("creds.json")).unwrap()),
            sink.clone(),
            ckpt.path().join("ck"),
        ));
        (rt, sink, dir, session.id)
    }

    async fn wait_idle(sink: &RecordingSink) {
        for _ in 0..200 {
            if sink.done.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("runtime did not reach idle in time");
    }

    #[tokio::test]
    async fn step_limit_pauses_with_a_note_instead_of_an_error() {
        let (rt, sink, dir, session_id) = setup().await;
        let mut settings = rt.db.get_settings().unwrap();
        settings.max_iterations = 1;
        rt.db.save_settings(&settings).unwrap();
        rt.ensure_session_policy(&session_id, dir.path().to_str().unwrap(), PolicyMode::Balanced);

        // The mock answers "demo" with a tool call, so a second step is
        // needed — which the limit of 1 forbids. (RecordingSink panics on
        // any error event.)
        rt.send_message(&session_id, "please demo create a file", &[]).await.unwrap();
        wait_idle(&sink).await;

        let messages = rt.db.list_messages(&session_id).unwrap();
        let last = messages.last().unwrap();
        assert_eq!(last.role, Role::Assistant);
        assert!(last.content.starts_with(PAUSE_MARKER), "{}", last.content);
        assert!(last.tool_calls.is_empty());
        // The tool call before the note got its result, so history stays valid.
        assert_eq!(messages[messages.len() - 2].role, Role::Tool);
    }

    #[tokio::test]
    async fn full_turn_with_tool_and_auto_approve_balanced() {
        let (rt, sink, dir, session_id) = setup().await;
        // Balanced policy auto-approves writes inside the workspace.
        rt.ensure_session_policy(&session_id, dir.path().to_str().unwrap(), PolicyMode::Balanced);

        rt.send_message(&session_id, "please demo create a file", &[])
            .await
            .unwrap();
        wait_idle(&sink).await;

        let target = dir.path().join("mock-demo.txt");
        assert!(target.exists(), "mock tool call should have written the file");
        let db = &rt.db;
        let messages = db.list_messages(&session_id).unwrap();
        assert!(messages.iter().any(|m| m.role == Role::User));
        assert!(messages.iter().any(|m| m.role == Role::Assistant));
        assert!(messages.iter().any(|m| m.role == Role::Tool));
        let runs = db.list_tool_runs(&session_id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, ToolRunStatus::Success);
        assert_eq!(runs[0].tool_name, "fs_write");
        // Session title derived from first message.
        let sess = db.get_session(&session_id).unwrap().unwrap();
        assert!(sess.title.contains("demo"));
    }

    #[tokio::test]
    async fn safe_mode_requires_approval_then_user_denies() {
        let (rt, sink, dir, session_id) = setup().await;
        rt.ensure_session_policy(&session_id, dir.path().to_str().unwrap(), PolicyMode::Safe);

        rt.send_message(&session_id, "please demo create a file", &[])
            .await
            .unwrap();

        // Wait for the approval request to arrive.
        let approval = loop {
            if let Some(a) = sink.saw_approval.lock().unwrap().first() {
                break a.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        };
        assert!(approval.description.contains("Write"));

        rt.decide_approval(&approval.id, false, None).await.unwrap();
        wait_idle(&sink).await;

        assert!(!dir.path().join("mock-demo.txt").exists());
        let runs = rt.db.list_tool_runs(&session_id).unwrap();
        assert_eq!(runs[0].status, ToolRunStatus::Denied);
        // The tool result message tells the model it was denied.
        let msgs = rt.db.list_messages(&session_id).unwrap();
        assert!(msgs
            .iter()
            .any(|m| m.role == Role::Tool && m.content.contains("denied")));
    }

    #[tokio::test]
    async fn switching_to_yolo_resolves_pending_approval_and_persists() {
        let (rt, sink, dir, session_id) = setup().await;
        rt.ensure_session_policy(&session_id, dir.path().to_str().unwrap(), PolicyMode::Safe);

        rt.send_message(&session_id, "please demo create a file", &[])
            .await
            .unwrap();
        let _approval = {
            let mut found: Option<ApprovalRecord> = None;
            for _ in 0..400 {
                if let Some(a) = sink.saw_approval.lock().unwrap().first() {
                    found = Some(a.clone());
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            found.expect("approval request never arrived within 10s")
        };

        // Switching to full access should auto-approve the pending request
        // and persist the choice.
        rt.set_policy_mode(&session_id, PolicyMode::Yolo).await.unwrap();
        wait_idle(&sink).await;

        assert!(dir.path().join("mock-demo.txt").exists());
        let runs = rt.db.list_tool_runs(&session_id).unwrap();
        assert_eq!(runs[0].status, ToolRunStatus::Success);
        // Persisted on the session row.
        let session = rt.db.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.policy_mode, Some(PolicyMode::Yolo));
        // And the approval row reflects the resolution.
        let approvals = rt.db.list_approvals(&session_id).unwrap();
        assert_eq!(approvals[0].status, ApprovalStatus::Approved);

        // A follow-up turn no longer asks for anything.
        sink.done.store(false, Ordering::SeqCst);
        sink.saw_approval.lock().unwrap().clear();
        rt.send_message(&session_id, "another demo task", &[])
            .await
            .unwrap();
        wait_idle(&sink).await;
        assert!(sink.saw_approval.lock().unwrap().is_empty(), "no further approvals under yolo");
    }

    #[tokio::test]
    async fn approval_with_session_grant_skips_next_ask() {
        let (rt, sink, dir, session_id) = setup().await;
        rt.ensure_session_policy(&session_id, dir.path().to_str().unwrap(), PolicyMode::Safe);

        rt.send_message(&session_id, "please demo create a file", &[])
            .await
            .unwrap();
        let approval = loop {
            if let Some(a) = sink.saw_approval.lock().unwrap().first() {
                break a.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        };
        rt.decide_approval(&approval.id, true, Some(Grant::WriteFiles))
            .await
            .unwrap();
        wait_idle(&sink).await;
        assert!(dir.path().join("mock-demo.txt").exists());
    }

    #[tokio::test]
    async fn cancellation_stops_turn() {
        let (rt, sink, _dir, session_id) = setup().await;
        rt.ensure_session_policy(&session_id, "/tmp", PolicyMode::Balanced);
        rt.send_message(&session_id, "hello there", &[]).await.unwrap();
        // Cancel quickly; mock streams in 25ms chunks.
        rt.cancel(&session_id).await.unwrap();
        wait_idle(&sink).await;
        // History was still persisted with the user message.
        let msgs = rt.db.list_messages(&session_id).unwrap();
        assert!(msgs.iter().any(|m| m.role == Role::User));
    }

    #[tokio::test]
    async fn policy_switch_only_auto_approves_actions_the_new_policy_allows() {
        let (rt, _sink, dir, session_id) = setup().await;
        let root = canonical_root(dir.path().to_str().unwrap());
        rt.ensure_session_policy(&session_id, &root, PolicyMode::Safe);

        let (tx_write, mut rx_write) = oneshot::channel();
        let (tx_push, mut rx_push) = oneshot::channel();
        let (tx_env, mut rx_env) = oneshot::channel();
        {
            let mut p = rt.pending.lock().unwrap();
            p.insert("w".into(), PendingApproval {
                session_id: session_id.clone(),
                action: Action::WriteFile { path: PathBuf::from(&root).join("a.txt") },
                tx: tx_write,
            });
            p.insert("g".into(), PendingApproval {
                session_id: session_id.clone(),
                action: Action::RunCommand { command: "git -C . push".into(), cwd: PathBuf::from(&root) },
                tx: tx_push,
            });
            p.insert("e".into(), PendingApproval {
                session_id: session_id.clone(),
                action: Action::ReadFile { path: PathBuf::from(&root).join(".env"), sensitive: true },
                tx: tx_env,
            });
        }
        // Balanced allows edits but still asks for git push and secrets.
        rt.set_policy_mode(&session_id, PolicyMode::Balanced).await.unwrap();
        assert!(matches!(rx_write.try_recv(), Ok(Decision::Approved { .. })));
        assert!(rx_push.try_recv().is_err(), "git push must keep waiting");
        assert!(rx_env.try_recv().is_err(), "sensitive read must keep waiting");
        {
            let p = rt.pending.lock().unwrap();
            assert!(p.contains_key("g") && p.contains_key("e") && !p.contains_key("w"));
        }

        // Full access never prompts, so everything still waiting goes through.
        rt.set_policy_mode(&session_id, PolicyMode::Yolo).await.unwrap();
        assert!(matches!(rx_push.try_recv(), Ok(Decision::Approved { .. })));
        assert!(matches!(rx_env.try_recv(), Ok(Decision::Approved { .. })));
        assert!(rt.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancel_during_approval_leaves_paired_history() {
        let (rt, sink, dir, session_id) = setup().await;
        rt.ensure_session_policy(&session_id, dir.path().to_str().unwrap(), PolicyMode::Safe);
        rt.send_message(&session_id, "please demo create a file", &[]).await.unwrap();
        for _ in 0..400 {
            if !sink.saw_approval.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        rt.cancel(&session_id).await.unwrap();
        wait_idle(&sink).await;
        for _ in 0..200 {
            if !rt.is_busy(&session_id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!rt.is_busy(&session_id));

        let msgs = rt.db.list_messages(&session_id).unwrap();
        for m in msgs.iter().filter(|m| m.role == Role::Assistant) {
            for c in &m.tool_calls {
                assert!(
                    msgs.iter().any(|t| t.role == Role::Tool && t.tool_call_id.as_deref() == Some(&c.id)),
                    "tool call {} has no result",
                    c.id
                );
            }
        }
    }

    #[test]
    fn invalid_arguments_explain_output_cut_off() {
        let err = serde_json::from_str::<Value>("{\"path\":\"a.txt\",\"content\":\"abc").err();
        let msg = invalid_arguments_message("fs_write", err, true);
        assert!(msg.starts_with("Error:"), "counts as a failure: {msg}");
        assert!(msg.contains("output token limit"));
        let msg = invalid_arguments_message("fs_write", None, false);
        assert!(!msg.contains("token limit"));
    }

    struct PanickingSink {
        errors: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl EventSink for PanickingSink {
        async fn status(&self, _s: &str, _st: ChatStatus, _d: Option<String>) {}
        async fn delta(&self, _s: &str, _m: &str, _d: &str) {}
        async fn message(&self, _s: &str, _m: &MessageRecord) {
            panic!("sink exploded");
        }
        async fn tool_run(&self, _s: &str, _r: &ToolRunRecord) {}
        async fn approval(&self, _s: &str, _a: &ApprovalRecord) {}
        async fn usage(&self, _s: &str, _u: TokenUsage, _c: Option<f64>) {}
        async fn error(&self, _s: &str, e: &str) {
            self.errors.lock().unwrap().push(e.to_string());
        }
    }

    #[tokio::test]
    async fn panicking_turn_clears_busy_flag() {
        let db = Arc::new(Db::open_memory().unwrap());
        seed_defaults(&db).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let ws = db.upsert_workspace(dir.path().to_str().unwrap(), "ws").unwrap();
        let session = db
            .create_session(&ws.id, "s", Some("build"), Some("mock"), Some("mock-standard"))
            .unwrap();
        let ckpt = tempfile::tempdir().unwrap();
        let sink = Arc::new(PanickingSink { errors: std::sync::Mutex::new(Vec::new()) });
        let rt = Arc::new(AgentRuntime::new(
            db.clone(),
            Arc::new(jcode_provider_core::FileCredentialStore::new(ckpt.path().join("c.json")).unwrap()),
            sink.clone(),
            ckpt.path().join("ck"),
        ));
        rt.send_message(&session.id, "hello", &[]).await.unwrap();
        for _ in 0..200 {
            if !rt.is_busy(&session.id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!rt.is_busy(&session.id), "busy must be cleared after a panic");
        assert!(sink.errors.lock().unwrap().iter().any(|e| e.contains("crashed")));
        // And the session accepts a new turn.
        rt.send_message(&session.id, "again", &[]).await.unwrap();
    }
}
