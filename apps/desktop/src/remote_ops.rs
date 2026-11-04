//! Blocking request handlers for the phone (docs/REMOTE_PROTOCOL.md §6.6–§6.8):
//! agents, models, session settings, skills, files and git. Everything is
//! scoped to the project named by `workspace_id` (or the session's project),
//! never to whichever project the desktop window has open.

use jcode_common::*;
use jcode_remote::protocol::AppError;
use jcode_storage::Db;
use jcode_tool_filesystem::{is_sensitive_name, Workspace};
use jcode_tool_git::GitRepo;
use serde_json::{json, Value};
use std::path::Path;

const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_DIFF_BYTES: usize = 1024 * 1024;

pub fn bad(msg: impl Into<String>) -> AppError {
    AppError::new("bad_request", msg)
}

fn not_found(msg: impl Into<String>) -> AppError {
    AppError::new("not_found", msg)
}

fn internal(e: impl std::fmt::Display) -> AppError {
    AppError::new("internal", format!("{e:#}"))
}

pub fn not_allowed() -> AppError {
    AppError::new(
        "not_allowed",
        "Not allowed from the phone. Turn on \"Allow Full access & terminal from phone\" on the desktop (Settings → Remote control).",
    )
}

pub fn str_param<'a>(params: &'a Value, key: &str) -> Result<&'a str, AppError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && s.len() <= 1000)
        .ok_or_else(|| bad(format!("missing {key}")))
}

fn opt_str<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
}

pub fn workspace_root(db: &Db, workspace_id: &str) -> Result<String, AppError> {
    db.get_workspace(workspace_id)
        .map_err(internal)?
        .map(|w| w.root_path)
        .ok_or_else(|| not_found("project not found"))
}

fn workspace(db: &Db, params: &Value) -> Result<Workspace, AppError> {
    let root = workspace_root(db, str_param(params, "workspace_id")?)?;
    Workspace::open(&root).map_err(|e| bad(format!("{e:#}")))
}

fn repo(db: &Db, params: &Value) -> Result<GitRepo, AppError> {
    let root = workspace_root(db, str_param(params, "workspace_id")?)?;
    GitRepo::detect(Path::new(&root)).ok_or_else(|| bad("this project is not a git repository"))
}

fn session(db: &Db, id: &str) -> Result<SessionSummary, AppError> {
    db.get_session(id).map_err(internal)?.ok_or_else(|| not_found("session not found"))
}

/// Refuse secret-like files (`.env`, keys, …) for the phone, as for `@`.
fn check_not_sensitive(path: &str) -> Result<(), AppError> {
    if is_sensitive_name(Path::new(path)) {
        return Err(bad("this file may hold secrets and cannot be opened from the phone"));
    }
    Ok(())
}

fn cap_text(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n… (truncated)\n");
    }
    s
}

fn paths_param(params: &Value) -> Result<Vec<String>, AppError> {
    let list = params
        .get("paths")
        .and_then(|v| v.as_array())
        .ok_or_else(|| bad("missing paths"))?;
    let paths: Vec<String> = list.iter().filter_map(|p| p.as_str().map(str::to_string)).collect();
    if paths.is_empty() || paths.len() > 500 {
        return Err(bad("paths must hold 1–500 entries"));
    }
    Ok(paths)
}

