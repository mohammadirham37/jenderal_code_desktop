//! All Tauri commands exposed to the frontend.

use crate::state::AppState;
use jcode_agent_cli::{run_cli_agent, CliAgentConfig};
use jcode_provider_core::CredentialStore as _;
use jcode_agent_runtime::create_provider;
use jcode_common::*;
use jcode_permissions::Grant;
use jcode_storage::Db;
use jcode_tool_filesystem::{SearchQuery, Workspace};
use jcode_tool_git::GitRepo;
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, State};

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    format!("{e:#}")
}

/// Run blocking work (filesystem, git, SQLite, process spawning) on the
/// blocking pool. Synchronous Tauri commands run on the main thread and
/// would freeze the UI.
async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CmdResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("background task failed: {e}"))?
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Reject workspace roots that would expose far more than a project: the
/// filesystem root, the home directory itself, and OS system directories.
fn validate_workspace_root(path: &Path) -> CmdResult<()> {
    let canon = std::fs::canonicalize(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    if canon.parent().is_none() {
        return Err("refusing to open the filesystem root as a workspace; choose a project folder".into());
    }
    if let Some(home) = home_dir() {
        let home = std::fs::canonicalize(&home).unwrap_or(home);
        if canon == home {
            return Err("refusing to open your home directory as a workspace; choose a project folder".into());
        }
    }
    let lower = canon.to_string_lossy().to_lowercase().replace('\\', "/");
    const UNIX_SYSTEM: &[&str] = &[
        "/system", "/usr", "/bin", "/sbin", "/etc", "/private/etc", "/dev", "/proc", "/boot",
    ];
    let is_system = if cfg!(windows) {
        // Strip the verbatim prefix canonicalize adds (\\?\C:\...).
        let l = lower.trim_start_matches("//?/");
        let rest = l.get(2..).unwrap_or("");
        l.get(1..2) == Some(":")
            && ["/windows", "/program files", "/program files (x86)", "/programdata"]
                .iter()
                .any(|d| rest == *d || rest.starts_with(&format!("{d}/")))
    } else {
        UNIX_SYSTEM
            .iter()
            .any(|d| lower == *d || lower.starts_with(&format!("{d}/")))
    };
    if is_system {
        return Err(format!(
            "refusing to open system directory {} as a workspace",
            canon.display()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Workspaces
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_workspaces(state: State<'_, AppState>) -> CmdResult<Vec<WorkspaceInfo>> {
    state.db.list_workspaces().map_err(err)
}

#[tauri::command]
pub async fn open_workspace(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> CmdResult<WorkspaceInfo> {
    let p = PathBuf::from(&path);
    if !p.is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    validate_workspace_root(&p)?;
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());
    let info = state.db.upsert_workspace(&path, &name).map_err(err)?;
    *state.active_workspace.write().unwrap() = Some(info.clone());

    // (Re)start the file watcher off the main thread (recursive watch
    // registration can be slow on large trees).
    let app2 = app.clone();
    let root = PathBuf::from(&path);
    blocking(move || {
        use tauri::Manager;
        let watcher = crate::watcher::start(app2.clone(), root).ok();
        let store = app2.state::<crate::WatcherStore>();
        *store.0.lock().unwrap_or_else(|e| e.into_inner()) = watcher;
        Ok(())
    })
    .await?;
    Ok(info)
}

#[tauri::command]
pub fn get_active_workspace(state: State<'_, AppState>) -> CmdResult<Option<WorkspaceInfo>> {
    Ok(state.active_workspace.read().unwrap().clone())
}

#[tauri::command]
pub fn close_workspace(state: State<'_, AppState>) -> CmdResult<()> {
    *state.active_workspace.write().unwrap() = None;
    Ok(())
}

#[tauri::command]
pub fn remove_workspace(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.db.delete_workspace(&id).map_err(err)
}

// ---------------------------------------------------------------------------
// Filesystem (UI operations; agent tools go through the runtime)
// ---------------------------------------------------------------------------

/// Run `f` against the active workspace on the blocking pool.
async fn with_workspace<T, F>(state: &State<'_, AppState>, f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce(Workspace) -> CmdResult<T> + Send + 'static,
{
    let root = state.active_root()?;
    blocking(move || f(Workspace::open(&root).map_err(err)?)).await
}

#[tauri::command]
pub async fn fs_list(
    state: State<'_, AppState>,
    path: Option<String>,
    show_hidden: Option<bool>,
) -> CmdResult<Vec<FileEntry>> {
    with_workspace(&state, move |ws| {
        ws.list_dir(path.as_deref().unwrap_or(""), show_hidden.unwrap_or(false))
            .map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn fs_read(state: State<'_, AppState>, path: String) -> CmdResult<ReadFileResult> {
    with_workspace(&state, move |ws| {
        let content = ws.read_file(&path).map_err(err)?;
        Ok(ReadFileResult { path, content })
    })
    .await
}

#[derive(Serialize)]
pub struct ReadFileResult {
    pub path: String,
    pub content: String,
}

#[tauri::command]
pub async fn fs_write(state: State<'_, AppState>, path: String, content: String) -> CmdResult<()> {
    with_workspace(&state, move |ws| ws.write_file(&path, &content).map(|_| ()).map_err(err)).await
}

#[tauri::command]
pub async fn fs_create(
    state: State<'_, AppState>,
    path: String,
    is_dir: bool,
) -> CmdResult<()> {
    with_workspace(&state, move |ws| {
        let abs = ws.resolve(&path).map_err(err)?;
        if is_dir {
            std::fs::create_dir_all(&abs).map_err(err)?;
        } else {
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent).map_err(err)?;
            }
            std::fs::write(&abs, "").map_err(err)?;
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn fs_delete(state: State<'_, AppState>, path: String) -> CmdResult<()> {
    with_workspace(&state, move |ws| ws.delete_path(&path).map(|_| ()).map_err(err)).await
}

#[tauri::command]
pub async fn fs_rename(state: State<'_, AppState>, from: String, to: String) -> CmdResult<()> {
    with_workspace(&state, move |ws| ws.rename(&from, &to).map(|_| ()).map_err(err)).await
}

#[tauri::command]
pub async fn fs_search(
    state: State<'_, AppState>,
    query: String,
    is_regex: Option<bool>,
    include_glob: Option<String>,
) -> CmdResult<Vec<SearchHit>> {
    with_workspace(&state, move |ws| {
        ws.search(&SearchQuery {
            query,
            is_regex: is_regex.unwrap_or(false),
            include_glob,
        })
        .map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn fs_glob(state: State<'_, AppState>, pattern: String) -> CmdResult<Vec<String>> {
    with_workspace(&state, move |ws| ws.glob_files(&pattern, 500).map_err(err)).await
}

// ---------------------------------------------------------------------------
// Credential storage
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_credential_backend(state: State<'_, AppState>) -> CmdResult<String> {
    Ok(state.creds.backend_name().await.to_string())
}

/// Switch API-key storage between the local 0600 file (default, no OS
/// password prompts) and the OS keychain. Existing keys are migrated.
#[tauri::command]
pub async fn switch_credential_storage(
    state: State<'_, AppState>,
    target: String,
) -> CmdResult<usize> {
    if target != "file" && target != "keychain" {
        return Err(format!("unknown credential storage target: {target}"));
    }
    // Collect all credential references that exist.
    let refs: Vec<String> = state
        .db
        .list_providers()
        .map_err(err)?
        .iter()
        .filter_map(|p| p.credential_ref.clone())
        .collect();
    let migrated = state.creds.switch_to(&target, &refs).await.map_err(err)?;
    let mut settings = state.db.get_settings().map_err(err)?;
    settings.credential_storage = target;
    state.db.save_settings(&settings).map_err(err)?;
    Ok(migrated)
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_providers(state: State<'_, AppState>) -> CmdResult<Vec<ProviderConfig>> {
    state.db.list_providers().map_err(err)
}

#[tauri::command]
pub async fn save_provider(
    state: State<'_, AppState>,
    provider: ProviderConfig,
    api_key: Option<String>,
) -> CmdResult<ProviderConfig> {
    let mut provider = provider;
    let now = now_ms();
    if provider.created_at == 0 {
        provider.created_at = now;
    }
    provider.updated_at = now;
    // The credential reference is always derived here; a frontend-supplied
    // value could point at another provider's key.
    let cred_ref = format!("provider:{}", provider.id);

    match api_key {
        Some(key) if !key.trim().is_empty() => {
            state
                .creds
                .set(&cred_ref, key.trim())
                .await
                .map_err(err)?;
            provider.credential_ref = Some(cred_ref);
        }
        Some(_) => {
            // Empty string clears the stored credential.
            state.creds.delete(&cred_ref).await.map_err(err)?;
            provider.credential_ref = None;
        }
        None => {
            // Edit without key change: keep a reference only if this provider
            // already had a stored credential.
            let had_key = state
                .db
                .get_provider(&provider.id)
                .map_err(err)?
                .is_some_and(|p| p.credential_ref.is_some());
            provider.credential_ref = had_key.then_some(cred_ref);
        }
    }
    state.db.save_provider(&provider).map_err(err)?;
    Ok(provider)
}

#[tauri::command]
pub async fn delete_provider(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    if let Some(cfg) = state.db.get_provider(&id).map_err(err)? {
        if let Some(cred) = &cfg.credential_ref {
            let _ = state.creds.delete(cred).await;
        }
    }
    state.db.delete_provider(&id).map_err(err)
}

#[tauri::command]
pub async fn test_provider(
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<ProviderTestResult> {
    let cfg = state
        .db
        .get_provider(&id)
        .map_err(err)?
        .ok_or_else(|| format!("provider {id} not found"))?;
    let adapter = create_provider(&cfg, state.creds.clone()).map_err(err)?;
    let started = Instant::now();
    match adapter.list_models().await {
        Ok(models) => Ok(ProviderTestResult {
            ok: true,
            latency_ms: started.elapsed().as_millis() as u64,
            model_count: Some(models.len()),
            error: None,
        }),
        Err(e) => Ok(ProviderTestResult {
            ok: false,
            latency_ms: started.elapsed().as_millis() as u64,
            model_count: None,
            error: Some(format!("{e:#}")),
        }),
    }
}

#[tauri::command]
pub async fn refresh_models(
    state: State<'_, AppState>,
    provider_id: String,
) -> CmdResult<Vec<ModelInfo>> {
    let cfg = state
        .db
        .get_provider(&provider_id)
        .map_err(err)?
        .ok_or_else(|| format!("provider {provider_id} not found"))?;
    let adapter = create_provider(&cfg, state.creds.clone()).map_err(err)?;
    let models = adapter.list_models().await.map_err(err)?;
    state
        .db
        .replace_discovered_models(&provider_id, &models)
        .map_err(err)?;
    state.db.list_models(Some(&provider_id)).map_err(err)
}

#[tauri::command]
pub fn list_models(
    state: State<'_, AppState>,
    provider_id: Option<String>,
) -> CmdResult<Vec<ModelInfo>> {
    state.db.list_models(provider_id.as_deref()).map_err(err)
}

#[tauri::command]
pub fn add_custom_model(
    state: State<'_, AppState>,
    provider_id: String,
    name: String,
    display_name: Option<String>,
    input_price_per_mtok: Option<f64>,
    output_price_per_mtok: Option<f64>,
) -> CmdResult<ModelInfo> {
    state
        .db
        .save_custom_model(
            &provider_id,
            &name,
            display_name.as_deref(),
            input_price_per_mtok,
            output_price_per_mtok,
        )
        .map_err(err)
}

#[tauri::command]
pub fn delete_model(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.db.delete_model(&id).map_err(err)
}

#[tauri::command]
pub fn provider_presets() -> CmdResult<Vec<serde_json::Value>> {
    Ok(jcode_provider_core::provider_presets()
        .into_iter()
        .map(|p| {
            json!({
                "id": p.id,
                "label": p.label,
                "kind": p.kind.as_str(),
                "base_url": p.base_url,
                "needs_key": p.needs_key,
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Agent profiles
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_agent_profiles(state: State<'_, AppState>) -> CmdResult<Vec<AgentProfile>> {
    state.db.list_agent_profiles().map_err(err)
}

#[tauri::command]
pub fn save_agent_profile(
    state: State<'_, AppState>,
    mut profile: AgentProfile,
) -> CmdResult<AgentProfile> {
    if profile.created_at == 0 {
        profile.created_at = now_ms();
    }
    // Built-in ids stay built-in; custom ids cannot spoof them for deletion
    // protection purposes.
    state.db.save_agent_profile(&profile).map_err(err)?;
    Ok(profile)
}

#[tauri::command]
pub fn delete_agent_profile(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let profiles = state.db.list_agent_profiles().map_err(err)?;
    if profiles.iter().any(|p| p.id == id && p.built_in) {
        return Err("built-in profiles cannot be deleted".into());
    }
    state.db.delete_agent_profile(&id).map_err(err)
}

// ---------------------------------------------------------------------------
// Sessions & chat
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_sessions(
    state: State<'_, AppState>,
    workspace_id: Option<String>,
) -> CmdResult<Vec<SessionSummary>> {
    state.db.list_sessions(workspace_id.as_deref()).map_err(err)
}

#[tauri::command]
pub async fn search_sessions(state: State<'_, AppState>, query: String) -> CmdResult<Vec<SessionSummary>> {
    let db = state.db.clone();
    blocking(move || db.search_sessions(&query).map_err(err)).await
}

#[tauri::command]
pub fn create_session(
    state: State<'_, AppState>,
    workspace_id: String,
    agent_profile_id: Option<String>,
    provider_id: Option<String>,
    model_name: Option<String>,
) -> CmdResult<SessionSummary> {
    let mut s = state
        .db
        .create_session(
            &workspace_id,
            "New session",
            agent_profile_id.as_deref(),
            provider_id.as_deref(),
            model_name.as_deref(),
        )
        .map_err(err)?;
    // Persist the default policy on the session so the choice survives
    // restarts and later settings changes.
    let settings = state.db.get_settings().unwrap_or_default();
    state
        .db
        .set_session_policy_mode(&s.id, settings.default_policy_mode.as_str())
        .map_err(err)?;
    s.policy_mode = Some(settings.default_policy_mode);
    // Prime the session policy engine.
    if let Some(ws) = state.db.get_workspace(&workspace_id).map_err(err)? {
        state
            .runtime
            .ensure_session_policy(&s.id, &ws.root_path, settings.default_policy_mode);
    }
    Ok(s)
}

#[tauri::command]
pub async fn get_session_messages(
    state: State<'_, AppState>,
    session_id: String,
) -> CmdResult<Vec<MessageRecord>> {
    let db = state.db.clone();
    blocking(move || db.list_messages(&session_id).map_err(err)).await
}

#[tauri::command]
pub fn get_session_tool_runs(
    state: State<'_, AppState>,
    session_id: String,
) -> CmdResult<Vec<ToolRunRecord>> {
    state.db.list_tool_runs(&session_id).map_err(err)
}

#[tauri::command]
pub fn get_session_approvals(
    state: State<'_, AppState>,
    session_id: String,
) -> CmdResult<Vec<ApprovalRecord>> {
    state.db.list_approvals(&session_id).map_err(err)
}

#[tauri::command]
pub async fn get_session_detail(
    state: State<'_, AppState>,
    session_id: String,
) -> CmdResult<SessionDetail> {
    let db = state.db.clone();
    blocking(move || {
        let session = db
            .get_session(&session_id)
            .map_err(err)?
            .ok_or_else(|| format!("session {session_id} not found"))?;
        Ok(SessionDetail {
            session,
            messages: db.list_messages(&session_id).map_err(err)?,
            tool_runs: db.list_tool_runs(&session_id).map_err(err)?,
            approvals: db.list_approvals(&session_id).map_err(err)?,
        })
    })
    .await
}

#[derive(Serialize)]
pub struct SessionDetail {
    pub session: SessionSummary,
    pub messages: Vec<MessageRecord>,
    pub tool_runs: Vec<ToolRunRecord>,
    pub approvals: Vec<ApprovalRecord>,
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    session_id: String,
    content: String,
    attachments: Vec<String>,
) -> CmdResult<()> {
    state
        .runtime
        .send_message(&session_id, &content, &attachments)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn cancel_generation(state: State<'_, AppState>, session_id: String) -> CmdResult<()> {
    state.runtime.cancel(&session_id).await.map_err(err)
}

#[tauri::command]
pub async fn decide_approval(
    state: State<'_, AppState>,
    approval_id: String,
    approved: bool,
    grant: Option<String>,
) -> CmdResult<()> {
    let grant = grant.as_deref().and_then(Grant::parse);
    state
        .runtime
        .decide_approval(&approval_id, approved, grant)
        .await
        .map_err(err)
}

#[tauri::command]
pub fn set_session_model(
    state: State<'_, AppState>,
    session_id: String,
    provider_id: Option<String>,
    model_name: Option<String>,
) -> CmdResult<()> {
    state
        .db
        .update_session_model(&session_id, provider_id.as_deref(), model_name.as_deref())
        .map_err(err)
}

#[tauri::command]
pub fn set_session_agent(
    state: State<'_, AppState>,
    session_id: String,
    agent_profile_id: Option<String>,
) -> CmdResult<()> {
    state
        .db
        .update_session_agent(&session_id, agent_profile_id.as_deref())
        .map_err(err)
}

#[tauri::command]
pub async fn set_policy_mode(
    state: State<'_, AppState>,
    session_id: String,
    mode: String,
) -> CmdResult<()> {
    let mode = PolicyMode::from_str_lossy(&mode);
    state
        .runtime
        .set_policy_mode(&session_id, mode)
        .await
        .map_err(err)
}

#[tauri::command]
pub fn delete_session(state: State<'_, AppState>, session_id: String) -> CmdResult<()> {
    state.db.delete_session(&session_id).map_err(err)
}

#[tauri::command]
pub fn rename_session(state: State<'_, AppState>, session_id: String, title: String) -> CmdResult<()> {
    state.db.rename_session(&session_id, &title).map_err(err)
}

#[tauri::command]
pub fn session_is_busy(state: State<'_, AppState>, session_id: String) -> CmdResult<bool> {
    Ok(state.runtime.is_busy(&session_id))
}

#[tauri::command]
pub async fn list_checkpoints(
    state: State<'_, AppState>,
    session_id: String,
) -> CmdResult<Vec<jcode_tool_filesystem::CheckpointEntry>> {
    let runtime = state.runtime.clone();
    blocking(move || Ok(runtime.list_checkpoints(&session_id))).await
}

/// Restore a session's checkpoints into *that session's* workspace (not
/// whichever workspace happens to be open now).
#[tauri::command]
pub async fn restore_checkpoints(
    state: State<'_, AppState>,
    session_id: String,
) -> CmdResult<usize> {
    let db = state.db.clone();
    let runtime = state.runtime.clone();
    blocking(move || {
        let session = db
            .get_session(&session_id)
            .map_err(err)?
            .ok_or_else(|| format!("session {session_id} not found"))?;
        let ws = db
            .get_workspace(&session.workspace_id)
            .map_err(err)?
            .ok_or_else(|| format!("workspace of session {session_id} not found"))?;
        runtime
            .restore_checkpoints(&session_id, &ws.root_path)
            .map_err(err)
    })
    .await
}

// ---------------------------------------------------------------------------
// Git
// ---------------------------------------------------------------------------

/// Run `f` against the active workspace's git repo on the blocking pool.
async fn with_repo<T, F>(state: &State<'_, AppState>, f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce(GitRepo) -> CmdResult<T> + Send + 'static,
{
    let root = state.active_root()?;
    blocking(move || {
        let repo = GitRepo::detect(&root)
            .ok_or_else(|| "the active workspace is not a git repository".to_string())?;
        f(repo)
    })
    .await
}

#[tauri::command]
pub async fn git_status(state: State<'_, AppState>) -> CmdResult<Vec<jcode_tool_git::FileStatus>> {
    with_repo(&state, move |repo| {
        repo.status().map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn git_diff(
    state: State<'_, AppState>,
    path: Option<String>,
    staged: Option<bool>,
) -> CmdResult<String> {
    with_repo(&state, move |repo| {
        repo
            .diff(path.as_deref(), staged.unwrap_or(false))
            .map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn git_diff_all(state: State<'_, AppState>, path: Option<String>) -> CmdResult<String> {
    with_repo(&state, move |repo| {
        repo.diff_head(path.as_deref()).map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn git_stage(state: State<'_, AppState>, paths: Vec<String>) -> CmdResult<()> {
    with_repo(&state, move |repo| {
        let refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
        repo.stage(&refs).map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn git_unstage(state: State<'_, AppState>, paths: Vec<String>) -> CmdResult<()> {
    with_repo(&state, move |repo| {
        let refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
        repo.unstage(&refs).map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn git_discard(state: State<'_, AppState>, path: String) -> CmdResult<()> {
    with_repo(&state, move |repo| {
        repo.discard(&path).map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn git_commit(state: State<'_, AppState>, message: String) -> CmdResult<String> {
    with_repo(&state, move |repo| {
        repo.commit(&message).map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn git_branches(state: State<'_, AppState>) -> CmdResult<Vec<jcode_tool_git::BranchInfo>> {
    with_repo(&state, move |repo| {
        repo.branches().map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn git_checkout_branch(state: State<'_, AppState>, name: String) -> CmdResult<()> {
    with_repo(&state, move |repo| {
        repo.checkout_branch(&name).map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn git_create_branch(state: State<'_, AppState>, name: String) -> CmdResult<()> {
    with_repo(&state, move |repo| {
        repo.create_branch(&name).map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn git_current_branch(state: State<'_, AppState>) -> CmdResult<Option<String>> {
    with_repo(&state, move |repo| {
        repo.current_branch().map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn git_log(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> CmdResult<Vec<jcode_tool_git::CommitInfo>> {
    with_repo(&state, move |repo| {
        repo.log(limit.unwrap_or(20)).map_err(err)
    })
    .await
}

// ---------------------------------------------------------------------------
// Usage & settings
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_data_dir(state: State<'_, AppState>) -> CmdResult<String> {
    Ok(state.data_dir.to_string_lossy().to_string())
}

/// Set the webview zoom factor for all windows (Cmd/Ctrl +/- shortcuts).
/// The factor is clamped server-side as a safety net.
#[tauri::command]
pub fn set_zoom(app: AppHandle, factor: f64) -> CmdResult<()> {
    use tauri::Manager;
    let clamped = factor.clamp(0.5, 2.0);
    for (_, window) in app.webview_windows() {
        window.set_zoom(clamped).map_err(err)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn usage_summary(state: State<'_, AppState>) -> CmdResult<jcode_storage::UsageSummary> {
    let db = state.db.clone();
    blocking(move || db.usage_summary().map_err(err)).await
}

#[tauri::command]
pub async fn get_session_usage(
    state: State<'_, AppState>,
    session_id: String,
) -> CmdResult<jcode_storage::SessionUsage> {
    let db = state.db.clone();
    blocking(move || db.session_usage(&session_id).map_err(err)).await
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> CmdResult<AppSettings> {
    state.db.get_settings().map_err(err)
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: AppSettings) -> CmdResult<()> {
    state.db.save_settings(&settings).map_err(err)
}

#[tauri::command]
pub async fn apply_retention(state: State<'_, AppState>, days: u32) -> CmdResult<usize> {
    let db = state.db.clone();
    blocking(move || db.apply_retention(days).map_err(err)).await
}

#[tauri::command]
pub async fn clear_history(state: State<'_, AppState>) -> CmdResult<usize> {
    let db = state.db.clone();
    blocking(move || db.clear_all_history().map_err(err)).await
}

/// Validate an export destination: `.json` only, never through a symlink,
/// never inside the app data dir (DB, credentials, checkpoints).
fn validate_export_path(path: &Path, data_dir: &Path) -> CmdResult<()> {
    let is_json = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
    if !is_json {
        return Err("export path must end in .json".into());
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err("refusing to export through a symbolic link".into());
        }
        if !meta.is_file() {
            return Err("export path exists and is not a regular file".into());
        }
    }
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => return Err("export path must be absolute".into()),
    };
    let parent = std::fs::canonicalize(parent)
        .map_err(|e| format!("export directory {}: {e}", parent.display()))?;
    let data_dir = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    if parent.starts_with(&data_dir) {
        return Err("refusing to write into the application data directory".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn export_history(state: State<'_, AppState>, path: String) -> CmdResult<String> {
    let db = state.db.clone();
    let data_dir = state.data_dir.clone();
    blocking(move || {
        let target = PathBuf::from(&path);
        if !target.is_absolute() {
            return Err("export path must be absolute".into());
        }
        validate_export_path(&target, &data_dir)?;
        let json = db.export_history_json().map_err(err)?;
        std::fs::write(&target, &json).map_err(err)?;
        Ok(path)
    })
    .await
}

// ---------------------------------------------------------------------------
// Terminals
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn terminal_create(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    cols: Option<u16>,
    rows: Option<u16>,
) -> CmdResult<()> {
    let cwd = state.active_root().unwrap_or_else(|_| PathBuf::from("."));
    let terminals = state.terminals.clone();
    blocking(move || {
        terminals
            .create(&app, &id, &cwd, cols.unwrap_or(80), rows.unwrap_or(24))
            .map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn terminal_write(state: State<'_, AppState>, id: String, data: String) -> CmdResult<()> {
    let terminals = state.terminals.clone();
    blocking(move || terminals.write(&id, &data).map_err(err)).await
}

#[tauri::command]
pub fn terminal_resize(
    state: State<'_, AppState>,
    id: String,
    cols: u16,
    rows: u16,
) -> CmdResult<()> {
    state.terminals.resize(&id, cols, rows).map_err(err)
}

#[tauri::command]
pub async fn terminal_kill(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let terminals = state.terminals.clone();
    blocking(move || terminals.kill(&id).map_err(err)).await
}

#[tauri::command]
pub fn terminal_list(state: State<'_, AppState>) -> CmdResult<Vec<String>> {
    Ok(state.terminals.list())
}

// ---------------------------------------------------------------------------
// External CLI agents (experimental)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn cli_agent_presets() -> CmdResult<Vec<CliAgentConfig>> {
    Ok(jcode_agent_cli::cli_agent_presets())
}

#[tauri::command]
pub async fn run_external_agent(
    state: State<'_, AppState>,
    preset_id: String,
    prompt: String,
) -> CmdResult<jcode_agent_cli::CliAgentResult> {
    let preset = jcode_agent_cli::cli_agent_presets()
        .into_iter()
        .find(|p| p.id == preset_id)
        .ok_or_else(|| format!("unknown external agent preset: {preset_id}"))?;
    let cwd = state.active_root()?;
    let cancel = jcode_tool_terminal::CancelToken::new();
    run_cli_agent(&preset, &prompt, &cwd, &cancel)
        .await
        .map_err(err)
}

// Silence "unused import" for Arc used in signatures above.
#[allow(unused)]
fn _arc_guard(_: Arc<Db>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_root_rejects_dangerous_dirs() {
        assert!(validate_workspace_root(Path::new("/")).is_err());
        if let Some(home) = home_dir() {
            assert!(validate_workspace_root(&home).is_err());
        }
        #[cfg(unix)]
        {
            assert!(validate_workspace_root(Path::new("/etc")).is_err());
            assert!(validate_workspace_root(Path::new("/usr")).is_err());
            assert!(validate_workspace_root(Path::new("/bin")).is_err());
        }
        let tmp = std::env::temp_dir().join(format!("jcode-ws-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(validate_workspace_root(&tmp).is_ok());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn export_path_validation() {
        let base = std::env::temp_dir().join(format!("jcode-export-{}", std::process::id()));
        let data = base.join("data");
        let out = base.join("out");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        assert!(validate_export_path(&out.join("h.json"), &data).is_ok());
        assert!(validate_export_path(&out.join("h.txt"), &data).is_err());
        assert!(validate_export_path(&data.join("h.json"), &data).is_err());
        #[cfg(unix)]
        {
            let link = out.join("link.json");
            std::os::unix::fs::symlink(data.join("x.json"), &link).unwrap();
            assert!(validate_export_path(&link, &data).is_err());
        }
        std::fs::remove_dir_all(&base).ok();
    }
}
