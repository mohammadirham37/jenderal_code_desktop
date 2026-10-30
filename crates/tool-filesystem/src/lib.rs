//! Filesystem tools for the agent runtime: read, write, edit, search,
//! and checkpoint-based recovery.
//!
//! # Path safety
//!
//! Every path coming from the model or the UI goes through
//! [`Workspace::resolve`], which:
//! - accepts absolute paths only when they are inside the workspace root,
//! - lexical-normalizes `.`/`..` components for files that do not exist yet,
//! - canonicalizes existing paths (resolving symlinks) and rejects any result
//!   that escapes the workspace root.
//!
//! This is the single choke point used by both the permission engine and the
//! executor, so a symlink inside the workspace pointing outside cannot be
//! used to read or write outside files.

mod edit;

pub use edit::{plan_edit, MatchStrategy, PlannedEdit};

use anyhow::{anyhow, bail, Context, Result};
use glob::Pattern;
use ignore::WalkBuilder;
use jcode_common::{FileEntry, SearchHit};
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

pub const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_WRITE_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_SEARCH_RESULTS: usize = 100;
pub const MAX_SEARCH_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// Directories that are always skipped for listing/search/context purposes
/// (they are build artifacts or VCS internals, almost never useful context).
pub const IGNORED_DIRS: &[&str] = &[
    "node_modules", ".git", "target", "dist", "build", ".next", ".nuxt",
    ".venv", "venv", "__pycache__", ".cache", ".turbo", ".gradle", "Pods",
];

pub fn is_ignored_dir(name: &str) -> bool {
    IGNORED_DIRS.contains(&name)
}

/// Maximum total size of a directory snapshot taken before deleting it.
pub const MAX_DIR_SNAPSHOT_BYTES: u64 = 10 * 1024 * 1024;
/// Maximum number of files in a directory snapshot.
pub const MAX_DIR_SNAPSHOT_FILES: usize = 5_000;

/// True when a file name looks like it holds secrets. Mirrors
/// `jcode_permissions::is_sensitive_path` (this crate does not depend on
/// the permissions crate) and is used to keep such files out of workspace
/// search results.
pub fn is_sensitive_name(path: &Path) -> bool {
    const NAMES: &[&str] = &[
        ".env", ".env.local", ".env.development", ".env.production", ".env.test",
        "id_rsa", "id_dsa", "id_ecdsa", "id_ed25519", ".git-credentials", ".npmrc",
        ".pypirc", ".netrc", "credentials.json", "secrets.yaml", "secrets.yml", "secrets.json",
    ];
    const EXTENSIONS: &[&str] = &["pem", "key", "p12", "pfx", "keystore"];
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if NAMES.iter().any(|p| name == *p || name.starts_with(&format!("{p}."))) {
        return true;
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
            return true;
        }
    }
    if name.starts_with(".env.") || name.starts_with("secrets.") {
        return true;
    }
    name.contains("secret") || name.contains("credential")
}

/// `exists()` that does not follow a final symlink (a dangling link
/// "exists" here).
fn lexists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

#[cfg(unix)]
fn no_follow(opts: &mut fs::OpenOptions) -> &mut fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    opts.custom_flags(libc::O_NOFOLLOW)
}

#[cfg(not(unix))]
fn no_follow(opts: &mut fs::OpenOptions) -> &mut fs::OpenOptions {
    opts
}

/// Write `content` to `abs` without ever following a symlink at `abs`.
///
/// - Existing path: must be a regular file (symlinks are refused).
/// - New path: created with `create_new` (O_CREAT|O_EXCL), which fails
///   instead of following a symlink planted at that name.
pub fn write_no_follow(abs: &Path, content: &[u8]) -> Result<()> {
    let mut file = match fs::symlink_metadata(abs) {
        Ok(m) if m.file_type().is_symlink() => {
            bail!("refusing to write through a symlink: {}", abs.display())
        }
        Ok(m) if m.is_dir() => bail!("path is a directory: {}", abs.display()),
        // O_NOFOLLOW closes the race between the check above and the open:
        // a symlink swapped in meanwhile makes the open fail instead of
        // being followed.
        Ok(_) => no_follow(fs::OpenOptions::new().write(true).truncate(true))
            .open(abs)
            .with_context(|| format!("opening {}", abs.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            no_follow(fs::OpenOptions::new().write(true).create_new(true))
                .open(abs)
                .with_context(|| format!("creating {}", abs.display()))?
        }
        Err(e) => return Err(anyhow::Error::new(e).context(format!("inspecting {}", abs.display()))),
    };
    file.write_all(content)
        .with_context(|| format!("writing {}", abs.display()))?;
    Ok(())
}

/// A workspace rooted at a canonical directory on disk.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditOutcome {
    pub path: String,
    pub occurrences: usize,
    pub applied: bool,
    pub diff: String,
    /// How `old_string` was matched (edits only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<MatchStrategy>,
}

