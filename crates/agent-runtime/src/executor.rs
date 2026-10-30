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
/// Lines `fs_read` returns when no `limit` is given.
const READ_DEFAULT_LIMIT: usize = 2_000;
/// Longer lines (minified code, data) are cut in `fs_read` output.
const READ_MAX_LINE_CHARS: usize = 2_000;

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
            "Read a text file from the workspace. Lines are numbered like `cat -n` (`<number>\\t<line>`); \
             the number prefix is not part of the file. Returns up to 2000 lines; use offset/limit for \
             long files. Read several files in one response when you need them.",
            json!({
                "path": {"type": "string", "description": "File path relative to the workspace root (or absolute inside it)"},
                "offset": {"type": "integer", "description": "1-based line number to start from (default 1)"},
                "limit": {"type": "integer", "description": "Maximum number of lines to return (default 2000)"}
            }),
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
            "Search file contents across the workspace (respects .gitignore). Returns `path:line: text` matches; \
             with context_lines, surrounding lines are shown as `path-line- text`.",
            json!({
                "query": {"type": "string", "description": "Text or regex to find"},
                "is_regex": {"type": "boolean", "description": "Treat the query as a regular expression"},
                "case_insensitive": {"type": "boolean", "description": "Ignore letter case"},
                "include_glob": {"type": "string", "description": "Only search files matching this glob, e.g. **/*.rs"},
                "path": {"type": "string", "description": "Only search inside this directory or file"},
                "context_lines": {"type": "integer", "description": "Lines of context before and after each match (0-5)"},
                "max_results": {"type": "integer", "description": "Maximum matches to return (default 100, max 500)"}
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
                "timeout_ms": {"type": "integer", "description": "Timeout in milliseconds (1000-600000; default 120000)"}
            }),
            &["command"],
        ),
        spec(
            "web_fetch",
            "Fetch a web page (documentation, API reference, changelog) and return its readable text. \
             Needs user approval unless full access is on. Use it for information that is not in the workspace.",
            json!({
                "url": {"type": "string", "description": "http(s) URL"},
                "raw": {"type": "boolean", "description": "Return the raw body instead of text extracted from HTML"}
            }),
            &["url"],
        ),
        spec(
            "todo_write",
            "Create or update your task list for multi-step work. Send the complete list every time. Use it for \
             tasks with 3 or more steps: plan the steps first, keep exactly one item in_progress, and mark items \
             completed as soon as they are done.",
            json!({
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {"type": "string", "description": "What to do, imperative"},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}
                        },
                        "required": ["content", "status"]
                    }
                }
            }),
            &["todos"],
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
            "Replace a string in a file. Always fs_read the file first and copy old_string from its CURRENT content (not from memory), without the line-number prefix — include 2-3 surrounding lines so it is unique. Small differences in indentation, trailing whitespace, line endings or quote style are tolerated; the match must still be unique unless replace_all is true. Prefer this over fs_write for small changes. On failure the error shows the most similar region of the file.",
            json!({
                "path": {"type": "string"},
                "old_string": {"type": "string"},
                "new_string": {"type": "string"},
                "replace_all": {"type": "boolean", "description": "Replace every occurrence"}
            }),
            &["path", "old_string", "new_string"],
        ));
        specs.push(spec(
            "fs_multi_edit",
            "Apply several edits to one file in a single step, in order. Each edit works like fs_edit (old_string \
             from the current content, unique unless replace_all). All edits succeed or none is applied. Prefer \
             this over several fs_edit calls on the same file.",
            json!({
                "path": {"type": "string"},
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": {"type": "string"},
                            "new_string": {"type": "string"},
                            "replace_all": {"type": "boolean"}
                        },
                        "required": ["old_string", "new_string"]
                    }
                }
            }),
            &["path", "edits"],
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
    !matches!(name, "fs_write" | "fs_edit" | "fs_multi_edit" | "fs_delete")
}

