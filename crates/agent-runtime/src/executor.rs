//! Tool registry, permission classification, previews, and execution.
//!
//! Every tool the model can call is defined here once; the specs are sent to
//! providers in their native formats, actions are classified for the
//! permission engine, and execution goes through the workspace-layer safety
//! checks (path containment, symlinks, sensitive files).

use anyhow::{anyhow, bail, Result};
use jcode_common::{AgentMode, AgentProfile, ToolSpec, WorkspaceInfo};
use jcode_permissions::{is_sensitive_path, Action};
use jcode_tool_filesystem::{lexical_normalize, CheckpointStore, SearchQuery, Workspace};
use jcode_tool_git::GitRepo;
use jcode_tool_terminal::{CancelToken, ExecSpec};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Cap for content sent back to the model per tool result.
const MAX_RESULT_FOR_MODEL: usize = 100_000;

#[derive(Clone)]
pub struct ToolContext {
    pub session_id: String,
    pub workspace: Workspace,
    pub checkpoints: Option<Arc<CheckpointStore>>,
}

impl ToolContext {
    pub fn new(session_id: impl Into<String>, workspace: Workspace, checkpoints: Option<CheckpointStore>) -> Self {
        Self {
            session_id: session_id.into(),
            workspace,
            checkpoints: checkpoints.map(Arc::new),
        }
    }

    /// Snapshot a file before it is modified. The checkpoint key is the
    /// normalized workspace-relative path of the *resolved* target, so
    /// `a.rs`, `./a.rs` and `/abs/ws/a.rs` share one snapshot.
    fn checkpoint(&self, input: &str) -> Result<()> {
        if let Some(store) = &self.checkpoints {
            let rel = checkpoint_key(&self.workspace, input)?;
            store.snapshot(&self.session_id, self.workspace.root(), &rel)?;
        }
        Ok(())
    }
}

/// Normalized workspace-relative key for a model-provided path.
pub fn checkpoint_key(ws: &Workspace, input: &str) -> Result<String> {
    let resolved = ws.resolve(input)?;
    let rel = ws.rel_string(&resolved);
    if rel.is_empty() {
        bail!("refusing to checkpoint the workspace root itself");
    }
    Ok(rel)
}

fn spec(name: &str, description: &str, properties: Value, required: &[&str]) -> ToolSpec {
    let mut props = json!({});
    for (k, v) in properties.as_object().unwrap() {
        props[k] = v.clone();
    }
    ToolSpec {
        name: name.into(),
        description: description.into(),
        parameters: json!({
            "type": "object",
            "properties": props,
            "required": required,
        }),
    }
}

