//! `task` tool: a read-only sub-agent with its own context. The main agent
//! hands it a research/exploration prompt; it reads and searches with the
//! session's model and returns only a final report, keeping the main
//! context small. Several `task` calls in one response run in parallel.

use crate::{executor, record_usage, retry, AgentRuntime, EventSink};
use anyhow::{anyhow, bail, Result};
use futures::StreamExt;
use jcode_common::*;
use jcode_permissions::{CheckOutcome, PolicyEngine};
use jcode_provider_core::AiProvider;
use jcode_storage::Db;
use jcode_tool_terminal::CancelToken;
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// Tools a sub-agent may use: reads only, nothing that needs approval in
/// the common case, and no further `task`.
pub const SUBAGENT_TOOLS: [&str; 7] = ["fs_read", "fs_list", "fs_search", "glob_files", "git_status", "git_diff", "git_log"];
const MAX_STEPS: usize = 30;

const SUBAGENT_PROMPT: &str = "

# Sub-agent
You are a sub-agent working for the main coding agent on one task. You only have read-only tools, and nobody \
can answer questions, so decide for yourself. Investigate efficiently: search first, read what matters, put \
independent tool calls in the same response. When done, reply with your final report only: a concise, \
well-structured answer to the task with `path:line` references and short code excerpts where useful. The main \
agent sees nothing but this report.";

/// What a turn's model calls need, passed down to tool handling.
pub(crate) struct TurnEnv<'a> {
    pub provider: &'a Arc<dyn AiProvider>,
    pub provider_cfg: &'a ProviderConfig,
    pub model_name: &'a str,
    pub model_info: &'a Option<ModelInfo>,
    pub system: &'a str,
    pub budget: usize,
}