impl Workspace {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        if !root.is_dir() {
            bail!("workspace root is not a directory: {}", root.display());
        }
        let canonical = root
            .canonicalize()
            .with_context(|| format!("canonicalizing {}", root.display()))?;
        Ok(Self { root: canonical })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn rel_string(&self, abs: &Path) -> String {
        abs.strip_prefix(&self.root)
            .unwrap_or(abs)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Resolve a user/model-provided path against the workspace root with
    /// full traversal and symlink-escape protection. Works for both existing
    /// and not-yet-existing paths (e.g. new files about to be written).
    pub fn resolve(&self, input: &str) -> Result<PathBuf> {
        let input = input.trim();
        if input.is_empty() {
            bail!("empty path");
        }
        // The model sometimes wraps paths in backticks or quotes.
        let input = input.trim_matches(|c| c == '`' || c == '"' || c == '\'');

        let candidate = Path::new(input);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };

        // Fast path: the path exists (as anything, including a symlink) ->
        // canonicalize (resolves every symlink component) and check
        // containment. A dangling symlink fails to canonicalize and is
        // rejected rather than treated as a "new file" (writing to it would
        // create the link target, possibly outside the workspace).
        if lexists(&joined) {
            let canon = joined.canonicalize().map_err(|e| {
                anyhow!("cannot resolve {input} (dangling or broken symlink?): {e}")
            })?;
            if canon.starts_with(&self.root) {
                return Ok(canon);
            }
            bail!(
                "path escapes the workspace: {} resolves to {}",
                input,
                canon.display()
            );
        }

        // Slow path: file does not exist yet. Canonicalize the deepest
        // existing ancestor, then append the remaining components.
        let mut existing = joined.clone();
        let mut tail: Vec<String> = Vec::new();
        while !lexists(&existing) {
            let name = match existing.file_name() {
                Some(n) => n.to_string_lossy().to_string(),
                None => bail!("invalid path: {input}"),
            };
            tail.push(name);
            existing = existing
                .parent()
                .ok_or_else(|| anyhow!("invalid path: {input}"))?
                .to_path_buf();
        }
        let canon = existing.canonicalize().map_err(|e| {
            anyhow!("cannot resolve {input} (dangling or broken symlink?): {e}")
        })?;
        if !canon.starts_with(&self.root) {
            bail!("path escapes the workspace: {input}");
        }
        let mut resolved = canon;
        for part in tail.iter().rev() {
            // Reject parent-directory components in the new part.
            if part == ".." || part == "." {
                bail!("invalid path component in new path: {input}");
            }
            resolved.push(part);
        }
        if resolved.starts_with(&self.root) {
            Ok(resolved)
        } else {
            bail!("path escapes the workspace: {input}")
        }
    }

    fn ensure_parent(&self, abs: &Path) -> Result<PathBuf> {
        if let Some(parent) = abs.parent() {
            if !parent.exists() {
                if !parent.starts_with(&self.root) {
                    bail!("parent directory escapes workspace");
                }
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
        }
        Ok(abs.to_path_buf())
    }

    pub fn read_file(&self, rel: &str) -> Result<String> {
        let abs = self.resolve(rel)?;
        let meta = fs::metadata(&abs)?;
        if meta.len() > MAX_READ_BYTES {
            bail!(
                "file too large to read ({} bytes > {} byte limit)",
                meta.len(),
                MAX_READ_BYTES
            );
        }
        let bytes = fs::read(&abs).with_context(|| format!("reading {}", abs.display()))?;
        if bytes[..bytes.len().min(8192)].contains(&0) {
            bail!(
                "binary file, not shown: {} ({} bytes)",
                self.rel_string(&abs),
                bytes.len()
            );
        }
        match String::from_utf8(bytes) {
            Ok(s) => Ok(s),
            Err(_) => bail!("file is not valid UTF-8 text: {}", self.rel_string(&abs)),
        }
    }

    pub fn write_file(&self, rel: &str, content: &str) -> Result<EditOutcome> {
        if content.len() as u64 > MAX_WRITE_BYTES {
            bail!("content too large ({} bytes)", content.len());
        }
        let abs = self.resolve(rel)?;
        if abs == self.root {
            bail!("refusing to overwrite the workspace root");
        }
        let existed_before = abs.exists();
        let before = if existed_before {
            self.read_file(&self.rel_string(&abs)).unwrap_or_default()
        } else {
            String::new()
        };
        self.ensure_parent(&abs)?;
        write_no_follow(&abs, content.as_bytes())?;
        let diff = unified_diff(&before, content, &self.rel_string(&abs));
        Ok(EditOutcome {
            path: self.rel_string(&abs),
            occurrences: if existed_before { 1 } else { 0 },
            applied: true,
            diff,
            strategy: None,
        })
    }

    /// Replace `old_string` with `new_string`. The match must be unique
    /// unless `replace_all` is set — same semantics as common agent CLIs.
    pub fn edit_file(
        &self,
        rel: &str,
        old_string: &str,
        new_string: &str,
        replace_all: bool,
    ) -> Result<EditOutcome> {
        let abs = self.resolve(rel)?;
        let rel = self.rel_string(&abs);
        let before = self.read_file(&rel)?;
        let planned = plan_edit(&before, old_string, new_string, replace_all, &rel)?;
        write_no_follow(&abs, planned.after.as_bytes())?;
        let diff = unified_diff(&before, &planned.after, &rel);
        Ok(EditOutcome {
            path: rel,
            occurrences: planned.occurrences,
            applied: true,
            diff,
            strategy: Some(planned.strategy),
        })
    }

    pub fn delete_path(&self, rel: &str) -> Result<String> {
        let abs = self.resolve(rel)?;
        if abs == self.root {
            bail!("refusing to delete the workspace root");
        }
        if abs.is_dir() {
            fs::remove_dir_all(&abs).with_context(|| format!("deleting {}", abs.display()))?;
        } else if abs.exists() {
            fs::remove_file(&abs).with_context(|| format!("deleting {}", abs.display()))?;
        } else {
            bail!("path does not exist: {}", self.rel_string(&abs));
        }
        Ok(self.rel_string(&abs))
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<String> {
        let src = self.resolve(from)?;
        let dst = self.resolve(to)?;
        if !src.exists() {
            bail!("source does not exist: {from}");
        }
        self.ensure_parent(&dst)?;
        fs::rename(&src, &dst).with_context(|| format!("renaming {from} -> {to}"))?;
        Ok(self.rel_string(&dst))
    }

    /// List a directory (one level). `show_hidden` toggles dotfiles.
    pub fn list_dir(&self, rel: &str, show_hidden: bool) -> Result<Vec<FileEntry>> {
        let abs = if rel.trim().is_empty() || rel.trim() == "." {
            self.root.clone()
        } else {
            self.resolve(rel)?
        };
        if !abs.is_dir() {
            bail!("not a directory: {}", self.rel_string(&abs));
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(&abs).with_context(|| abs.display().to_string())? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') && name != ".github" {
                continue;
            }
            // OS metadata, never useful to open.
            if name == ".DS_Store" {
                continue;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir && is_ignored_dir(&name) {
                continue;
            }
            let size = entry.metadata().ok().map(|m| m.len());
            entries.push(FileEntry {
                path: self.rel_string(&entry.path()),
                name,
                is_dir,
                size,
            });
        }
        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });
        Ok(entries)
    }