/// Tools that only read and never wait on the user in the common case, so a
/// batch of them can run concurrently.
pub fn is_parallel_safe(name: &str) -> bool {
    matches!(
        name,
        "fs_read" | "fs_list" | "fs_search" | "glob_files" | "git_status" | "git_diff" | "git_log" | "todo_write"
    )
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
        // `todo_write` only echoes its arguments: classify it as a plain read.
        "fs_list" | "glob_files" | "fs_search" | "git_status" | "git_diff" | "git_log" | "todo_write" => {
            // Read-only tools: reads stay inside the workspace (enforced at
            // execution by Workspace::resolve).
            Ok(Action::ReadFile {
                path: ctx.workspace.root().to_path_buf(),
                sensitive: false,
            })
        }
        "web_fetch" => Ok(Action::FetchUrl { url: get_str(args, "url")?.trim().to_string() }),
        "fs_write" | "fs_edit" | "fs_multi_edit" => {
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
            let (old, new) = strip_copied_line_numbers(&before, old, new);
            match jcode_tool_filesystem::plan_edit(&before, &old, &new, replace_all, &path) {
                Ok(p) => Some(jcode_tool_filesystem::unified_diff(&before, &p.after, &path)),
                Err(e) => {
                    let first = e.to_string().lines().next().unwrap_or_default().to_string();
                    Some(format!("(this edit will fail: {first})"))
                }
            }
        }
        "fs_multi_edit" => {
            let path = get_str(args, "path").ok()?;
            let before = ctx.workspace.read_file(&path).ok()?;
            match apply_multi_edit(&before, args, &path) {
                Ok(after) => Some(jcode_tool_filesystem::unified_diff(&before, &after, &path)),
                Err(e) => Some(format!("(this edit will fail: {})", e.to_string().lines().next().unwrap_or_default())),
            }
        }
        "fs_delete" => {
            let path = get_str(args, "path").ok()?;
            Some(format!("Delete {path}"))
        }
        "web_fetch" => get_str(args, "url").ok(),
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

/// Like [`truncate_for_model`] but keeps the start and the (more useful) end
/// of command output: build errors and test summaries come last.
fn truncate_middle_for_model(s: &str) -> String {
    if s.len() <= MAX_RESULT_FOR_MODEL {
        return s.to_string();
    }
    let mut head = MAX_RESULT_FOR_MODEL / 4;
    while !s.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = s.len() - (MAX_RESULT_FOR_MODEL - head);
    while !s.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}\n\n... [{} bytes omitted] ...\n\n{}", &s[..head], tail - head, &s[tail..])
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
    if name == "web_fetch" {
        return run_web_fetch(args, cancel).await;
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
        if res.truncated {
            " | long output: start and end kept, middle omitted; filter it (grep, tail) to see a specific part"
        } else {
            ""
        }
    );
    Ok(ToolOutcome {
        summary,
        result_for_model: truncate_middle_for_model(&body),
        diff: None,
    })
}

async fn run_web_fetch(args: &Value, cancel: &CancelToken) -> Result<ToolOutcome> {
    let url = get_str(args, "url")?.trim().to_string();
    let raw = args.get("raw").and_then(|v| v.as_bool()).unwrap_or(false);
    let fetched = tokio::select! {
        r = crate::web::fetch(&url, raw) => r?,
        _ = cancel.cancelled() => bail!("cancelled"),
    };
    let header = if fetched.final_url == url {
        format!("--- {url} ({}) ---", fetched.content_type)
    } else {
        format!("--- {url} -> {} ({}) ---", fetched.final_url, fetched.content_type)
    };
    Ok(ToolOutcome {
        summary: format!("Fetched {} ({:.1} KB)", truncate(&url, 60), fetched.bytes as f64 / 1024.0),
        result_for_model: truncate_for_model(&format!("{header}\n{}", fetched.text)),
        diff: None,
    })
}

/// Apply `fs_multi_edit` edits to `before` in order; any failure aborts.
fn apply_multi_edit(before: &str, args: &Value, path: &str) -> Result<String> {
    let edits = args
        .get("edits")
        .and_then(|v| v.as_array())
        .filter(|e| !e.is_empty())
        .ok_or_else(|| anyhow!("`edits` must be a non-empty list"))?;
    let mut content = before.to_string();
    for (i, e) in edits.iter().enumerate() {
        let old = get_str(e, "old_string").map_err(|err| anyhow!("edit {}: {err}", i + 1))?;
        let new = get_str(e, "new_string").map_err(|err| anyhow!("edit {}: {err}", i + 1))?;
        let replace_all = e.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
        let (old, new) = strip_copied_line_numbers(&content, old, new);
        content = jcode_tool_filesystem::plan_edit(&content, &old, &new, replace_all, path)
            .map_err(|err| anyhow!("edit {} of {} failed (no edits were applied): {err:#}", i + 1, edits.len()))?
            .after;
    }
    Ok(content)
}

