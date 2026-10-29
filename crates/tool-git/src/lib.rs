//! Git operations via the Git CLI (PRD: "Git CLI sebagai implementasi awal").
//!
//! All commands run with `--no-optional-locks` to avoid contending with
//! editors, and `core.quotepath=false` so non-ASCII paths stay readable.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// Validate a branch name before passing it to git. Rejects names that
/// could be interpreted as options or pathspecs (`-x`, `.`) and anything
/// `git check-ref-format --branch` would reject.
pub fn validate_branch_name(name: &str) -> Result<()> {
    let bad = |why: &str| -> Result<()> { anyhow::bail!("invalid branch name {name:?}: {why}") };
    if name.is_empty() {
        return bad("empty");
    }
    if name.starts_with('-') {
        return bad("must not start with '-'");
    }
    if name == "." || name == ".." || name == "@" || name == "HEAD" {
        return bad("reserved name");
    }
    if name.contains("..") {
        return bad("must not contain '..'");
    }
    if name.contains("@{") {
        return bad("must not contain '@{'");
    }
    if name.ends_with(".lock") || name.ends_with('/') || name.ends_with('.') {
        return bad("must not end with '.lock', '/' or '.'");
    }
    if name.starts_with('/') || name.contains("//") {
        return bad("invalid '/' placement");
    }
    if name
        .split('/')
        .any(|c| c.is_empty() || c.starts_with('.') || c.ends_with(".lock"))
    {
        return bad("invalid path component");
    }
    if let Some(c) = name.chars().find(|c| {
        c.is_whitespace()
            || c.is_control()
            || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    }) {
        return bad(&format!("contains forbidden character {c:?}"));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    Unmodified,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Untracked,
    Ignored,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStatus {
    pub path: String,
    pub index: StatusKind,
    pub worktree: StatusKind,
}

impl FileStatus {
    pub fn staged(&self) -> bool {
        !matches!(self.index, StatusKind::Unmodified | StatusKind::Untracked)
    }

    pub fn is_untracked(&self) -> bool {
        matches!(self.index, StatusKind::Untracked)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
    pub author: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
pub struct GitRepo {
    root: PathBuf,
}

impl GitRepo {
    /// Returns `Some(repo)` when `root` is inside a git work tree.
    pub fn detect(root: impl AsRef<Path>) -> Option<Self> {
        let root = root.as_ref();
        if !root.is_dir() {
            return None;
        }
        let out = Stdcommand("git")
            .args(["--no-optional-locks", "rev-parse", "--show-toplevel"])
            .current_dir(root)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let top = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if top.is_empty() {
            return None;
        }
        Some(Self {
            root: PathBuf::from(top),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        self.git_inner(args, false)
    }

    /// Run a read-only git command with repo-configured command hooks
    /// disabled (fsmonitor, hooks) so inspecting an untrusted repository
    /// cannot execute code from its `.git/config`.
    fn git_ro(&self, args: &[&str]) -> Result<String> {
        self.git_inner(args, true)
    }

    fn git_inner(&self, args: &[&str], read_only: bool) -> Result<String> {
        let mut cmd = Stdcommand("git");
        cmd.arg("-c").arg("core.quotepath=false");
        if read_only {
            cmd.arg("-c")
                .arg("core.fsmonitor=false")
                .arg("-c")
                .arg(format!("core.hooksPath={NULL_DEVICE}"));
        }
        let out = cmd
            .arg("--no-optional-locks")
            .args(args)
            .current_dir(&self.root)
            .output()
            .with_context(|| format!("running git {}", args.first().unwrap_or(&"")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            anyhow::bail!("git {}: {err}", args.first().unwrap_or(&""));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    pub fn current_branch(&self) -> Result<Option<String>> {
        let out = self.git_ro(&["rev-parse", "--abbrev-ref", "HEAD"])?;
        let b = out.trim();
        if b.is_empty() || b == "HEAD" {
            Ok(None) // detached HEAD
        } else {
            Ok(Some(b.to_string()))
        }
    }

    /// Working-tree status (porcelain v1, NUL separated for path safety).
    pub fn status(&self) -> Result<Vec<FileStatus>> {
        let out = self.git_ro(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
        let mut result = Vec::new();
        let mut fields = out.split('\0');
        while let Some(field) = fields.next() {
            if field.is_empty() {
                continue;
            }
            if field.len() < 3 {
                continue;
            }
            let bytes = field.as_bytes();
            let x = bytes[0] as char;
            let y = bytes[1] as char;
            let mut path = field[3..].to_string();
            if x == 'R' || x == 'C' {
                // Rename/copy: "XY new\0old"; the next field is the old path.
                if let Some(_old) = fields.next() {
                    // Keep the new path; expose old via Renamed kind.
                }
            }
            if path.starts_with('"') && path.ends_with('"') && path.len() > 1 {
                path = path[1..path.len() - 1].to_string();
            }
            result.push(FileStatus {
                path,
                index: parse_status_code(x),
                worktree: parse_status_code(y),
            });
        }
        Ok(result)
    }

    /// Unified diff of the working tree (vs index) or staged changes.
    pub fn diff(&self, path: Option<&str>, staged: bool) -> Result<String> {
        let mut args: Vec<String> = vec![
            "diff".into(),
            "--no-color".into(),
            "--unified=3".into(),
        ];
        if staged {
            args.push("--cached".into());
        }
        if let Some(p) = path {
            args.push("--".into());
            args.push(p.into());
        }
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.git_ro(&refs)
    }

    /// Diff of everything vs HEAD (staged + unstaged), including new files
    /// content when `include_untracked`.
    pub fn diff_head(&self, path: Option<&str>) -> Result<String> {
        let mut args: Vec<String> = vec![
            "diff".into(),
            "HEAD".into(),
            "--no-color".into(),
            "--unified=3".into(),
        ];
        if let Some(p) = path {
            args.push("--".into());
            args.push(p.into());
        }
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let tracked = self.git_ro(&refs).unwrap_or_default();
        // Append untracked files as fully-added diffs.
        let mut out = tracked;
        for f in self.status()? {
            if f.is_untracked() {
                if let Some(p) = path {
                    if f.path != p {
                        continue;
                    }
                }
                let abs = self.root.join(&f.path);
                // Never follow symlinks: an untracked link could point at a
                // secret outside the repository.
                let is_regular_file = std::fs::symlink_metadata(&abs)
                    .map(|m| m.file_type().is_file())
                    .unwrap_or(false);
                if is_regular_file {
                    let content = std::fs::read_to_string(&abs).unwrap_or_default();
                    out.push_str(&format!(
                        "diff --git a/{0} b/{0}\nnew file mode 100644\n--- /dev/null\n+++ b/{0}\n",
                        f.path
                    ));
                    for line in content.lines() {
                        out.push('+');
                        out.push_str(line);
                        out.push('\n');
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn stage(&self, paths: &[&str]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args: Vec<String> = vec!["add".into(), "--".into()];
        args.extend(paths.iter().map(|s| s.to_string()));
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.git(&refs).map(|_| ())
    }

    pub fn stage_all(&self) -> Result<()> {
        self.git(&["add", "-A"]).map(|_| ())
    }

    pub fn unstage(&self, paths: &[&str]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args: Vec<String> = vec!["reset".into(), "HEAD".into(), "--".into()];
        args.extend(paths.iter().map(|s| s.to_string()));
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.git(&refs).map(|_| ())
    }

    /// Discard working-tree changes for a path (destructive; caller must
    /// have permission).
    pub fn discard(&self, path: &str) -> Result<()> {
        self.git(&["checkout", "--", path]).map(|_| ())
    }

    pub fn commit(&self, message: &str) -> Result<String> {
        let out = self.git(&["commit", "-m", message])?;
        let hash = self.git(&["rev-parse", "HEAD"])?;
        let _ = out;
        Ok(hash.trim().to_string())
    }

    pub fn branches(&self) -> Result<Vec<BranchInfo>> {
        let out = self.git_ro(&["branch", "--format=%(HEAD)%(refname:short)"])?;
        let mut branches = Vec::new();
        for line in out.lines() {
            if line.len() < 2 {
                continue;
            }
            let current = line.starts_with('*');
            let name = line.trim_start_matches('*').trim().to_string();
            if !name.is_empty() {
                branches.push(BranchInfo { name, current });
            }
        }
        Ok(branches)
    }

    pub fn checkout_branch(&self, name: &str) -> Result<()> {
        validate_branch_name(name)?;
        // `--` ends option parsing and marks `name` as a revision, never a
        // pathspec (so `.` can't discard working-tree changes).
        self.git(&["checkout", name, "--"]).map(|_| ())
    }

    pub fn create_branch(&self, name: &str) -> Result<()> {
        validate_branch_name(name)?;
        self.git(&["checkout", "-b", name, "--"]).map(|_| ())
    }

    pub fn log(&self, limit: usize) -> Result<Vec<CommitInfo>> {
        let fmt = "--pretty=format:%H%x1f%h%x1f%s%x1f%an%x1f%at%x1e";
        let out = self.git_ro(&["log", &format!("-n{limit}"), fmt])?;
        let mut commits = Vec::new();
        for record in out.split('\x1e') {
            let record = record.trim();
            if record.is_empty() {
                continue;
            }
            let parts: Vec<&str> = record.split('\x1f').collect();
            if parts.len() >= 5 {
                commits.push(CommitInfo {
                    hash: parts[0].to_string(),
                    short_hash: parts[1].to_string(),
                    subject: parts[2].to_string(),
                    author: parts[3].to_string(),
                    timestamp: parts[4].parse().unwrap_or(0),
                });
            }
        }
        Ok(commits)
    }

    pub fn is_clean(&self) -> Result<bool> {
        Ok(self.status()?.is_empty())
    }
}

fn parse_status_code(c: char) -> StatusKind {
    match c {
        'M' => StatusKind::Modified,
        'A' => StatusKind::Added,
        'D' => StatusKind::Deleted,
        'R' => StatusKind::Renamed,
        'C' => StatusKind::Copied,
        '?' => StatusKind::Untracked,
        '!' => StatusKind::Ignored,
        ' ' => StatusKind::Unmodified,
        _ => StatusKind::Other,
    }
}

// Wrapper named to avoid clashing with std::process::Command in this module.
#[allow(non_snake_case)]
fn Stdcommand(program: &str) -> StdCommand {
    StdCommand::new(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_available() -> bool {
        Stdcommand("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn repo() -> (tempfile_like::TempDirGuard, GitRepo) {
        assert!(git_available(), "git must be installed for these tests");
        let dir = std::env::temp_dir().join(format!(
            "jcode-git-{}-{}",
            std::process::id(),
            chrono_like::nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Stdcommand("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .output()
            .unwrap();
        Stdcommand("git")
            .args(["config", "user.email", "test@jenderalcode.dev"])
            .current_dir(&dir)
            .output()
            .unwrap();
        Stdcommand("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&dir)
            .output()
            .unwrap();
        (
            tempfile_like::TempDirGuard { path: dir.clone() },
            GitRepo::detect(&dir).unwrap(),
        )
    }

    // Minimal temp-dir guard to avoid a dev-dependency in this crate.
    mod tempfile_like {
        pub struct TempDirGuard {
            pub path: std::path::PathBuf,
        }
        impl Drop for TempDirGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }
    mod chrono_like {
        /// Unique per call: wall-clock nanos (only µs resolution on macOS)
        /// plus a process-wide counter so parallel tests never share a dir.
        pub fn nanos() -> u128 {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            t * 1_000 + SEQ.fetch_add(1, Ordering::Relaxed) as u128 % 1_000
        }
    }

    #[test]
    fn branch_name_validation() {
        for bad in [
            "", ".", "..", "-f", "--orphan", "a..b", "a b", "a\tb", "x~1", "x^", "a:b", "a?", "a*",
            "a[b", "a\\b", "foo.lock", "foo/", "/foo", "a//b", "a@{1}", ".hidden", "a/.b", "a\u{7}",
        ] {
            assert!(validate_branch_name(bad).is_err(), "should reject {bad:?}");
        }
        for good in ["main", "feature/x-1", "fix_2", "release/v1.2.3", "user@host"] {
            assert!(validate_branch_name(good).is_ok(), "should accept {good:?}");
        }
    }

    #[test]
    fn checkout_dot_does_not_discard_changes() {
        let (_guard, repo) = repo();
        std::fs::write(repo.root().join("a.txt"), "v1\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("init").unwrap();
        std::fs::write(repo.root().join("a.txt"), "v2\n").unwrap();
        assert!(repo.checkout_branch(".").is_err());
        assert!(repo.create_branch("-D").is_err());
        assert_eq!(std::fs::read_to_string(repo.root().join("a.txt")).unwrap(), "v2\n");
        repo.create_branch("feature/ok").unwrap();
        assert_eq!(repo.current_branch().unwrap().as_deref(), Some("feature/ok"));
    }

    #[cfg(unix)]
    #[test]
    fn diff_head_skips_untracked_symlinks() {
        let (_guard, repo) = repo();
        std::fs::write(repo.root().join("a.txt"), "v1\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("init").unwrap();
        let outside = std::env::temp_dir().join(format!("jcode-git-secret-{}", chrono_like::nanos()));
        std::fs::write(&outside, "TOP-SECRET-VALUE\n").unwrap();
        std::os::unix::fs::symlink(&outside, repo.root().join("leak.txt")).unwrap();
        std::fs::write(repo.root().join("new.txt"), "visible\n").unwrap();
        let d = repo.diff_head(None).unwrap();
        assert!(d.contains("+visible"));
        assert!(!d.contains("TOP-SECRET-VALUE"));
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn detect_non_repo() {
        let dir = std::env::temp_dir().join(format!("jcode-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(GitRepo::detect(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_stage_commit_log_flow() {
        let (_guard, repo) = repo();
        std::fs::write(repo.root().join("a.txt"), "hello\n").unwrap();
        let st = repo.status().unwrap();
        assert_eq!(st.len(), 1);
        assert!(st[0].is_untracked());

        repo.stage(&["a.txt"]).unwrap();
        let st = repo.status().unwrap();
        assert!(st[0].staged());

        let hash = repo.commit("initial").unwrap();
        assert!(!hash.is_empty());
        assert!(repo.is_clean().unwrap());

        std::fs::write(repo.root().join("a.txt"), "hello world\n").unwrap();
        let d = repo.diff(None, false).unwrap();
        assert!(d.contains("-hello") && d.contains("+hello world"));

        let dh = repo.diff_head(None).unwrap();
        assert!(dh.contains("a.txt"));

        repo.discard("a.txt").unwrap();
        assert!(repo.is_clean().unwrap());

        let log = repo.log(5).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].subject, "initial");

        let branches = repo.branches().unwrap();
        assert!(branches.iter().any(|b| b.current));
        assert!(repo.current_branch().unwrap().is_some());
    }
}