    /// Render a depth-limited tree of the project for chat context.
    pub fn tree_string(&self, max_depth: usize, max_entries: usize) -> String {
        let mut out = String::new();
        let mut count = 0usize;
        self.walk_tree(&self.root, 0, max_depth, max_entries, &mut out, &mut count, &mut HashSet::new());
        if count >= max_entries {
            out.push_str("... (truncated)\n");
        }
        out
    }

    fn walk_tree(
        &self,
        dir: &Path,
        depth: usize,
        max_depth: usize,
        max_entries: usize,
        out: &mut String,
        count: &mut usize,
        visited: &mut HashSet<PathBuf>,
    ) {
        if depth > max_depth || *count >= max_entries {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else { return };
        let mut items: Vec<_> = entries.flatten().collect();
        items.sort_by_key(|e| (!e.file_type().map(|t| t.is_dir()).unwrap_or(false), e.file_name()));
        for entry in items {
            if *count >= max_entries {
                return;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && name != ".github" {
                continue;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir && is_ignored_dir(&name) {
                continue;
            }
            out.push_str(&"  ".repeat(depth));
            out.push_str(&name);
            if is_dir {
                out.push('/');
            }
            out.push('\n');
            *count += 1;
            if is_dir {
                // Protect against symlink loops.
                let canon = entry.path().canonicalize().unwrap_or_else(|_| entry.path());
                if visited.insert(canon) {
                    self.walk_tree(&entry.path(), depth + 1, max_depth, max_entries, out, count, visited);
                }
            }
        }
    }

    /// Content search across the workspace, respecting .gitignore.
    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>> {
        let pattern = if query.is_regex { query.query.clone() } else { regex::escape(&query.query) };
        let re = if query.is_regex || query.case_insensitive {
            Some(
                regex::RegexBuilder::new(&pattern)
                    .case_insensitive(query.case_insensitive)
                    .build()
                    .context("invalid regex")?,
            )
        } else {
            None
        };
        let literal = query.query.clone();
        let max_results = query.max_results.unwrap_or(MAX_SEARCH_RESULTS).clamp(1, 500);
        let context = query.context_lines.min(5);
        let start = match query.path.as_deref().filter(|p| !p.trim().is_empty()) {
            Some(p) => self.resolve(p)?,
            None => self.root.clone(),
        };
        let include = query
            .include_glob
            .as_ref()
            .map(|g| Pattern::new(g).context("invalid include glob"))
            .transpose()?;
        let walker = WalkBuilder::new(&start)
            .hidden(true)
            .git_ignore(true)
            .git_exclude(true)
            // Apply .gitignore even when the folder is not a git repo yet.
            .require_git(false)
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                !e.file_type().map(|t| t.is_dir()).unwrap_or(false) || !is_ignored_dir(&name)
            })
            .max_filesize(Some(MAX_SEARCH_FILE_BYTES))
            .build();
        let mut hits = Vec::new();
        for entry in walker.flatten() {
            if hits.len() >= max_results {
                break;
            }
            let path = entry.path();
            // Never follow symlinks (they may point outside the workspace)
            // and never surface secret-looking files.
            if entry.path_is_symlink()
                || !entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                || is_sensitive_name(path)
            {
                continue;
            }
            let rel = self.rel_string(path);
            if let Some(pat) = &include {
                if !pat.matches_path(&PathBuf::from(&rel)) {
                    continue;
                }
            }
            let Ok(content) = fs::read_to_string(path) else { continue };
            let lines: Vec<&str> = content.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let matched = match &re {
                    Some(re) => re.is_match(line),
                    None => line.contains(&literal),
                };
                if matched {
                    let around = |r: std::ops::Range<usize>| lines[r].iter().map(|l| clip_line(l, 300)).collect();
                    hits.push(SearchHit {
                        path: rel.clone(),
                        line_number: Some(i as u64 + 1),
                        line: clip_line(line, 300),
                        before: around(i.saturating_sub(context)..i),
                        after: around(i + 1..(i + 1 + context).min(lines.len())),
                    });
                    if hits.len() >= max_results {
                        break;
                    }
                }
            }
        }
        Ok(hits)
    }