/// Tool specs for a session. `read_only` drops every mutating tool
/// (Plan/Review modes); `allowed` further filters by the agent profile.
pub fn tool_specs(read_only: bool, allowed: Option<&[String]>) -> Vec<ToolSpec> {
    let mut specs = vec![
        spec(
            "fs_read",
            "Read a text file from the workspace. Returns the full content.",
            json!({"path": {"type": "string", "description": "File path relative to the workspace root (or absolute inside it)"}}),
            &["path"],
        ),
        spec(
            "fs_list",
            "List entries of a directory (one level). Omit path for the workspace root.",
            json!({"path": {"type": "string", "description": "Directory path; defaults to the workspace root"}}),
            &[],
        ),
        spec(
            "fs_search",
            "Search file contents across the workspace (respects .gitignore). Returns path:line matches.",
            json!({
                "query": {"type": "string", "description": "Text or regex to find"},
                "is_regex": {"type": "boolean", "description": "Treat the query as a regular expression"},
                "include_glob": {"type": "string", "description": "Only search files matching this glob, e.g. **/*.rs"}
            }),
            &["query"],
        ),
        spec(
            "glob_files",
            "List workspace files matching a glob pattern (e.g. **/*.ts).",
            json!({"pattern": {"type": "string"}}),
            &["pattern"],
        ),
        spec(
            "terminal_run",
            "Run a shell command in the workspace root and return combined output. Use it for builds, tests, and inspections.",
            json!({
                "command": {"type": "string"},
                "timeout_ms": {"type": "integer", "description": "Timeout in milliseconds (1000-600000; default 60000)"}
            }),
            &["command"],
        ),
        spec("git_status", "Show the working tree status (staged + modified + untracked files).", json!({}), &[]),
        spec(
            "git_diff",
            "Show a unified diff of changes. By default the working tree vs index.",
            json!({
                "path": {"type": "string", "description": "Limit to one path"},
                "staged": {"type": "boolean", "description": "Show staged changes instead"}
            }),
            &[],
        ),
        spec(
            "git_log",
            "Show recent commits.",
            json!({"limit": {"type": "integer", "description": "Number of commits (default 10)"}}),
            &[],
        ),
    ];
    if !read_only {
        specs.push(spec(
            "fs_write",
            "Create or fully overwrite a file with the given content. Parent directories are created automatically.",
            json!({
                "path": {"type": "string"},
                "content": {"type": "string"}
            }),
            &["path", "content"],
        ));
        specs.push(spec(
            "fs_edit",
            "Replace an exact string in a file. old_string must match the file exactly (including whitespace) and be unique unless replace_all is true. Prefer this over fs_write for small changes.",
            json!({
                "path": {"type": "string"},
                "old_string": {"type": "string"},
                "new_string": {"type": "string"},
                "replace_all": {"type": "boolean", "description": "Replace every occurrence"}
            }),
            &["path", "old_string", "new_string"],
        ));
        specs.push(spec(
            "fs_delete",
            "Delete a file or directory inside the workspace.",
            json!({"path": {"type": "string"}}),
            &["path"],
        ));
    }
    if let Some(allow) = allowed {
        specs.retain(|s| allow.iter().any(|a| a == &s.name));
    }
    specs
}

/// Is this tool read-only (allowed in Plan/Review modes)? Note that
/// `terminal_run` is allowed in Review (running tests is part of reviewing)
/// and kept out of Plan via that profile's tool allowlist; the permission
/// engine still gates every command.
pub fn is_read_only_tool(name: &str) -> bool {
    !matches!(name, "fs_write" | "fs_edit" | "fs_delete")
}

/// Execution-time gate for agent mode and profile allowlist. Returns the
/// rejection message for the model, or `None` when the tool may run.
/// - tools outside `profile.allowed_tools` are rejected;
/// - Plan mode is read-only *without* terminal access (`terminal_run`
///   rejected); Review keeps `terminal_run` (gated by the policy engine);
/// - write tools are rejected in Plan/Review (also checked by the caller via
///   [`is_read_only_tool`]).
pub fn tool_rejection(profile: &AgentProfile, name: &str) -> Option<String> {
    if let Some(allowed) = &profile.allowed_tools {
        if !allowed.iter().any(|a| a == name) {
            return Some(format!(
                "Tool `{name}` is not enabled for the `{}` agent profile.",
                profile.name
            ));
        }
    }
    match profile.mode {
        AgentMode::Plan if name == "terminal_run" => Some(format!(
            "Tool `{name}` is not available in plan mode (read-only, no terminal)."
        )),
        AgentMode::Plan | AgentMode::Review if !is_read_only_tool(name) => Some(format!(
            "Tool `{name}` is not available in {} mode (write access disabled).",
            profile.mode.as_str()
        )),
        _ => None,
    }
}

fn get_str(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| anyhow!("missing required string argument `{key}`"))
}

fn get_str_opt(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(String::from)
}

/// Same trimming `Workspace::resolve` applies to model-provided paths, so
/// classification looks at the path that will actually be used.
fn trim_model_path(input: &str) -> &str {
    input
        .trim()
        .trim_matches(|c| c == '`' || c == '"' || c == '\'')
}