impl AgentRuntime {
    /// Handle a `task` call end to end (tool-run record included); returns
    /// the tool result for the main agent.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_task(
        &self,
        session_id: &str,
        args: &Value,
        run_id: &str,
        env: &TurnEnv<'_>,
        tool_ctx: &executor::ToolContext,
        policy: &Arc<Mutex<PolicyEngine>>,
        cancel: &CancelToken,
        sink: &Arc<dyn EventSink>,
        db: &Arc<Db>,
    ) -> Result<String> {
        let description = args["description"].as_str().unwrap_or("Sub-agent task").trim().to_string();
        let prompt = args["prompt"].as_str().map(str::trim).filter(|p| !p.is_empty());
        let Some(prompt) = prompt else {
            let msg = "Error: `task` needs a non-empty `prompt`.".to_string();
            db.update_tool_run(run_id, ToolRunStatus::Error, None, Some(&msg))?;
            crate::emit_tool_update(sink, session_id, run_id, ToolRunStatus::Error, db).await;
            return Ok(msg);
        };
        db.update_tool_run(run_id, ToolRunStatus::Running, None, None)?;
        crate::emit_tool_update(sink, session_id, run_id, ToolRunStatus::Running, db).await;
        match self.run_subagent(session_id, prompt, env, tool_ctx, policy, cancel, db, sink).await {
            Ok((report, calls)) => {
                let summary = format!("{description} · {calls} tool call(s)");
                db.update_tool_run(run_id, ToolRunStatus::Success, Some(&summary), None)?;
                crate::emit_tool_update(sink, session_id, run_id, ToolRunStatus::Success, db).await;
                Ok(format!("Sub-agent report ({description}):\n{report}"))
            }
            Err(e) => {
                let msg = format!("Error: sub-agent failed: {e:#}");
                db.update_tool_run(run_id, ToolRunStatus::Error, None, Some(&msg))?;
                crate::emit_tool_update(sink, session_id, run_id, ToolRunStatus::Error, db).await;
                Ok(msg)
            }
        }
    }

    /// The sub-agent loop: `(final report, tool calls made)`.
    #[allow(clippy::too_many_arguments)]
    async fn run_subagent(
        &self,
        session_id: &str,
        prompt: &str,
        env: &TurnEnv<'_>,
        tool_ctx: &executor::ToolContext,
        policy: &Arc<Mutex<PolicyEngine>>,
        cancel: &CancelToken,
        db: &Arc<Db>,
        sink: &Arc<dyn EventSink>,
    ) -> Result<(String, usize)> {
        let allowed: Vec<String> = SUBAGENT_TOOLS.iter().map(|t| t.to_string()).collect();
        let tools = executor::tool_specs(true, Some(&allowed));
        // Own read tracking (its reads must not let the main agent edit
        // files it has not seen) and no language servers.
        let sub_ctx = executor::ToolContext::new(session_id, tool_ctx.workspace.clone(), None);
        let system = format!("{}{SUBAGENT_PROMPT}", env.system);
        let mut history = vec![ChatMessage::new(Role::User, prompt)];
        let mut calls_made = 0;
        let mut last_text = String::new();
        for step in 0..MAX_STEPS {
            let request = ModelRequest {
                model: env.model_name.to_string(),
                messages: crate::context::prepare_history(&history, env.budget),
                tools: tools.clone(),
                system: Some(system.clone()),
                temperature: None,
                max_tokens: env.model_info.as_ref().and_then(|m| m.capabilities.max_output_tokens),
            };
            // A connection that drops mid-response is retried like one that
            // fails to open (nothing has run yet).
            let mut dropped = 0;
            let reply = loop {
                let stream = retry::open_stream(env.provider.as_ref(), &request, cancel, &retry::RETRY_DELAYS, |_, _, _| async {})
                    .await?
                    .ok_or_else(|| anyhow!("cancelled"))?;
                let mut reply = collect(stream, cancel).await?;
                let Some(e) = reply.interrupted.take() else { break reply };
                let Some(delay) = retry::RETRY_DELAYS.get(dropped).copied() else {
                    bail!("the connection to the provider dropped during the response: {e}");
                };
                dropped += 1;
                if !retry::wait(delay, cancel).await {
                    bail!("cancelled");
                }
            };
            record_usage(db, sink, session_id, env.provider_cfg, env.model_name, env.model_info, reply.usage).await;
            let mut msg = ChatMessage::new(Role::Assistant, reply.text.clone());
            msg.tool_calls = reply.calls.clone();
            msg.reasoning = (!reply.reasoning.is_empty()).then_some(reply.reasoning);
            history.push(msg);
            if !reply.text.trim().is_empty() {
                last_text = reply.text.clone();
            }
            if reply.calls.is_empty() {
                return Ok((reply.text, calls_made));
            }
            calls_made += reply.calls.len();
            let results = futures::future::join_all(
                reply.calls.iter().map(|c| subagent_tool(c, &sub_ctx, policy, cancel)),
            )
            .await;
            for (call, content) in reply.calls.iter().zip(results) {
                let mut m = ChatMessage::new(Role::Tool, content);
                m.tool_call_id = Some(call.id.clone());
                m.tool_name = Some(call.name.clone());
                history.push(m);
            }
            if step + 1 == MAX_STEPS {
                break;
            }
        }
        Ok((
            format!("(The sub-agent stopped after {MAX_STEPS} steps without a final report.) Last notes:\n{last_text}"),
            calls_made,
        ))
    }
}

/// Run one sub-agent tool call; anything that would need the user's
/// approval is refused (sub-agents cannot ask).
async fn subagent_tool(
    call: &ToolCall,
    ctx: &executor::ToolContext,
    policy: &Arc<Mutex<PolicyEngine>>,
    cancel: &CancelToken,
) -> String {
    if !SUBAGENT_TOOLS.contains(&call.name.as_str()) {
        return format!("Error: tool `{}` is not available to sub-agents.", call.name);
    }
    let raw = if call.arguments.trim().is_empty() { "{}" } else { call.arguments.as_str() };
    let args: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => return format!("Error: arguments are not valid JSON ({e})."),
    };
    let action = match executor::classify(&call.name, &args, ctx) {
        Ok(a) => a,
        Err(e) => return format!("Error: {e:#}"),
    };
    let check = policy.lock().unwrap_or_else(|e| e.into_inner()).check(&action);
    match check {
        CheckOutcome::Allowed => match executor::execute(&call.name, &args, ctx, cancel).await {
            Ok(out) => out.result_for_model,
            Err(e) => format!("Error: {e:#}"),
        },
        CheckOutcome::NeedsApproval(req) => format!(
            "Not available: \"{}\" needs the user's approval, which sub-agents cannot ask for. Mention it in your report instead.",
            req.description
        ),
        CheckOutcome::Denied(reason) => format!("Blocked by permission policy: {reason}"),
    }
}