    /// List files whose paths match a glob (e.g. `**/*.rs`).
    pub fn glob_files(&self, pattern: &str, max: usize) -> Result<Vec<String>> {
        let pat = Pattern::new(pattern).context("invalid glob")?;
        let walker = WalkBuilder::new(&self.root)
            .hidden(true)
            .git_ignore(true)
            .require_git(false)
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                !e.file_type().map(|t| t.is_dir()).unwrap_or(false) || !is_ignored_dir(&name)
            })
            .build();
        let mut out = Vec::new();
        for entry in walker.flatten() {
            if out.len() >= max {
                break;
            }
            let path = entry.path();
            if entry.path_is_symlink() {
                // Only list links that resolve to a file inside the root.
                match path.canonicalize() {
                    Ok(c) if c.starts_with(&self.root) && c.is_file() => {}
                    _ => continue,
                }
            } else if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let rel = self.rel_string(path);
            if pat.matches_path(&PathBuf::from(&rel)) {
                out.push(rel);
            }
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    #[serde(default)]
    pub is_regex: bool,
    #[serde(default)]
    pub include_glob: Option<String>,
    #[serde(default)]
    pub case_insensitive: bool,
    /// Lines of context before and after each match (capped at 5).
    #[serde(default)]
    pub context_lines: usize,
    /// Directory or file (workspace-relative) to search within.
    #[serde(default)]
    pub path: Option<String>,
    /// Result cap (default [`MAX_SEARCH_RESULTS`], at most 500).
    #[serde(default)]
    pub max_results: Option<usize>,
}

/// At most `max` bytes of `line`, cut on a char boundary.
fn clip_line(line: &str, max: usize) -> String {
    if line.len() <= max {
        return line.to_string();
    }
    let mut end = max;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_string()
}

/// Render a unified diff between two file contents.
pub fn unified_diff(old: &str, new: &str, path: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = diff
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string();
    if out.is_empty() {
        out = format!("(no changes in {path})");
    }
    out
}

/// Normalize a path lexically (remove `.` and resolve `..` without touching
/// the filesystem). Used for safety checks on not-yet-existing paths.
pub fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Checkpoints
// ---------------------------------------------------------------------------

/// A checkpoint stores the original content of files before the agent
/// modifies them, so any agent edit can be rolled back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointEntry {
    pub id: String,
    pub session_id: String,
    pub rel_path: String,
    /// Absolute path of the stored snapshot (inside the app data dir).
    /// For directories this is a directory mirroring the original tree.
    pub stored_path: String,
    /// Whether the file existed before the agent action.
    pub existed: bool,
    /// Whether the snapshot is of a directory (taken before deleting it).
    #[serde(default)]
    pub is_dir: bool,
    pub created_at: i64,
}

pub struct CheckpointStore {
    base: PathBuf,
}

/// Normalize a workspace-relative checkpoint key: forward slashes, no `.`
/// components, no leading `./`. Rejects absolute paths and `..`.
fn checkpoint_rel(rel: &str) -> Result<PathBuf> {
    let p = Path::new(rel);
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            _ => bail!("invalid checkpoint path: {rel}"),
        }
    }
    if out.as_os_str().is_empty() {
        bail!("invalid checkpoint path: {rel}");
    }
    Ok(out)
}

/// Resolve a restore target inside the workspace. The parent directory
/// must resolve inside the root (no symlink escape); the final component
/// is returned unresolved so callers can detect and refuse a symlink there.
fn restore_target(ws: &Workspace, rel: &str) -> Result<PathBuf> {
    // Older manifests may hold absolute paths; accept them only when they
    // are under the workspace root.
    let rel_owned;
    let rel = if Path::new(rel).is_absolute() {
        rel_owned = Path::new(rel)
            .strip_prefix(ws.root())
            .map_err(|_| anyhow!("checkpoint path is outside the workspace: {rel}"))?
            .to_string_lossy()
            .to_string();
        rel_owned.as_str()
    } else {
        rel
    };
    let rel = checkpoint_rel(rel)?;
    let name = rel
        .file_name()
        .ok_or_else(|| anyhow!("invalid checkpoint path"))?
        .to_os_string();
    let parent = match rel.parent() {
        Some(p) if !p.as_os_str().is_empty() => ws.resolve(&p.to_string_lossy())?,
        _ => ws.root().to_path_buf(),
    };
    if !parent.starts_with(ws.root()) {
        bail!("checkpoint path escapes the workspace: {}", rel.display());
    }
    Ok(parent.join(name))
}