/// Classified target of a path-taking tool.
struct ClassifiedPath {
    path: PathBuf,
    inside: bool,
    sensitive: bool,
}

/// Resolve a path the same way execution will (`Workspace::resolve`:
/// trimming, canonicalization, symlink-escape checks) and report whether it
/// is inside the workspace and whether it looks sensitive. Sensitivity is
/// checked on both the raw (trimmed) input and the resolved target, so a
/// symlink `notes.txt -> .env` or a quoted "`.env`" is still caught.
/// If resolution fails the path is treated as outside the workspace
/// (writes/deletes are then denied, reads need approval).
fn classify_path(ws: &Workspace, input: &str) -> ClassifiedPath {
    let trimmed = trim_model_path(input);
    let raw_sensitive = is_sensitive_path(Path::new(trimmed));
    match ws.resolve(input) {
        Ok(resolved) => {
            let inside = resolved.starts_with(ws.root());
            let sensitive = raw_sensitive || is_sensitive_path(&resolved);
            ClassifiedPath { path: resolved, inside, sensitive }
        }
        Err(_) => {
            let candidate = Path::new(trimmed);
            let joined = if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                ws.root().join(candidate)
            };
            let norm = lexical_normalize(&joined);
            let sensitive = raw_sensitive || is_sensitive_path(&norm);
            ClassifiedPath { path: norm, inside: false, sensitive }
        }
    }
}

/// Map a tool invocation to a permission action.
pub fn classify(name: &str, args: &Value, ctx: &ToolContext) -> Result<Action> {
    match name {
        "fs_read" => {
            let path = get_str(args, "path")?;
            let c = classify_path(&ctx.workspace, &path);
            if c.inside {
                Ok(Action::ReadFile { path: c.path, sensitive: c.sensitive })
            } else {
                Ok(Action::ReadOutsideWorkspace { path: c.path })
            }
        }
        "fs_list" | "glob_files" | "fs_search" | "git_status" | "git_diff" | "git_log" => {
            // Read-only tools: reads stay inside the workspace (enforced at
            // execution by Workspace::resolve).
            Ok(Action::ReadFile {
                path: ctx.workspace.root().to_path_buf(),
                sensitive: false,
            })
        }
        "fs_write" | "fs_edit" => {
            let path = get_str(args, "path")?;
            let ClassifiedPath { path: p, inside, .. } = classify_path(&ctx.workspace, &path);
            if !inside {
                return Ok(Action::WriteOutsideWorkspace { path: p });
            }
            if name == "fs_write" {
                Ok(Action::WriteFile { path: p })
            } else {
                Ok(Action::EditFile { path: p })
            }
        }
        "fs_delete" => {
            let path = get_str(args, "path")?;
            let ClassifiedPath { path: p, inside, .. } = classify_path(&ctx.workspace, &path);
            if !inside {
                return Ok(Action::WriteOutsideWorkspace { path: p });
            }
            Ok(Action::DeleteFile { path: p })
        }
        "terminal_run" => {
            let command = get_str(args, "command")?;
            Ok(Action::RunCommand {
                command,
                cwd: ctx.workspace.root().to_path_buf(),
            })
        }
        other => bail!("unknown tool: {other}"),
    }
}