fn execute_blocking(name: &str, args: &Value, ctx: &ToolContext) -> Result<ToolOutcome> {
    let ws = &ctx.workspace;
    match name {
        "fs_read" => {
            let path = get_str(args, "path")?;
            let content = ws.read_file(&path)?;
            let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
            let limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|l| l.max(1) as usize)
                .unwrap_or(READ_DEFAULT_LIMIT);
            let (body, shown) = numbered_lines(&path, &content, offset, limit)?;
            Ok(ToolOutcome {
                summary: format!("Read {path} ({shown})"),
                result_for_model: truncate_for_model(&body),
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
            let max = args.get("max_results").and_then(|v| v.as_u64()).map(|n| n as usize);
            let hits = ws.search(&SearchQuery {
                query: query.clone(),
                is_regex: args.get("is_regex").and_then(|v| v.as_bool()).unwrap_or(false),
                include_glob: get_str_opt(args, "include_glob"),
                case_insensitive: args.get("case_insensitive").and_then(|v| v.as_bool()).unwrap_or(false),
                context_lines: args.get("context_lines").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                path: get_str_opt(args, "path"),
                max_results: max,
            })?;
            let summary = format!("Searched {:?} — {} hit(s)", query, hits.len());
            let cap = max.unwrap_or(jcode_tool_filesystem::MAX_SEARCH_RESULTS).clamp(1, 500);
            let mut body = if hits.is_empty() { "No matches.".to_string() } else { format_hits(&hits) };
            if hits.len() >= cap {
                body.push_str(&format!(
                    "\n(Stopped at {cap} matches. Narrow the query, or use path / include_glob.)"
                ));
            }
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
            let (old, new) = match ws.read_file(&path) {
                Ok(before) => strip_copied_line_numbers(&before, old, new),
                Err(_) => (old, new),
            };
            ctx.checkpoint(&path)?;
            let out = ws.edit_file(&path, &old, &new, replace_all)?;
            let fuzzy = out.strategy.filter(|s| *s != jcode_tool_filesystem::MatchStrategy::Exact);
            let note = fuzzy.map(|s| format!(" ({})", s.describe())).unwrap_or_default();
            // Show the model what actually changed, so it can verify the
            // edit (especially a relaxed match) without re-reading the file.
            let result_for_model = truncate_for_model(&format!(
                "Successfully edited {}{note}. Resulting diff:\n{}",
                out.path, out.diff
            ));
            Ok(ToolOutcome {
                summary: format!("Edited {} ({} replacement(s)){note}", out.path, out.occurrences),
                result_for_model,
                diff: Some(out.diff),
            })
        }
        "fs_multi_edit" => {
            let path = get_str(args, "path")?;
            let before = ws.read_file(&path)?;
            let after = apply_multi_edit(&before, args, &path)?;
            let count = args.get("edits").and_then(|v| v.as_array()).map_or(0, Vec::len);
            ctx.checkpoint(&path)?;
            let out = ws.write_file(&path, &after)?;
            Ok(ToolOutcome {
                summary: format!("Edited {} ({count} edit(s))", out.path),
                result_for_model: truncate_for_model(&format!(
                    "Successfully applied {count} edit(s) to {}. Resulting diff:\n{}",
                    out.path, out.diff
                )),
                diff: Some(out.diff),
            })
        }
        "todo_write" => {
            let (summary, result_for_model) = crate::todo::apply(args)?;
            Ok(ToolOutcome { summary, result_for_model, diff: None })
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

/// `fs_read` output for lines `offset..offset+limit` (1-based) in `cat -n`
/// style, plus a short description of the range for the UI summary.
fn numbered_lines(path: &str, content: &str, offset: usize, limit: usize) -> Result<(String, String)> {
    let total = content.lines().count();
    if total == 0 {
        return Ok((format!("--- {path} (empty file) ---"), "empty".into()));
    }
    if offset > total {
        bail!("offset {offset} is past the end of {path} ({total} lines)");
    }
    let end = (offset - 1 + limit).min(total);
    let mut out = format!("--- {path} (lines {offset}-{end} of {total}) ---\n");
    for (i, line) in content.lines().enumerate().skip(offset - 1).take(end + 1 - offset) {
        let line = match line.char_indices().nth(READ_MAX_LINE_CHARS) {
            Some((cut, _)) => format!("{}… [line truncated]", &line[..cut]),
            None => line.to_string(),
        };
        out.push_str(&format!("{:>6}\t{line}\n", i + 1));
    }
    if end < total {
        out.push_str(&format!("(Showing lines {offset}-{end} of {total}. Use offset={} to read more.)\n", end + 1));
    }
    let shown = if offset == 1 && end == total {
        format!("{total} lines")
    } else {
        format!("lines {offset}-{end} of {total}")
    };
    Ok((out, shown))
}

/// Remove a `fs_read` line-number prefix (`   12\t`) from `s` when every
/// non-empty line has one.
fn strip_line_number_prefix(s: &str) -> Option<String> {
    let mut any = false;
    let mut out = Vec::new();
    for line in s.split('\n') {
        if line.is_empty() {
            out.push(line);
            continue;
        }
        let rest = line.trim_start_matches(' ');
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let stripped = rest[digits..].strip_prefix('\t').filter(|_| digits > 0)?;
        out.push(stripped);
        any = true;
    }
    any.then(|| out.join("\n"))
}

/// Models sometimes paste `old_string`/`new_string` straight from `fs_read`
/// output, line numbers included. If `old` is not in the file but its
/// prefix-stripped form is, use the stripped strings.
fn strip_copied_line_numbers(before: &str, old: String, new: String) -> (String, String) {
    if before.contains(&old) {
        return (old, new);
    }
    match strip_line_number_prefix(&old) {
        Some(stripped) if before.contains(&stripped) => {
            let new = strip_line_number_prefix(&new).unwrap_or(new);
            (stripped, new)
        }
        _ => (old, new),
    }
}

/// ripgrep-style output: `path:line: match`, context as `path-line- text`,
/// `--` between non-adjacent groups.
fn format_hits(hits: &[jcode_common::SearchHit]) -> String {
    let with_context = hits.iter().any(|h| !h.before.is_empty() || !h.after.is_empty());
    let mut out = Vec::new();
    for (i, h) in hits.iter().enumerate() {
        let n = h.line_number.unwrap_or(0);
        if with_context && i > 0 {
            out.push("--".to_string());
        }
        for (k, l) in h.before.iter().enumerate() {
            out.push(format!("{}-{}- {}", h.path, n as usize - h.before.len() + k, l));
        }
        out.push(format!("{}:{}: {}", h.path, n, h.line.trim_end()));
        for (k, l) in h.after.iter().enumerate() {
            out.push(format!("{}-{}- {}", h.path, n as usize + 1 + k, l));
        }
    }
    out.join("\n")
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

    #[tokio::test]
    async fn fs_read_numbers_lines_and_pages() {
        let (_t, c) = ctx();
        let body: String = (1..=5).map(|i| format!("line {i}\n")).collect();
        c.workspace.write_file("f.txt", &body).unwrap();
        let read = |args: Value| {
            let c = c.clone();
            async move { execute("fs_read", &args, &c, &CancelToken::new()).await }
        };
        let out = read(json!({"path": "f.txt"})).await.unwrap();
        assert!(out.result_for_model.starts_with("--- f.txt (lines 1-5 of 5) ---"));
        assert!(out.result_for_model.contains("     1\tline 1\n"));
        assert!(!out.result_for_model.contains("offset="));

        let out = read(json!({"path": "f.txt", "offset": 2, "limit": 2})).await.unwrap();
        assert!(out.result_for_model.contains("     2\tline 2"));
        assert!(out.result_for_model.contains("     3\tline 3"));
        assert!(!out.result_for_model.contains("line 4"));
        assert!(out.result_for_model.contains("Use offset=4"));
        assert_eq!(out.summary, "Read f.txt (lines 2-3 of 5)");

        assert!(read(json!({"path": "f.txt", "offset": 9})).await.is_err());

        c.workspace.write_file("long.txt", &"x".repeat(5_000)).unwrap();
        let out = read(json!({"path": "long.txt"})).await.unwrap();
        assert!(out.result_for_model.contains("[line truncated]"));
        assert!(out.result_for_model.len() < 2_200);
    }

    #[tokio::test]
    async fn fs_edit_accepts_old_string_copied_with_line_numbers() {
        let (_t, c) = ctx();
        c.workspace.write_file("a.rs", "fn a() {\n    1\n}\n").unwrap();
        let args = json!({
            "path": "a.rs",
            "old_string": "     1\tfn a() {\n     2\t    1",
            "new_string": "     1\tfn a() {\n     2\t    2",
        });
        assert!(preview("fs_edit", &args, &c).unwrap().contains("+    2"));
        execute("fs_edit", &args, &c, &CancelToken::new()).await.unwrap();
        assert_eq!(c.workspace.read_file("a.rs").unwrap(), "fn a() {\n    2\n}\n");
    }

    #[test]
    fn line_number_prefix_is_only_stripped_when_every_line_has_one() {
        assert_eq!(strip_line_number_prefix("  12\tfoo\n  13\tbar").as_deref(), Some("foo\nbar"));
        assert_eq!(strip_line_number_prefix("  12\tfoo\nbar"), None);
        assert_eq!(strip_line_number_prefix("foo"), None);
        // Text that is really in the file is never rewritten.
        let (old, _) = strip_copied_line_numbers("1\tid", "1\tid".into(), String::new());
        assert_eq!(old, "1\tid");
    }

    #[test]
    fn long_command_output_keeps_start_and_end() {
        let body = format!("START{}END", "é".repeat(MAX_RESULT_FOR_MODEL));
        let out = truncate_middle_for_model(&body);
        assert!(out.len() < MAX_RESULT_FOR_MODEL + 100);
        assert!(out.starts_with("START") && out.ends_with("END"));
        assert!(out.contains("bytes omitted"));
        assert_eq!(truncate_middle_for_model("short"), "short");
    }

    #[tokio::test]
    async fn multi_edit_is_all_or_nothing() {
        let (_t, c) = ctx();
        c.workspace.write_file("m.rs", "let a = 1;\nlet b = 2;\n").unwrap();
        let run = |args: Value| {
            let c = c.clone();
            async move { execute("fs_multi_edit", &args, &c, &CancelToken::new()).await }
        };
        let out = run(json!({"path": "m.rs", "edits": [
            {"old_string": "let a = 1;", "new_string": "let a = 10;"},
            {"old_string": "let b = 2;", "new_string": "let b = 20;"}
        ]}))
        .await
        .unwrap();
        assert!(out.summary.contains("2 edit(s)"));
        assert_eq!(c.workspace.read_file("m.rs").unwrap(), "let a = 10;\nlet b = 20;\n");

        let err = run(json!({"path": "m.rs", "edits": [
            {"old_string": "let a = 10;", "new_string": "let a = 0;"},
            {"old_string": "missing", "new_string": "x"}
        ]}))
        .await
        .err()
        .unwrap();
        assert!(format!("{err:#}").contains("edit 2 of 2 failed"));
        assert_eq!(c.workspace.read_file("m.rs").unwrap(), "let a = 10;\nlet b = 20;\n", "nothing applied");
        assert!(matches!(
            classify("fs_multi_edit", &json!({"path": "m.rs", "edits": []}), &c).unwrap(),
            Action::EditFile { .. }
        ));
    }

    #[tokio::test]
    async fn search_context_todo_and_fetch_classification() {
        let (_t, c) = ctx();
        c.workspace.write_file("s.txt", "alpha\nBeta\ngamma\n").unwrap();
        let out = execute(
            "fs_search",
            &json!({"query": "beta", "case_insensitive": true, "context_lines": 1}),
            &c,
            &CancelToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(out.result_for_model, "s.txt-1- alpha\ns.txt:2: Beta\ns.txt-3- gamma");

        let out = execute(
            "todo_write",
            &json!({"todos": [{"content": "a", "status": "in_progress"}]}),
            &c,
            &CancelToken::new(),
        )
        .await
        .unwrap();
        assert!(out.result_for_model.contains("[>] a"));

        let a = classify("web_fetch", &json!({"url": " https://example.com "}), &c).unwrap();
        assert!(matches!(a, Action::FetchUrl { ref url } if url == "https://example.com"));
        let names: Vec<String> = tool_specs(true, None).into_iter().map(|t| t.name).collect();
        for t in ["web_fetch", "todo_write"] {
            assert!(names.iter().any(|n| n == t), "{t} offered in read-only modes");
        }
        assert!(!names.iter().any(|n| n == "fs_multi_edit"));
        assert!(tool_rejection(&profile(AgentMode::Plan, None), "fs_multi_edit").is_some());
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