/// Methods handled here. `None` = not one of ours. `can_edit` is the
/// desktop's "Allow Full access & terminal from phone" setting.
pub fn dispatch(db: &Db, method: &str, params: &Value, can_edit: bool) -> Option<Result<Value, AppError>> {
    let edit = || if can_edit { Ok(()) } else { Err(not_allowed()) };
    let r = match method {
        // §6.6 chat
        "agents.list" => db.list_agent_profiles().map_err(internal).map(|list| {
            let agents: Vec<Value> = list
                .into_iter()
                .map(|p| json!({ "id": p.id, "name": p.name, "mode": p.mode, "policy_mode": p.policy_mode, "built_in": p.built_in }))
                .collect();
            json!({ "agents": agents })
        }),
        "models.list" => (|| {
            let providers: Vec<ProviderConfig> =
                db.list_providers().map_err(internal)?.into_iter().filter(|p| p.enabled).collect();
            let ids: std::collections::HashSet<&str> = providers.iter().map(|p| p.id.as_str()).collect();
            let models: Vec<ModelInfo> = db
                .list_models(None)
                .map_err(internal)?
                .into_iter()
                .filter(|m| ids.contains(m.provider_id.as_str()))
                .collect();
            let providers: Vec<Value> = providers
                .iter()
                .map(|p| json!({ "id": p.id, "name": p.name, "kind": p.kind }))
                .collect();
            Ok(json!({ "providers": providers, "models": models }))
        })(),
        "session.set_agent" => (|| {
            let sid = str_param(params, "session_id")?;
            let pid = str_param(params, "agent_profile_id")?;
            let exists = db.list_agent_profiles().map_err(internal)?.iter().any(|p| p.id == pid);
            if !exists {
                return Err(not_found("agent not found"));
            }
            session(db, sid)?;
            db.update_session_agent(sid, Some(pid)).map_err(internal)?;
            Ok(json!({ "session": session(db, sid)? }))
        })(),
        "session.set_model" => (|| {
            let sid = str_param(params, "session_id")?;
            let provider = str_param(params, "provider_id")?;
            let model = str_param(params, "model_name")?;
            let known = db
                .list_models(Some(provider))
                .map_err(internal)?
                .iter()
                .any(|m| m.name == model);
            if !known {
                return Err(not_found("model not found"));
            }
            session(db, sid)?;
            db.update_session_model(sid, Some(provider), Some(model)).map_err(internal)?;
            Ok(json!({ "session": session(db, sid)? }))
        })(),
        "session.rename" => (|| {
            let sid = str_param(params, "session_id")?;
            let title = str_param(params, "title")?.trim();
            if title.is_empty() || title.chars().count() > 200 {
                return Err(bad("title must be 1–200 characters"));
            }
            session(db, sid)?;
            db.rename_session(sid, title).map_err(internal)?;
            Ok(json!({ "session": session(db, sid)? }))
        })(),
        "session.delete" => (|| {
            let sid = str_param(params, "session_id")?;
            session(db, sid)?;
            db.delete_session(sid).map_err(internal)?;
            Ok(json!({}))
        })(),
        "session.usage" => (|| {
            let sid = str_param(params, "session_id")?;
            db.session_usage(sid).map_err(internal).map(|u| json!(u))
        })(),
        "skills.list" => (|| {
            let root = workspace_root(db, str_param(params, "workspace_id")?)?;
            let disabled = db.get_settings().map(|s| s.disabled_skills).unwrap_or_default();
            let home = jcode_agent_runtime::skills::home_dir();
            let skills: Vec<Value> = jcode_agent_runtime::skills::discover(Some(Path::new(&root)), home.as_deref())
                .into_iter()
                .filter(|s| !disabled.contains(&s.name))
                .map(|s| json!({ "name": s.name, "description": s.description, "project": s.project }))
                .collect();
            Ok(json!({ "skills": skills }))
        })(),
        "fs.find" => (|| {
            let ws = workspace(db, params)?;
            let q = params.get("query").and_then(|v| v.as_str()).unwrap_or_default();
            Ok(json!({ "paths": ws.find_files(q, 30) }))
        })(),

        // §6.7 files
        "fs.list" => (|| {
            let ws = workspace(db, params)?;
            let path = params.get("path").and_then(|v| v.as_str()).unwrap_or_default();
            let hidden = params.get("show_hidden").and_then(|v| v.as_bool()).unwrap_or(true);
            let entries = ws.list_dir(path, hidden).map_err(|e| bad(format!("{e:#}")))?;
            Ok(json!({ "entries": entries }))
        })(),
        "fs.read" => (|| {
            let ws = workspace(db, params)?;
            let path = str_param(params, "path")?;
            check_not_sensitive(path)?;
            let content = ws.read_file(path).map_err(|e| bad(format!("{e:#}")))?;
            if content.len() > MAX_FILE_BYTES {
                return Err(bad("file is larger than 1 MiB"));
            }
            Ok(json!({ "path": path, "size": content.len(), "content": content }))
        })(),
        "fs.write" => edit().and_then(|_| {
            let ws = workspace(db, params)?;
            let path = str_param(params, "path")?;
            check_not_sensitive(path)?;
            let content = params.get("content").and_then(|v| v.as_str()).ok_or_else(|| bad("missing content"))?;
            if content.len() > MAX_FILE_BYTES {
                return Err(bad("content is larger than 1 MiB"));
            }
            ws.write_file(path, content).map_err(|e| bad(format!("{e:#}")))?;
            Ok(json!({}))
        }),
        "fs.create" => edit().and_then(|_| {
            let ws = workspace(db, params)?;
            let path = str_param(params, "path")?;
            check_not_sensitive(path)?;
            let is_dir = params.get("is_dir").and_then(|v| v.as_bool()).unwrap_or(false);
            let abs = ws.resolve(path).map_err(|e| bad(format!("{e:#}")))?;
            if abs.exists() {
                return Err(bad("already exists"));
            }
            if is_dir {
                std::fs::create_dir_all(&abs).map_err(internal)?;
            } else {
                ws.write_file(path, "").map_err(|e| bad(format!("{e:#}")))?;
            }
            Ok(json!({}))
        }),
        "fs.delete" => edit().and_then(|_| {
            let ws = workspace(db, params)?;
            ws.delete_path(str_param(params, "path")?).map_err(|e| bad(format!("{e:#}")))?;
            Ok(json!({}))
        }),
        "fs.rename" => edit().and_then(|_| {
            let ws = workspace(db, params)?;
            let to = str_param(params, "to")?;
            check_not_sensitive(to)?;
            ws.rename(str_param(params, "from")?, to).map_err(|e| bad(format!("{e:#}")))?;
            Ok(json!({}))
        }),

        // §6.8 git
        "git.status" => (|| {
            let repo = repo(db, params)?;
            let files = repo.status().map_err(|e| bad(format!("{e:#}")))?;
            let branch = repo.current_branch().ok().flatten();
            Ok(json!({ "branch": branch, "files": files }))
        })(),
        "git.diff" => (|| {
            let repo = repo(db, params)?;
            let staged = params.get("staged").and_then(|v| v.as_bool()).unwrap_or(false);
            let diff = repo.diff(opt_str(params, "path"), staged).map_err(|e| bad(format!("{e:#}")))?;
            Ok(json!({ "diff": cap_text(diff, MAX_DIFF_BYTES) }))
        })(),
        "git.diff_head" => (|| {
            let repo = repo(db, params)?;
            let diff = repo.diff_head(opt_str(params, "path")).map_err(|e| bad(format!("{e:#}")))?;
            Ok(json!({ "diff": cap_text(diff, MAX_DIFF_BYTES) }))
        })(),
        "git.log" => (|| {
            let repo = repo(db, params)?;
            let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(20).clamp(1, 100) as usize;
            Ok(json!({ "commits": repo.log(limit).map_err(|e| bad(format!("{e:#}")))? }))
        })(),
        "git.branches" => (|| {
            let repo = repo(db, params)?;
            Ok(json!({ "branches": repo.branches().map_err(|e| bad(format!("{e:#}")))? }))
        })(),
        "git.stage" | "git.unstage" => edit().and_then(|_| {
            let repo = repo(db, params)?;
            let paths = paths_param(params)?;
            let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
            let r = if method == "git.stage" { repo.stage(&refs) } else { repo.unstage(&refs) };
            r.map_err(|e| bad(format!("{e:#}")))?;
            Ok(json!({}))
        }),
        "git.discard" => edit().and_then(|_| {
            let repo = repo(db, params)?;
            repo.discard(str_param(params, "path")?).map_err(|e| bad(format!("{e:#}")))?;
            Ok(json!({}))
        }),
        "git.commit" => edit().and_then(|_| {
            let repo = repo(db, params)?;
            let message = str_param(params, "message")?.trim();
            if message.is_empty() {
                return Err(bad("empty commit message"));
            }
            Ok(json!({ "hash": repo.commit(message).map_err(|e| bad(format!("{e:#}")))? }))
        }),
        "git.checkout" => edit().and_then(|_| {
            let repo = repo(db, params)?;
            repo.checkout_branch(str_param(params, "name")?).map_err(|e| bad(format!("{e:#}")))?;
            Ok(json!({}))
        }),
        "git.create_branch" => edit().and_then(|_| {
            let repo = repo(db, params)?;
            repo.create_branch(str_param(params, "name")?).map_err(|e| bad(format!("{e:#}")))?;
            Ok(json!({}))
        }),
        _ => return None,
    };
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with_project() -> (tempfile::TempDir, Db, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1\n").unwrap();
        let db = Db::open(dir.path().join("t.db")).unwrap();
        let ws = db.upsert_workspace(root.to_str().unwrap(), "proj").unwrap();
        (dir, db, ws.id)
    }

    #[test]
    fn files_are_scoped_to_the_project_and_secrets_stay_closed() {
        let (_d, db, wid) = db_with_project();
        let list = dispatch(&db, "fs.list", &json!({ "workspace_id": wid, "path": "" }), false).unwrap().unwrap();
        let names: Vec<&str> = list["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"src"));
        let read = dispatch(&db, "fs.read", &json!({ "workspace_id": wid, "path": "src/main.rs" }), false).unwrap().unwrap();
        assert_eq!(read["content"], "fn main() {}\n");
        let env = dispatch(&db, "fs.read", &json!({ "workspace_id": wid, "path": ".env" }), false).unwrap();
        assert_eq!(env.unwrap_err().code, "bad_request");
        let escape = dispatch(&db, "fs.read", &json!({ "workspace_id": wid, "path": "../t.db" }), false).unwrap();
        assert!(escape.is_err());
    }

    #[test]
    fn writes_need_the_desktop_setting() {
        let (_d, db, wid) = db_with_project();
        let p = json!({ "workspace_id": wid, "path": "src/new.txt", "content": "hi" });
        assert_eq!(dispatch(&db, "fs.write", &p, false).unwrap().unwrap_err().code, "not_allowed");
        dispatch(&db, "fs.write", &p, true).unwrap().unwrap();
        let read = dispatch(&db, "fs.read", &json!({ "workspace_id": wid, "path": "src/new.txt" }), false).unwrap().unwrap();
        assert_eq!(read["content"], "hi");
        for m in ["fs.create", "fs.delete", "git.commit", "git.discard", "git.checkout", "git.stage"] {
            assert_eq!(dispatch(&db, m, &json!({}), false).unwrap().unwrap_err().code, "not_allowed", "{m}");
        }
    }

    #[test]
    fn git_needs_a_repository_and_unknown_methods_fall_through() {
        let (_d, db, wid) = db_with_project();
        let r = dispatch(&db, "git.status", &json!({ "workspace_id": wid }), false).unwrap();
        assert_eq!(r.unwrap_err().code, "bad_request");
        assert!(dispatch(&db, "chat.send", &json!({}), true).is_none());
        let missing = dispatch(&db, "fs.list", &json!({ "workspace_id": "nope" }), false).unwrap();
        assert_eq!(missing.unwrap_err().code, "not_found");
    }
}