/// Produce a human-readable preview for an approval card (diff for edits,
/// command line for terminal). Does not mutate anything.
pub fn preview(name: &str, args: &Value, ctx: &ToolContext) -> Option<String> {
    match name {
        "fs_write" => {
            let path = get_str(args, "path").ok()?;
            let content = get_str(args, "content").ok()?;
            let before = ctx.workspace.read_file(&path).unwrap_or_default();
            Some(jcode_tool_filesystem::unified_diff(
                &before,
                &content,
                &path,
            ))
        }
        "fs_edit" => {
            let path = get_str(args, "path").ok()?;
            let old = get_str(args, "old_string").ok()?;
            let new = get_str(args, "new_string").ok()?;
            let replace_all = args.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
            let before = ctx.workspace.read_file(&path).ok()?;
            let occurrences = before.matches(&old).count();
            if occurrences == 0 {
                return Some(format!("(old_string not found in {path} — this edit will fail)"));
            }
            if occurrences > 1 && !replace_all {
                return Some(format!(
                    "(old_string occurs {occurrences} times in {path} — provide more context or set replace_all)"
                ));
            }
            let after = if replace_all {
                before.replace(&old, &new)
            } else {
                before.replacen(&old, &new, 1)
            };
            Some(jcode_tool_filesystem::unified_diff(&before, &after, &path))
        }
        "fs_delete" => {
            let path = get_str(args, "path").ok()?;
            Some(format!("Delete {path}"))
        }
        "terminal_run" => get_str(args, "command").ok(),
        _ => None,
    }
}

fn truncate_for_model(s: &str) -> String {
    if s.len() <= MAX_RESULT_FOR_MODEL {
        return s.to_string();
    }
    // Cut on a char boundary near the limit.
    let mut end = MAX_RESULT_FOR_MODEL;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n... [truncated, {} of {} bytes shown]",
        &s[..end],
        end,
        s.len()
    )
}

/// Outcome of a successful tool execution.
pub struct ToolOutcome {
    /// Short summary for the UI tool-run card.
    pub summary: String,
    /// Content sent back to the model.
    pub result_for_model: String,
    /// Diff shown in the diff viewer (write/edit tools).
    pub diff: Option<String>,
}

pub async fn execute(
    name: &str,
    args: &Value,
    ctx: &ToolContext,
    cancel: &CancelToken,
) -> Result<ToolOutcome> {
    if cancel.is_cancelled() {
        bail!("cancelled before execution");
    }
    if name == "terminal_run" {
        return run_terminal(args, &ctx.workspace, cancel).await;
    }
    // Filesystem and git tools do blocking I/O (and spawn `git`): keep them
    // off the async executor threads.
    let name = name.to_string();
    let args = args.clone();
    let ctx = ctx.clone();
    tokio::task::spawn_blocking(move || execute_blocking(&name, &args, &ctx))
        .await
        .map_err(|e| anyhow!("tool task failed: {e}"))?
}

async fn run_terminal(args: &Value, ws: &Workspace, cancel: &CancelToken) -> Result<ToolOutcome> {
    let command = get_str(args, "command")?;
    let timeout = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(jcode_tool_terminal::DEFAULT_TIMEOUT_MS);
    let mut spec = ExecSpec::new(&command, ws.root());
    spec.timeout_ms = timeout.clamp(1_000, jcode_tool_terminal::MAX_TIMEOUT_MS);
    let res = jcode_tool_terminal::run_exec(&spec, cancel).await?;
    let summary = format!(
        "`{}` → {} ({})",
        truncate(&command, 60),
        match res.exit_code {
            Some(c) => format!("exit {c}"),
            None => "killed".to_string(),
        },
        format_duration(res.duration_ms)
    );
    let output = res.output;
    let exit_line = res
        .exit_code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "killed".into());
    let body = format!(
        "$ {command}\n{output}\n[exit: {} | {}{}]",
        exit_line,
        format_duration(res.duration_ms),
        if res.truncated { " | output truncated" } else { "" }
    );
    Ok(ToolOutcome {
        summary,
        result_for_model: truncate_for_model(&body),
        diff: None,
    })
}