/// Recursively collect regular files under `dir` (symlinks are skipped),
/// enforcing size and count caps.
fn collect_dir_files(dir: &Path, out: &mut Vec<(PathBuf, u64)>, total: &mut u64) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let path = entry.path();
        if ft.is_symlink() {
            continue;
        } else if ft.is_dir() {
            collect_dir_files(&path, out, total)?;
        } else if ft.is_file() {
            let len = entry.metadata()?.len();
            *total += len;
            out.push((path, len));
            if *total > MAX_DIR_SNAPSHOT_BYTES {
                bail!(
                    "directory is too large to checkpoint (> {} MB); refusing to delete it without a backup",
                    MAX_DIR_SNAPSHOT_BYTES / (1024 * 1024)
                );
            }
            if out.len() > MAX_DIR_SNAPSHOT_FILES {
                bail!(
                    "directory has too many files to checkpoint (> {MAX_DIR_SNAPSHOT_FILES}); refusing to delete it without a backup"
                );
            }
        }
    }
    Ok(())
}

impl CheckpointStore {
    pub fn new(base: impl AsRef<Path>) -> Result<Self> {
        let base = base.as_ref().join("checkpoints");
        fs::create_dir_all(&base)?;
        Ok(Self { base })
    }

    fn session_dir(&self, session_id: &str) -> PathBuf {
        // Keep it filesystem-safe.
        let safe: String = session_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
            .collect();
        self.base.join(safe)
    }