struct Reply {
    text: String,
    reasoning: String,
    calls: Vec<ToolCall>,
    usage: TokenUsage,
    /// The stream broke mid-response (`StreamEvent::Error`).
    interrupted: Option<String>,
}

async fn collect(mut stream: futures::stream::BoxStream<'static, StreamEvent>, cancel: &CancelToken) -> Result<Reply> {
    let mut reply = Reply {
        text: String::new(),
        reasoning: String::new(),
        calls: Vec::new(),
        usage: TokenUsage::default(),
        interrupted: None,
    };
    loop {
        let ev = tokio::select! {
            ev = stream.next() => ev,
            _ = cancel.cancelled() => bail!("cancelled"),
        };
        match ev {
            Some(StreamEvent::TextDelta(t)) => reply.text.push_str(&t),
            Some(StreamEvent::ReasoningDelta(t)) => reply.reasoning.push_str(&t),
            Some(StreamEvent::ToolCallDelta { index, id, name, arguments_delta }) => {
                while reply.calls.len() <= index {
                    reply.calls.push(ToolCall { id: String::new(), name: String::new(), arguments: String::new() });
                }
                let tc = &mut reply.calls[index];
                if let Some(id) = id.filter(|i| !i.is_empty()) {
                    tc.id = id;
                }
                if let Some(n) = name.filter(|n| !n.is_empty()) {
                    tc.name = n;
                }
                tc.arguments.push_str(&arguments_delta);
            }
            Some(StreamEvent::ToolCallComplete { index, id, name, arguments }) => {
                while reply.calls.len() <= index {
                    reply.calls.push(ToolCall { id: String::new(), name: String::new(), arguments: String::new() });
                }
                reply.calls[index] = ToolCall { id, name, arguments };
            }
            Some(StreamEvent::Usage(u)) => reply.usage = u,
            Some(StreamEvent::Error(e)) => {
                reply.interrupted = Some(e);
                break;
            }
            Some(StreamEvent::Done { .. }) | None => break,
        }
    }
    reply.calls.retain(|c| !c.name.is_empty());
    for (i, c) in reply.calls.iter_mut().enumerate() {
        if c.id.is_empty() {
            c.id = format!("sub-{i}-{}", new_id());
        }
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoopSink;
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use serde_json::json;

    /// Replies with the scripted event lists in order and records requests.
    struct Scripted {
        replies: Mutex<Vec<Vec<StreamEvent>>>,
        requests: Mutex<Vec<ModelRequest>>,
    }

    #[async_trait]
    impl AiProvider for Scripted {
        fn id(&self) -> &str {
            "scripted"
        }
        fn name(&self) -> &str {
            "Scripted"
        }
        async fn list_models(&self) -> Result<Vec<ModelInfo>> {
            Ok(vec![])
        }
        async fn stream(&self, request: ModelRequest) -> Result<BoxStream<'static, StreamEvent>> {
            self.requests.lock().unwrap().push(request);
            let next = self.replies.lock().unwrap().remove(0);
            Ok(futures::stream::iter(next).boxed())
        }
    }

    fn call(id: &str, name: &str, args: Value) -> StreamEvent {
        StreamEvent::ToolCallComplete { index: 0, id: id.into(), name: name.into(), arguments: args.to_string() }
    }

    #[tokio::test]
    async fn subagent_researches_with_read_only_tools_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        let ws = jcode_tool_filesystem::Workspace::open(dir.path()).unwrap();
        ws.write_file("src/auth.rs", "fn login() {}\n").unwrap();
        ws.write_file(".env", "SECRET=1\n").unwrap();

        let provider = Arc::new(Scripted {
            replies: Mutex::new(vec![
                vec![call("c1", "fs_search", json!({"query": "fn login"})), StreamEvent::Done { stop_reason: None }],
                vec![call("c2", "fs_read", json!({"path": ".env"})), StreamEvent::Done { stop_reason: None }],
                vec![call("c3", "fs_write", json!({"path": "x", "content": "y"})), StreamEvent::Done { stop_reason: None }],
                vec![StreamEvent::TextDelta("login is in src/auth.rs:1".into()), StreamEvent::Done { stop_reason: None }],
            ]),
            requests: Mutex::new(vec![]),
        });
        let provider_dyn: Arc<dyn AiProvider> = provider.clone();
        let db = Arc::new(Db::open_memory().unwrap());
        let sink: Arc<dyn EventSink> = Arc::new(NoopSink);
        let rt = AgentRuntime::new(
            db.clone(),
            Arc::new(jcode_provider_core::FileCredentialStore::new(dir.path().join("c.json")).unwrap()),
            sink.clone(),
            dir.path().join("ck"),
        );
        let cfg = ProviderConfig {
            id: "scripted".into(),
            name: "Scripted".into(),
            kind: ProviderKind::Mock,
            base_url: String::new(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
            extra: json!({}),
            credential_ref: None,
        };
        let env = TurnEnv {
            provider: &provider_dyn,
            provider_cfg: &cfg,
            model_name: "m",
            model_info: &None,
            system: "SYSTEM",
            budget: 0,
        };
        let ctx = executor::ToolContext::new("s", ws.clone(), None);
        let policy = Arc::new(Mutex::new(PolicyEngine::new(jcode_common::PolicyMode::Safe, ws.root())));
        let (report, calls) = rt
            .run_subagent("s", "Where is login?", &env, &ctx, &policy, &CancelToken::new(), &db, &sink)
            .await
            .unwrap();
        assert_eq!(report, "login is in src/auth.rs:1");
        assert_eq!(calls, 3);

        let requests = provider.requests.lock().unwrap();
        assert!(requests[0].system.as_deref().unwrap().contains("# Sub-agent"));
        let names: Vec<&str> = requests[0].tools.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.contains(&"task") && !names.contains(&"fs_write") && names.contains(&"fs_read"));
        let tool_results: Vec<&str> = requests[3]
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.content.as_str())
            .collect();
        assert!(tool_results[0].contains("src/auth.rs:1"));
        assert!(tool_results[1].contains("needs the user's approval"), "sensitive read refused: {}", tool_results[1]);
        assert!(tool_results[2].contains("not available to sub-agents"));
        assert!(!dir.path().join("x").exists());
    }

    /// A connection that drops mid-response is asked again; the cut-off
    /// text is not part of the report.
    #[tokio::test]
    async fn subagent_retries_a_dropped_stream() {
        let dir = tempfile::tempdir().unwrap();
        let ws = jcode_tool_filesystem::Workspace::open(dir.path()).unwrap();
        let provider = Arc::new(Scripted {
            replies: Mutex::new(vec![
                vec![StreamEvent::TextDelta("half an ans".into()), StreamEvent::Error("error decoding response body".into())],
                vec![StreamEvent::TextDelta("full answer".into()), StreamEvent::Done { stop_reason: None }],
            ]),
            requests: Mutex::new(vec![]),
        });
        let provider_dyn: Arc<dyn AiProvider> = provider.clone();
        let db = Arc::new(Db::open_memory().unwrap());
        let sink: Arc<dyn EventSink> = Arc::new(NoopSink);
        let rt = AgentRuntime::new(
            db.clone(),
            Arc::new(jcode_provider_core::FileCredentialStore::new(dir.path().join("c.json")).unwrap()),
            sink.clone(),
            dir.path().join("ck"),
        );
        let cfg = ProviderConfig {
            id: "scripted".into(),
            name: "Scripted".into(),
            kind: ProviderKind::Mock,
            base_url: String::new(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
            extra: json!({}),
            credential_ref: None,
        };
        let env = TurnEnv { provider: &provider_dyn, provider_cfg: &cfg, model_name: "m", model_info: &None, system: "S", budget: 0 };
        let ctx = executor::ToolContext::new("s", ws.clone(), None);
        let policy = Arc::new(Mutex::new(PolicyEngine::new(jcode_common::PolicyMode::Safe, ws.root())));
        let (report, _) = rt
            .run_subagent("s", "q", &env, &ctx, &policy, &CancelToken::new(), &db, &sink)
            .await
            .unwrap();
        assert_eq!(report, "full answer");
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].messages.len(), requests[1].messages.len(), "same history on retry");
    }
}