fn execute_blocking(name: &str, args: &Value, ctx: &ToolContext) -> Result<ToolOutcome> {
    let ws = &ctx.workspace;
    match name {
        "fs_read" => {
            let path = get_str(args, "path")?;
            let content = ws.read_file(&path)?;
            let lines = content.lines().count();
            Ok(ToolOutcome {
                summary: format!("Read {path} ({lines} lines)"),
                result_for_model: truncate_for_model(&format!("--- {path} ---\n{content}")),
                diff: None,
            })
        }
        "fs_list" => {
            let path = get_str_opt(args, "path").unwrap_or_default();
            let entries = ws.list_dir(&path, false)?;
            let listing = entries
                .iter()
                .map(|e| {
                    if e.is_dir {
                        format!("{}/", e.name)
                    } else {
                        e.name.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let summary = format!("Listed {} ({}/ entries)", if path.is_empty() { "." } else { &path }, entries.len());
            Ok(ToolOutcome {
                summary,
                result_for_model: truncate_for_model(&format!("--- {} ---\n{}", if path.is_empty() { "." } else { &path }, listing)),
                diff: None,
            })
        }
        "fs_search" => {
            let query = get_str(args, "query")?;
            let hits = ws.search(&SearchQuery {
                query: query.clone(),
                is_regex: args.get("is_regex").and_then(|v| v.as_bool()).unwrap_or(false),
                include_glob: get_str_opt(args, "include_glob"),
            })?;
            let summary = format!("Searched {:?} — {} hit(s)", query, hits.len());
            let body = if hits.is_empty() {
                "No matches.".to_string()
            } else {
                hits.iter()
                    .map(|h| format!("{}:{}: {}", h.path, h.line_number.unwrap_or(0), h.line.trim()))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(ToolOutcome {
                summary,
                result_for_model: truncate_for_model(&body),
                diff: None,
            })
        }
        "glob_files" => {
            let pattern = get_str(args, "pattern")?;
            let files = ws.glob_files(&pattern, 500)?;
            let summary = format!("Glob {} — {} file(s)", pattern, files.len());
            Ok(ToolOutcome {
                summary,
                result_for_model: truncate_for_model(&files.join("\n")),
                diff: None,
            })
        }
        "fs_write" => {
            let path = get_str(args, "path")?;
            let content = get_str(args, "content")?;
            ctx.checkpoint(&path)?;
            let out = ws.write_file(&path, &content)?;
            Ok(ToolOutcome {
                summary: format!("Wrote {} ({} bytes)", out.path, content.len()),
                result_for_model: format!("Successfully wrote {}.", out.path),
                diff: Some(out.diff),
            })
        }
        "fs_edit" => {
            let path = get_str(args, "path")?;
            let old = get_str(args, "old_string")?;
            let new = get_str(args, "new_string")?;
            let replace_all = args.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
            ctx.checkpoint(&path)?;
            let out = ws.edit_file(&path, &old, &new, replace_all)?;
            Ok(ToolOutcome {
                summary: format!("Edited {} ({} replacement(s))", out.path, out.occurrences),
                result_for_model: format!("Successfully edited {}.", out.path),
                diff: Some(out.diff),
            })
        }
        "fs_delete" => {
            let path = get_str(args, "path")?;
            ctx.checkpoint(&path)?;
            let deleted = ws.delete_path(&path)?;
            Ok(ToolOutcome {
                summary: format!("Deleted {deleted}"),
                result_for_model: format!("Successfully deleted {deleted}."),
                diff: None,
            })
        }
        "git_status" => {
            let repo = require_repo(ws)?;
            let status = repo.status()?;
            let body = if status.is_empty() {
                "Working tree clean.".to_string()
            } else {
                status
                    .iter()
                    .map(|f| {
                        format!(
                            "{} {}{}",
                            status_symbol(f),
                            f.path,
                            if f.staged() { " (staged)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(ToolOutcome {
                summary: format!("Git status: {} change(s)", status.len()),
                result_for_model: truncate_for_model(&body),
                diff: None,
            })
        }
        "git_diff" => {
            let repo = require_repo(ws)?;
            let path = get_str_opt(args, "path");
            let staged = args.get("staged").and_then(|v| v.as_bool()).unwrap_or(false);
            let diff = repo.diff(path.as_deref(), staged)?;
            Ok(ToolOutcome {
                summary: format!("Git diff ({} chars)", diff.len()),
                result_for_model: truncate_for_model(&diff),
                diff: None,
            })
        }
        "git_log" => {
            let repo = require_repo(ws)?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let log = repo.log(limit)?;
            let body = log
                .iter()
                .map(|c| format!("{} {} — {} ({})", c.short_hash, c.subject, c.author, c.timestamp))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(ToolOutcome {
                summary: format!("Git log: {} commit(s)", log.len()),
                result_for_model: truncate_for_model(&body),
                diff: None,
            })
        }
        other => bail!("unknown tool: {other}"),
    }
}

fn require_repo(ws: &Workspace) -> Result<GitRepo> {
    GitRepo::detect(ws.root()).ok_or_else(|| anyhow!("not a git repository"))
}

fn status_symbol(f: &jcode_tool_git::FileStatus) -> &'static str {
    use jcode_tool_git::StatusKind::*;
    if f.is_untracked() {
        return "??";
    }
    match (f.index, f.worktree) {
        (Modified, _) | (_, Modified) => "M ",
        (Added, _) => "A ",
        (Deleted, _) | (_, Deleted) => "D ",
        (Renamed, _) => "R ",
        _ => "  ",
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

/// Unused import guard: WorkspaceInfo may be needed by callers extending
/// this module.
#[allow(dead_code)]
fn _workspace_info_guard(_: Option<WorkspaceInfo>) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        (dir, ToolContext::new("sess", ws, None))
    }

    #[test]
    fn specs_respect_read_only_and_allowlist() {
        let all = tool_specs(false, None);
        assert!(all.iter().any(|t| t.name == "fs_write"));
        let ro = tool_specs(true, None);
        assert!(!ro.iter().any(|t| t.name == "fs_write"));
        assert!(!ro.iter().any(|t| t.name == "fs_delete"));
        // terminal stays available in read-only modes (Review runs tests);
        // Plan excludes it via its profile allowlist.
        assert!(ro.iter().any(|t| t.name == "terminal_run"));
        assert!(ro.iter().any(|t| t.name == "fs_read"));

        let filtered = tool_specs(false, Some(&["fs_read".into(), "fs_search".into()]));
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn classification_of_reads_and_writes() {
        let (_t, c) = ctx();
        let a = classify(
            "fs_read",
            &json!({"path": "src/main.rs"}),
            &c,
        )
        .unwrap();
        assert!(matches!(a, Action::ReadFile { sensitive: false, .. }));

        let a = classify("fs_read", &json!({"path": ".env"}), &c).unwrap();
        assert!(matches!(a, Action::ReadFile { sensitive: true, .. }));

        let a = classify("fs_read", &json!({"path": "../../etc/passwd"}), &c).unwrap();
        assert!(matches!(a, Action::ReadOutsideWorkspace { .. }));

        let a = classify("fs_write", &json!({"path": "new.txt", "content": "x"}), &c).unwrap();
        assert!(matches!(a, Action::WriteFile { .. }));

        let a = classify(
            "fs_write",
            &json!({"path": "/etc/passwd", "content": "x"}),
            &c,
        )
        .unwrap();
        assert!(matches!(a, Action::WriteOutsideWorkspace { .. }));

        let a = classify("terminal_run", &json!({"command": "cargo test"}), &c).unwrap();
        assert!(matches!(a, Action::RunCommand { .. }));
    }

    #[test]
    fn preview_shows_diff_without_applying() {
        let (_t, c) = ctx();
        c.workspace.write_file("p.txt", "line1\nline2\n").unwrap();
        let pv = preview(
            "fs_edit",
            &json!({"path": "p.txt", "old_string": "line2", "new_string": "LINE TWO"}),
            &c,
        )
        .unwrap();
        assert!(pv.contains("-line2"));
        assert!(pv.contains("+LINE TWO"));
        // File untouched by the preview.
        assert_eq!(c.workspace.read_file("p.txt").unwrap(), "line1\nline2\n");
    }

    #[tokio::test]
    async fn executes_read_write_and_search() {
        let (_t, c) = ctx();
        c.workspace.write_file("a.txt", "needle here\n").unwrap();

        let out = execute("fs_read", &json!({"path": "a.txt"}), &c, &CancelToken::new())
            .await
            .unwrap();
        assert!(out.result_for_model.contains("needle"));

        let out = execute(
            "fs_search",
            &json!({"query": "needle"}),
            &c,
            &CancelToken::new(),
        )
        .await
        .unwrap();
        assert!(out.result_for_model.contains("a.txt:1"));

        let out = execute(
            "fs_write",
            &json!({"path": "b/new.txt", "content": "fresh"}),
            &c,
            &CancelToken::new(),
        )
        .await
        .unwrap();
        assert!(out.diff.as_deref().unwrap().contains("+fresh"));
        assert_eq!(c.workspace.read_file("b/new.txt").unwrap(), "fresh");

        let out = execute(
            "terminal_run",
            &json!({"command": "echo exec-ok"}),
            &c,
            &CancelToken::new(),
        )
        .await
        .unwrap();
        assert!(out.result_for_model.contains("exec-ok"));
    }

    fn profile(mode: AgentMode, allowed: Option<Vec<&str>>) -> AgentProfile {
        AgentProfile {
            id: "p".into(),
            name: "P".into(),
            mode,
            system_prompt: String::new(),
            allowed_tools: allowed.map(|v| v.into_iter().map(String::from).collect()),
            policy_mode: jcode_common::PolicyMode::Safe,
            provider_id: None,
            model_id: None,
            built_in: false,
            created_at: 0,
        }
    }

    #[test]
    fn execution_gate_enforces_mode_and_allowlist() {
        let plan = profile(AgentMode::Plan, None);
        assert!(tool_rejection(&plan, "terminal_run").is_some());
        assert!(tool_rejection(&plan, "fs_write").is_some());
        assert!(tool_rejection(&plan, "fs_read").is_none());
        let review = profile(AgentMode::Review, None);
        assert!(tool_rejection(&review, "terminal_run").is_none());
        assert!(tool_rejection(&review, "fs_delete").is_some());
        let build = profile(AgentMode::Build, Some(vec!["fs_read"]));
        assert!(tool_rejection(&build, "fs_read").is_none());
        assert!(tool_rejection(&build, "terminal_run").is_some());
        assert!(tool_rejection(&profile(AgentMode::Build, None), "fs_write").is_none());
    }

    #[test]
    fn sensitive_classification_matches_resolution() {
        let (_t, c) = ctx();
        c.workspace.write_file(".env", "SECRET=1").unwrap();
        for p in ["`.env`", " .env ", "\".env\"", "'.env'", "./.env"] {
            let a = classify("fs_read", &json!({"path": p}), &c).unwrap();
            assert!(matches!(a, Action::ReadFile { sensitive: true, .. }), "{p:?} -> {a:?}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(c.workspace.root().join(".env"), c.workspace.root().join("notes.txt")).unwrap();
            let a = classify("fs_read", &json!({"path": "notes.txt"}), &c).unwrap();
            assert!(matches!(a, Action::ReadFile { sensitive: true, .. }), "symlink -> {a:?}");
        }
    }

    #[test]
    fn checkpoint_keys_are_normalized() {
        let (_t, c) = ctx();
        c.workspace.write_file("a.rs", "x").unwrap();
        let abs = c.workspace.root().join("a.rs");
        for p in ["a.rs", "./a.rs", "`a.rs`", abs.to_str().unwrap()] {
            assert_eq!(checkpoint_key(&c.workspace, p).unwrap(), "a.rs", "{p}");
        }
    }
}
