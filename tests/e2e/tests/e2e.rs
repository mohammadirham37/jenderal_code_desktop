//! End-to-end test: the full desktop stack without a UI.
//!
//! Covers the PRD acceptance path: workspace → session → streaming agent
//! turn with a tool call → permission approval → file written → checkpoint
//! restore → usage recorded → history persisted in SQLite on disk.

use async_trait::async_trait;
use jcode_agent_runtime::{seed_defaults, AgentRuntime, EventSink};
use jcode_common::*;
use jcode_storage::Db;
use jcode_provider_core::FileCredentialStore;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct E2ESink {
    approvals: Mutex<Vec<ApprovalRecord>>,
    tool_events: AtomicUsize,
    idle: AtomicBool,
}

#[async_trait]
impl EventSink for E2ESink {
    async fn status(&self, _s: &str, status: ChatStatus, _d: Option<String>) {
        if matches!(status, ChatStatus::Idle) {
            self.idle.store(true, Ordering::SeqCst);
        }
    }
    async fn delta(&self, _s: &str, _m: &str, _d: &str) {}
    async fn message(&self, _s: &str, _m: &MessageRecord) {}
    async fn tool_run(&self, _s: &str, _r: &ToolRunRecord) {
        self.tool_events.fetch_add(1, Ordering::SeqCst);
    }
    async fn approval(&self, _s: &str, a: &ApprovalRecord) {
        self.approvals.lock().unwrap().push(a.clone());
    }
    async fn usage(&self, _s: &str, _u: TokenUsage, _c: Option<f64>) {}
    async fn error(&self, _s: &str, e: &str) {
        panic!("unexpected runtime error: {e}");
    }
}

#[tokio::test]
async fn full_stack_user_journey() -> anyhow::Result<()> {
    // --- On-disk application data (like the desktop app's data dir). ---
    let data = tempfile::tempdir()?;
    let project = tempfile::tempdir()?;
    let workspace_root = project.path();

    // A tiny project with instructions and a git repo.
    std::fs::write(workspace_root.join("README.md"), "# demo project\n")?;
    std::fs::write(workspace_root.join("AGENTS.md"), "Always prefer minimal diffs.\n")?;
    assert!(jcode_tool_git::GitRepo::detect(workspace_root).is_none());
    init_git(workspace_root)?;

    let db = Arc::new(Db::open(data.path().join("jenderalcode.db"))?);
    seed_defaults(&db).await?;

    let sink = Arc::new(E2ESink {
        approvals: Mutex::new(Vec::new()),
        tool_events: AtomicUsize::new(0),
        idle: AtomicBool::new(false),
    });
    let runtime = Arc::new(AgentRuntime::new(
        db.clone(),
        Arc::new(FileCredentialStore::new(data.path().join("credentials.json"))?),
        sink.clone(),
        data.path().join("checkpoints"),
    ));

    // --- Open a workspace (persists + detects git). ---
    let ws = db.upsert_workspace(workspace_root.to_str().unwrap(), "demo")?;
    assert!(ws.is_git_repo);

    // --- Start a session on the mock provider with the Build profile. ---
    let session = db.create_session(&ws.id, "New session", Some("build"), Some("mock"), Some("mock-standard"))?;
    runtime.ensure_session_policy(&session.id, &ws.root_path, PolicyMode::Safe);

    // --- Ask for a demo file; Safe policy must ask for approval. ---
    runtime
        .send_message(&session.id, "please demo create a file", &[], vec![])
        .await?;

    let approval = wait_for(|| sink.approvals.lock().unwrap().first().cloned(), 10_000)
        .await
        .expect("approval requested");
    assert!(approval.description.contains("Write"), "got: {}", approval.description);
    assert!(approval.preview.as_deref().unwrap_or("").contains("+Hello from the JenderalCode mock provider"));

    runtime
        .decide_approval(&approval.id, true, Some(jcode_permissions::Grant::WriteFiles))
        .await?;
    wait_for(|| sink.idle.load(Ordering::SeqCst).then_some(true), 10_000)
        .await
        .expect("turn reached idle");

    // --- The file went through the real tool pipeline. ---
    let written = workspace_root.join("mock-demo.txt");
    assert!(written.exists(), "agent tool wrote the file");
    let content = std::fs::read_to_string(&written)?;
    assert!(content.contains("mock provider"));

    // --- Tool runs + messages persisted; usage recorded; title derived. ---
    let runs = db.list_tool_runs(&session.id)?;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, ToolRunStatus::Success);
    let messages = db.list_messages(&session.id)?;
    assert!(messages.iter().any(|m| m.role == Role::User));
    assert!(messages.iter().any(|m| m.role == Role::Assistant && m.tool_calls.len() == 1));
    assert!(messages.iter().any(|m| m.role == Role::Tool));
    let sessions = db.list_sessions(Some(&ws.id))?;
    assert!(sessions[0].title.contains("demo"));

    // --- Usage dashboard shows the mock's reported tokens. ---
    let summary = db.usage_summary()?;
    assert!(summary.total_input + summary.total_output > 0);

    // --- Checkpoints allow a one-call rollback of agent edits. ---
    let checkpoints = runtime.list_checkpoints(&session.id);
    assert_eq!(checkpoints.len(), 1);
    let restored = runtime.restore_checkpoints(&session.id, &ws.root_path)?;
    assert_eq!(restored, 1);
    assert!(!written.exists(), "created file removed by restore");

    // --- Session survives "app restart" (fresh read from disk). ---
    let detail_messages = db.list_messages(&session.id)?;
    assert!(!detail_messages.is_empty());

    Ok(())
}