    fn manifest_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("manifest.json")
    }

    fn load_manifest(&self, session_id: &str) -> Vec<CheckpointEntry> {
        fs::read_to_string(self.manifest_path(session_id))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save_manifest(&self, session_id: &str, entries: &[CheckpointEntry]) -> Result<()> {
        let dir = self.session_dir(session_id);
        fs::create_dir_all(&dir)?;
        // Write-then-rename so a crash never leaves a truncated manifest
        // (which would drop every earlier snapshot).
        let tmp = dir.join("manifest.json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(entries)?)?;
        fs::rename(&tmp, dir.join("manifest.json"))?;
        Ok(())
    }

    /// Snapshot the current on-disk state of `rel_path` inside
    /// `workspace_root`. Only the FIRST snapshot per (session, path) is
    /// kept, so restoring always returns to the pre-agent state rather than
    /// an intermediate one. Directories are snapshotted recursively
    /// (regular files only, capped at [`MAX_DIR_SNAPSHOT_BYTES`]).
    pub fn snapshot(
        &self,
        session_id: &str,
        workspace_root: &Path,
        rel_path: &str,
    ) -> Result<CheckpointEntry> {
        let ws = Workspace::open(workspace_root)?;
        let abs = ws.resolve(rel_path)?;
        if abs == ws.root() {
            bail!("refusing to checkpoint the workspace root");
        }
        let key = ws.rel_string(&abs);
        let mut entries = self.load_manifest(session_id);
        if let Some(existing) = entries.iter().find(|e| e.rel_path == key) {
            return Ok(existing.clone());
        }
        let id = format!("cp-{:08}", entries.len() + 1);
        let stored = self.session_dir(session_id).join(&id);
        fs::create_dir_all(self.session_dir(session_id))?;
        let meta = fs::symlink_metadata(&abs).ok();
        let existed = meta.is_some();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            let mut files = Vec::new();
            let mut total = 0u64;
            collect_dir_files(&abs, &mut files, &mut total)?;
            fs::create_dir_all(&stored)?;
            for (src, _) in files {
                let rel = src.strip_prefix(&abs).unwrap_or(&src);
                let dst = stored.join(rel);
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&src, &dst)
                    .with_context(|| format!("snapshotting {}", src.display()))?;
            }
        } else if existed {
            fs::copy(&abs, &stored).with_context(|| format!("snapshotting {}", abs.display()))?;
        } else {
            fs::write(&stored, b"")?;
        }
        let entry = CheckpointEntry {
            id,
            session_id: session_id.into(),
            rel_path: key,
            stored_path: stored.to_string_lossy().to_string(),
            existed,
            is_dir,
            created_at: jcode_common::now_ms(),
        };
        entries.push(entry.clone());
        self.save_manifest(session_id, &entries)?;
        Ok(entry)
    }

    pub fn list(&self, session_id: &str) -> Vec<CheckpointEntry> {
        self.load_manifest(session_id)
    }

    /// Restore every snapshotted path to its pre-agent state. Files that
    /// did not exist before are deleted (again).
    ///
    /// Entries are restored newest-first so the oldest snapshot of any
    /// overlapping path (e.g. a file and later its parent directory) wins.
    /// Targets are resolved inside `workspace_root` and symlinks are never
    /// written through. Every entry is attempted; failures are reported
    /// together at the end.
    pub fn restore_all(&self, session_id: &str, workspace_root: &Path) -> Result<usize> {
        let ws = Workspace::open(workspace_root)?;
        let entries = self.load_manifest(session_id);
        let mut restored = 0;
        let mut errors: Vec<String> = Vec::new();
        for e in entries.iter().rev() {
            match self.restore_entry(&ws, e) {
                Ok(()) => restored += 1,
                Err(err) => errors.push(format!("{}: {err:#}", e.rel_path)),
            }
        }
        if !errors.is_empty() {
            bail!(
                "restored {restored} of {} checkpoint(s); failures: {}",
                entries.len(),
                errors.join("; ")
            );
        }
        Ok(restored)
    }

    fn restore_entry(&self, ws: &Workspace, e: &CheckpointEntry) -> Result<()> {
        let target = restore_target(ws, &e.rel_path)?;
        if !e.existed {
            match fs::symlink_metadata(&target) {
                // A symlink is removed itself; its target is never touched.
                Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {
                    fs::remove_dir_all(&target)?
                }
                Ok(_) => fs::remove_file(&target)?,
                Err(_) => {}
            }
            return Ok(());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if e.is_dir {
            let stored = PathBuf::from(&e.stored_path);
            let mut files = Vec::new();
            let mut total = 0u64;
            if stored.is_dir() {
                collect_dir_files(&stored, &mut files, &mut total)?;
            }
            match fs::symlink_metadata(&target) {
                Ok(m) if m.file_type().is_symlink() => {
                    bail!("refusing to restore through a symlink: {}", e.rel_path)
                }
                Ok(m) if !m.is_dir() => fs::remove_file(&target)?,
                Ok(_) => {}
                Err(_) => fs::create_dir_all(&target)?,
            }
            let target_rel = ws.rel_string(&target);
            for (src, _) in files {
                let rel = src.strip_prefix(&stored).unwrap_or(&src);
                let rel_str = format!("{}/{}", target_rel, rel.to_string_lossy().replace('\\', "/"));
                let dst = restore_target(ws, &rel_str)?;
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                write_no_follow(&dst, &fs::read(&src)?)?;
            }
            return Ok(());
        }
        if let Ok(m) = fs::symlink_metadata(&target) {
            if m.is_dir() && !m.file_type().is_symlink() {
                bail!("a directory now exists at {}", e.rel_path);
            }
        }
        let bytes = fs::read(&e.stored_path)
            .with_context(|| format!("reading snapshot {}", e.stored_path))?;
        write_no_follow(&target, &bytes)
            .with_context(|| format!("restoring {}", target.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let w = Workspace::open(dir.path()).unwrap();
        (dir, w)
    }

    #[test]
    fn resolve_accepts_relative_and_absolute_inside() {
        let (_t, w) = ws();
        w.write_file("src/main.rs", "fn main() {}").unwrap();
        let a = w.resolve("src/main.rs").unwrap();
        let b = w.resolve(a.to_str().unwrap()).unwrap();
        assert_eq!(a, b);
        assert!(a.starts_with(w.root()));
    }

    #[test]
    fn resolve_rejects_traversal() {
        let (_t, w) = ws();
        assert!(w.resolve("../outside.txt").is_err());
        assert!(w.resolve("src/../../outside.txt").is_err());
        assert!(w.resolve("/etc/passwd").is_err());
        assert!(w.resolve("new/nested/file.txt").is_ok()); // new file is fine
    }

    #[test]
    fn resolve_rejects_symlink_escape() {
        let (_t, w) = ws();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("secret.txt");
        std::fs::write(&target, "top secret").unwrap();
        let link = w.root().join("link.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        #[cfg(not(unix))]
        let _ = (&target, &link);
        #[cfg(unix)]
        {
            assert!(w.resolve("link.txt").is_err());
            // Symlink to a path inside the workspace is fine.
            w.write_file("inside.txt", "x").unwrap();
            let link2 = w.root().join("link2.txt");
            std::os::unix::fs::symlink(w.root().join("inside.txt"), &link2).unwrap();
            assert!(w.resolve("link2.txt").is_ok());
        }
    }

    #[test]
    fn write_read_edit_roundtrip() {
        let (_t, w) = ws();
        let out = w.write_file("a.txt", "hello world\n").unwrap();
        assert!(out.diff.contains("+hello world"));
        assert_eq!(w.read_file("a.txt").unwrap(), "hello world\n");

        let edit = w
            .edit_file("a.txt", "hello world", "goodbye world", false)
            .unwrap();
        assert_eq!(edit.occurrences, 1);
        assert!(w.read_file("a.txt").unwrap().contains("goodbye"));

        assert!(w.edit_file("a.txt", "nonexistent", "x", false).is_err());
    }

    #[test]
    fn edit_requires_unique_match() {
        let (_t, w) = ws();
        w.write_file("dup.txt", "a\na\na\n").unwrap();
        assert!(w.edit_file("dup.txt", "a", "b", false).is_err());
        let e = w.edit_file("dup.txt", "a", "b", true).unwrap();
        assert_eq!(e.occurrences, 3);
    }

    #[test]
    fn search_supports_case_context_path_and_limit() {
        let (_t, w) = ws();
        w.write_file("src/a.rs", "one\nTwo Needle\nthree\nfour\n").unwrap();
        w.write_file("other/b.rs", "needle\n").unwrap();
        w.write_file("src/long.txt", &format!("{}needle\n", "é".repeat(400))).unwrap();
        let q = |f: fn(&mut SearchQuery)| {
            let mut q = SearchQuery { query: "needle".into(), ..Default::default() };
            f(&mut q);
            w.search(&q).unwrap()
        };
        // Case-sensitive literal by default.
        assert!(q(|_| {}).iter().all(|h| h.path != "src/a.rs"));
        let hits = q(|q| {
            q.case_insensitive = true;
            q.context_lines = 1;
            q.path = Some("src".into());
        });
        let a = hits.iter().find(|h| h.path == "src/a.rs").unwrap();
        assert_eq!(a.line_number, Some(2));
        assert_eq!(a.before, vec!["one"]);
        assert_eq!(a.after, vec!["three"]);
        assert!(hits.iter().all(|h| h.path.starts_with("src/")));
        // Long multi-byte lines are clipped without panicking.
        assert!(hits.iter().any(|h| h.path == "src/long.txt" && h.line.len() <= 300));
        assert_eq!(q(|q| { q.case_insensitive = true; q.max_results = Some(1); }).len(), 1);
    }

    #[test]
    fn search_respects_gitignore_and_finds_matches() {
        let (t, w) = ws();
        w.write_file("src/lib.rs", "fn find_me() {}\n").unwrap();
        w.write_file("docs/note.md", "nothing here\n").unwrap();
        let gitignore = t.path().join(".gitignore");
        std::fs::write(gitignore, "docs/\n").unwrap();
        let hits = w
            .search(&SearchQuery {
                query: "find_me".into(),
                is_regex: false,
                include_glob: None,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "src/lib.rs");

        let hits = w
            .search(&SearchQuery {
                query: "nothing".into(),
                is_regex: false,
                include_glob: None,
                ..Default::default()
            })
            .unwrap();
        assert!(hits.is_empty(), "gitignored docs/ should be skipped");
    }

    #[test]
    fn glob_files_matches() {
        let (_t, w) = ws();
        w.write_file("src/a.rs", "").unwrap();
        w.write_file("src/b.ts", "").unwrap();
        let rs = w.glob_files("**/*.rs", 10).unwrap();
        assert_eq!(rs, vec!["src/a.rs".to_string()]);
    }

    #[test]
    fn delete_and_rename() {
        let (_t, w) = ws();
        w.write_file("x.txt", "x").unwrap();
        assert!(w.delete_path("x.txt").is_ok());
        assert!(w.delete_path("x.txt").is_err());
        w.write_file("y.txt", "y").unwrap();
        w.rename("y.txt", "z.txt").unwrap();
        assert!(w.read_file("z.txt").is_ok());
    }

    #[test]
    fn checkpoint_snapshot_and_restore() {
        let (t, w) = ws();
        let store_base = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(store_base.path()).unwrap();
        w.write_file("p.txt", "original").unwrap();
        store.snapshot("sess1", t.path(), "p.txt").unwrap();
        // Second snapshot of same file is a no-op returning same entry.
        let e2 = store.snapshot("sess1", t.path(), "p.txt").unwrap();
        assert_eq!(store.list("sess1").len(), 1);
        let _ = e2;

        // Snapshot BEFORE the agent acts — including files it will create.
        store.snapshot("sess1", t.path(), "new.txt").unwrap();
        w.write_file("p.txt", "agent-changed").unwrap();
        w.write_file("new.txt", "created-by-agent").unwrap();

        let restored = store.restore_all("sess1", t.path()).unwrap();
        assert_eq!(restored, 2);
        assert_eq!(w.read_file("p.txt").unwrap(), "original");
        assert!(!t.path().join("new.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_write_is_rejected() {
        let (_t, w) = ws();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("created-by-attack");
        std::os::unix::fs::symlink(&target, w.root().join("notes.md")).unwrap();
        assert!(w.resolve("notes.md").is_err());
        assert!(w.write_file("notes.md", "pwned").is_err());
        assert!(!target.exists(), "write must not create a file outside the workspace");
        // Dangling symlinked directory as an intermediate component.
        std::os::unix::fs::symlink(outside.path().join("nodir"), w.root().join("d")).unwrap();
        assert!(w.write_file("d/x.txt", "pwned").is_err());
        assert!(!outside.path().join("nodir").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_outside_rejected_for_all_ops() {
        let (_t, w) = ws();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("f.txt"), "keep").unwrap();
        std::os::unix::fs::symlink(outside.path().join("f.txt"), w.root().join("l.txt")).unwrap();
        std::os::unix::fs::symlink(outside.path(), w.root().join("ldir")).unwrap();
        assert!(w.write_file("l.txt", "x").is_err());
        assert!(w.edit_file("l.txt", "keep", "x", false).is_err());
        assert!(w.read_file("l.txt").is_err());
        assert!(w.delete_path("l.txt").is_err());
        assert!(w.write_file("ldir/new.txt", "x").is_err());
        assert!(w.rename("l.txt", "m.txt").is_err());
        assert_eq!(std::fs::read_to_string(outside.path().join("f.txt")).unwrap(), "keep");
        assert!(!outside.path().join("new.txt").exists());
    }

    #[test]
    fn write_no_follow_refuses_symlink() {
        #[cfg(unix)]
        {
            let (_t, w) = ws();
            w.write_file("real.txt", "a").unwrap();
            let link = w.root().join("link.txt");
            std::os::unix::fs::symlink(w.root().join("real.txt"), &link).unwrap();
            assert!(write_no_follow(&link, b"b").is_err());
            assert_eq!(w.read_file("real.txt").unwrap(), "a");
        }
    }

    #[test]
    fn read_reports_binary_files() {
        let (t, w) = ws();
        std::fs::write(t.path().join("bin.dat"), [0u8, 159, 146, 150, 0, 1]).unwrap();
        let err = w.read_file("bin.dat").unwrap_err().to_string();
        assert!(err.contains("binary"), "{err}");
    }

    #[test]
    fn search_skips_sensitive_files_and_symlinks() {
        let (t, w) = ws();
        w.write_file("src/a.rs", "needle\n").unwrap();
        // `.env` is hidden anyway; also test non-hidden secret files.
        std::fs::write(t.path().join(".env"), "needle=1\n").unwrap();
        std::fs::write(t.path().join("server.pem"), "needle\n").unwrap();
        std::fs::write(t.path().join("secrets.toml"), "needle\n").unwrap();
        std::fs::write(t.path().join("credentials.json"), "needle\n").unwrap();
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("o.txt"), "needle\n").unwrap();
            std::os::unix::fs::symlink(outside.path().join("o.txt"), t.path().join("link.txt")).unwrap();
            std::os::unix::fs::symlink(outside.path(), t.path().join("linkdir")).unwrap();
            // Keep the tempdir alive for the search below.
            let hits = w
                .search(&SearchQuery { query: "needle".into(), ..Default::default() })
                .unwrap();
            let paths: Vec<_> = hits.iter().map(|h| h.path.as_str()).collect();
            assert_eq!(paths, vec!["src/a.rs"]);
            let globbed = w.glob_files("**/*.txt", 50).unwrap();
            assert!(globbed.is_empty(), "{globbed:?}");
        }
        assert!(is_sensitive_name(Path::new(".env.local")));
        assert!(is_sensitive_name(Path::new("id_ed25519.pub")));
        assert!(is_sensitive_name(Path::new("a/b/.pypirc")));
        assert!(!is_sensitive_name(Path::new("main.rs")));
    }

    #[test]
    fn checkpoint_keeps_first_snapshot() {
        let (t, w) = ws();
        let base = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(base.path()).unwrap();
        w.write_file("p.txt", "v1").unwrap();
        store.snapshot("s", t.path(), "p.txt").unwrap();
        w.write_file("p.txt", "v2").unwrap();
        // Different spellings of the same path must not create a new snapshot.
        store.snapshot("s", t.path(), "./p.txt").unwrap();
        let abs = w.root().join("p.txt");
        store.snapshot("s", t.path(), abs.to_str().unwrap()).unwrap();
        assert_eq!(store.list("s").len(), 1);
        w.write_file("p.txt", "v3").unwrap();
        store.restore_all("s", t.path()).unwrap();
        assert_eq!(w.read_file("p.txt").unwrap(), "v1");
    }

    #[test]
    fn directory_delete_with_checkpoint_works() {
        let (t, w) = ws();
        let base = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(base.path()).unwrap();
        w.write_file("dir/a.txt", "A1").unwrap();
        w.write_file("dir/sub/b.txt", "B").unwrap();
        // Edit a file first, then delete its whole directory.
        store.snapshot("s", t.path(), "dir/a.txt").unwrap();
        w.write_file("dir/a.txt", "A2").unwrap();
        let e = store.snapshot("s", t.path(), "dir").unwrap();
        assert!(e.is_dir);
        w.delete_path("dir").unwrap();
        assert!(!t.path().join("dir").exists());

        assert_eq!(store.restore_all("s", t.path()).unwrap(), 2);
        // Oldest snapshot wins for a.txt.
        assert_eq!(w.read_file("dir/a.txt").unwrap(), "A1");
        assert_eq!(w.read_file("dir/sub/b.txt").unwrap(), "B");
    }

    #[test]
    fn directory_snapshot_is_size_capped() {
        let (t, w) = ws();
        let base = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(base.path()).unwrap();
        std::fs::create_dir_all(t.path().join("big")).unwrap();
        let chunk = vec![b'x'; 3 * 1024 * 1024];
        for i in 0..4 {
            std::fs::write(t.path().join(format!("big/f{i}")), &chunk).unwrap();
        }
        let err = store.snapshot("s", t.path(), "big").unwrap_err().to_string();
        assert!(err.contains("too large"), "{err}");
        let _ = w;
    }

    #[cfg(unix)]
    #[test]
    fn restore_refuses_to_write_through_symlink() {
        let (t, w) = ws();
        let base = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(base.path()).unwrap();
        let outside = tempfile::tempdir().unwrap();
        w.write_file("p.txt", "orig").unwrap();
        store.snapshot("s", t.path(), "p.txt").unwrap();
        std::fs::remove_file(t.path().join("p.txt")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("victim"), t.path().join("p.txt")).unwrap();
        assert!(store.restore_all("s", t.path()).is_err());
        assert!(!outside.path().join("victim").exists());
    }

    #[test]
    fn tree_string_truncates() {
        let (_t, w) = ws();
        for i in 0..50 {
            w.write_file(&format!("f{i}.txt"), "x").unwrap();
        }
        let tree = w.tree_string(2, 10);
        assert!(tree.contains("f0.txt"));
        assert!(tree.contains("truncated"));
    }
}