/// Balanced policy: file edits are applied without asking, but terminal
/// commands still need approval; history stays provider-valid (every
/// assistant tool call has a result) across turns; checkpoint keys are
/// normalized workspace-relative paths.
#[tokio::test]
async fn balanced_policy_edits_auto_commands_ask() -> anyhow::Result<()> {
    use jcode_permissions::{Action, CheckOutcome, PolicyEngine};

    let data = tempfile::tempdir()?;
    let project = tempfile::tempdir()?;
    let db = Arc::new(Db::open(data.path().join("jenderalcode.db"))?);
    seed_defaults(&db).await?;
    let sink = Arc::new(E2ESink {
        approvals: Mutex::new(Vec::new()),
        tool_events: AtomicUsize::new(0),
        idle: AtomicBool::new(false),
    });
    let runtime = Arc::new(AgentRuntime::new(
        db.clone(),
        Arc::new(FileCredentialStore::new(data.path().join("credentials.json"))?),
        sink.clone(),
        data.path().join("checkpoints"),
    ));
    let ws = db.upsert_workspace(project.path().to_str().unwrap(), "balanced")?;
    let session = db.create_session(&ws.id, "New session", Some("build"), Some("mock"), Some("mock-standard"))?;
    runtime.ensure_session_policy(&session.id, &ws.root_path, PolicyMode::Balanced);

    for prompt in ["please demo create a file", "demo again please"] {
        sink.idle.store(false, Ordering::SeqCst);
        runtime.send_message(&session.id, prompt, &[], vec![]).await?;
        wait_for(|| sink.idle.load(Ordering::SeqCst).then_some(true), 10_000)
            .await
            .expect("turn reached idle");
        wait_for(|| (!runtime.is_busy(&session.id)).then_some(true), 5_000)
            .await
            .expect("busy cleared");
    }
    assert!(sink.approvals.lock().unwrap().is_empty(), "edits auto-applied in balanced");
    assert!(project.path().join("mock-demo.txt").exists());

    // Every assistant tool call has exactly one result directly after it.
    let messages = db.list_messages(&session.id)?;
    for (i, m) in messages.iter().enumerate().filter(|(_, m)| m.role == Role::Assistant) {
        let results: Vec<_> = messages[i + 1..]
            .iter()
            .take_while(|t| t.role == Role::Tool)
            .collect();
        assert_eq!(results.len(), m.tool_calls.len());
        for c in &m.tool_calls {
            let n = results
                .iter()
                .filter(|t| t.tool_call_id.as_deref() == Some(c.id.as_str()))
                .count();
            assert_eq!(n, 1, "tool call {} results", c.id);
        }
    }

    // One normalized checkpoint for the file despite two writes.
    let cps = runtime.list_checkpoints(&session.id);
    assert_eq!(cps.len(), 1);
    assert_eq!(cps[0].rel_path, "mock-demo.txt");

    // Commands still ask in balanced; full access (yolo) never prompts, but the
    // catastrophic deny-list still holds.
    let root = std::fs::canonicalize(project.path())?;
    let balanced = PolicyEngine::new(PolicyMode::Balanced, &root);
    let cmd = |c: &str| Action::RunCommand { command: c.into(), cwd: root.clone() };
    assert!(matches!(balanced.check(&cmd("cargo test")), CheckOutcome::NeedsApproval(_)));
    let yolo = PolicyEngine::new(PolicyMode::Yolo, &root);
    assert!(matches!(yolo.check(&cmd("cargo test")), CheckOutcome::Allowed));
    assert!(matches!(yolo.check(&cmd("true && git -C . push")), CheckOutcome::Allowed));
    assert!(matches!(yolo.check(&cmd("rm -fr ~")), CheckOutcome::Denied(_)));
    Ok(())
}

fn init_git(root: &std::path::Path) -> anyhow::Result<()> {
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "e2e@jenderalcode.dev"],
        vec!["config", "user.name", "E2E"],
    ] {
        let ok = std::process::Command::new("git")
            .args(&args)
            .current_dir(root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            anyhow::bail!("git {args:?} failed");
        }
    }
    Ok(())
}

/// Async polling helper — must yield (tokio sleep) so spawned tasks on the
/// current-thread runtime can progress between checks.
async fn wait_for<T>(mut f: impl FnMut() -> Option<T>, timeout_ms: u64) -> Option<T> {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
