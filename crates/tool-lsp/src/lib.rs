//! Language-server diagnostics for files the agent just wrote, so the model
//! sees type and syntax errors right after an edit (like opencode).
//!
//! One server per (workspace root, language), started lazily in the
//! background on the first edit of a matching file. Servers come from the
//! user's PATH (login shell) or, when allowed, are downloaded and run with
//! `npx`. A server that fails to start is not retried for a while.

mod client;

pub use client::{Diagnostic, LspClient};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use tokio::sync::watch;

/// Wait for a server started by this edit before giving up on diagnostics.
const FIRST_START_WAIT: Duration = Duration::from_secs(8);
/// Wait for diagnostics after each change.
const DIAGNOSTICS_WAIT: Duration = Duration::from_secs(5);
/// `initialize` can take long when npx downloads the server first.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(180);
const RETRY_FAILED_AFTER: Duration = Duration::from_secs(300);
/// Errors reported to the model per file.
const MAX_REPORTED: usize = 20;

struct ServerSpec {
    id: &'static str,
    extensions: &'static [&'static str],
    /// Installed binary + arguments.
    command: &'static [&'static str],
    /// `npx` fallback that downloads the server.
    npx: Option<&'static [&'static str]>,
}

const SERVERS: &[ServerSpec] = &[
    ServerSpec {
        id: "typescript",
        extensions: &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"],
        // vtsls bundles TypeScript, so it also works in projects without a
        // `typescript` dependency (typescript-language-server refuses those).
        command: &["vtsls", "--stdio"],
        npx: Some(&["npx", "--yes", "@vtsls/language-server", "--stdio"]),
    },
    ServerSpec {
        id: "php",
        extensions: &["php"],
        command: &["intelephense", "--stdio"],
        npx: Some(&["npx", "--yes", "intelephense", "--stdio"]),
    },
    ServerSpec {
        id: "python",
        extensions: &["py", "pyi"],
        command: &["pyright-langserver", "--stdio"],
        npx: Some(&["npx", "--yes", "-p", "pyright", "pyright-langserver", "--stdio"]),
    },
    ServerSpec { id: "rust", extensions: &["rs"], command: &["rust-analyzer"], npx: None },
    ServerSpec { id: "go", extensions: &["go"], command: &["gopls"], npx: None },
];

fn language_id(ext: &str) -> &'static str {
    match ext {
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "php" => "php",
        "py" | "pyi" => "python",
        "rs" => "rust",
        "go" => "go",
        _ => "plaintext",
    }
}

enum Slot {
    Starting(watch::Receiver<bool>),
    Ready(Arc<LspClient>),
    Failed(Instant),
}

struct Server {
    slot: Slot,
    child: Option<Child>,
}

#[derive(Default)]
pub struct LspManager {
    servers: Arc<Mutex<HashMap<(PathBuf, &'static str), Server>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl LspManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Diagnostics for `file` (absolute, inside `root`) with content `text`.
    /// `None` when no server applies, it is not available, or it did not
    /// answer in time; never an error for the caller to surface.
    pub async fn diagnostics(&self, root: &Path, file: &Path, text: &str, allow_download: bool) -> Option<Vec<Diagnostic>> {
        let ext = file.extension()?.to_str()?.to_ascii_lowercase();
        let spec = SERVERS.iter().find(|s| s.extensions.contains(&ext.as_str()))?;
        let client = self.client(root, spec, allow_download).await?;
        client.diagnostics(file, language_id(&ext), text, DIAGNOSTICS_WAIT).await.ok()
    }

    async fn client(&self, root: &Path, spec: &'static ServerSpec, allow_download: bool) -> Option<Arc<LspClient>> {
        let key = (root.to_path_buf(), spec.id);
        let mut ready = {
            let mut servers = lock(&self.servers);
            match servers.get(&key).map(|s| &s.slot) {
                Some(Slot::Ready(c)) if c.is_alive() => return Some(c.clone()),
                Some(Slot::Starting(rx)) => rx.clone(),
                Some(Slot::Failed(at)) if at.elapsed() < RETRY_FAILED_AFTER => return None,
                _ => {
                    let argv = resolve_command(spec, allow_download)?;
                    let (tx, rx) = watch::channel(false);
                    servers.insert(key.clone(), Server { slot: Slot::Starting(rx.clone()), child: None });
                    let servers = self.servers.clone();
                    let root = root.to_path_buf();
                    tokio::spawn(async move {
                        let started = start_server(&argv, &root).await;
                        let mut map = lock(&servers);
                        let entry = map.entry(key).or_insert(Server { slot: Slot::Failed(Instant::now()), child: None });
                        match started {
                            Some((client, child)) => {
                                entry.slot = Slot::Ready(client);
                                entry.child = Some(child);
                            }
                            None => entry.slot = Slot::Failed(Instant::now()),
                        }
                        drop(map);
                        let _ = tx.send(true);
                    });
                    rx
                }
            }
        };
        // Give a server that is starting a little time; later edits will
        // find it ready.
        let _ = tokio::time::timeout(FIRST_START_WAIT, ready.wait_for(|done| *done)).await;
        match lock(&self.servers).get(&(root.to_path_buf(), spec.id)).map(|s| &s.slot) {
            Some(Slot::Ready(c)) if c.is_alive() => Some(c.clone()),
            _ => None,
        }
    }

    /// Stop every language server (app exit).
    pub fn shutdown(&self) {
        for (_, mut server) in lock(&self.servers).drain() {
            if let Some(child) = server.child.as_mut() {
                let _ = child.start_kill();
            }
        }
    }
}

impl Drop for LspManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// argv for the installed server, else the npx fallback when allowed.
fn resolve_command(spec: &ServerSpec, allow_download: bool) -> Option<Vec<String>> {
    let path = search_path();
    let owned = |argv: &[&str]| argv.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    if find_program(spec.command[0], &path).is_some() {
        return Some(owned(spec.command));
    }
    match spec.npx {
        Some(npx) if allow_download && find_program("npx", &path).is_some() => Some(owned(npx)),
        _ => None,
    }
}

fn search_path() -> String {
    jcode_tool_terminal::user_path::user_path()
        .map(String::from)
        .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
}

fn find_program(name: &str, path: &str) -> Option<PathBuf> {
    std::env::split_paths(path).map(|d| d.join(name)).find(|p| p.is_file())
}

async fn start_server(argv: &[String], root: &Path) -> Option<(Arc<LspClient>, Child)> {
    let path = search_path();
    let program = find_program(&argv[0], &path)?;
    let mut cmd = Command::new(program);
    cmd.args(&argv[1..])
        .current_dir(root)
        .env("PATH", &path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in jcode_tool_terminal::scrubbed_env_keys() {
        cmd.env_remove(key);
    }
    let mut child = cmd.spawn().ok()?;
    let client = LspClient::start(child.stdout.take()?, child.stdin.take()?);
    if client.initialize(root, INITIALIZE_TIMEOUT).await.is_err() {
        let _ = child.start_kill();
        return None;
    }
    Some((client, child))
}

/// Text appended to a tool result: the file's errors, if any.
pub fn format_errors(path: &str, diagnostics: &[Diagnostic]) -> Option<String> {
    let errors: Vec<&Diagnostic> = diagnostics.iter().filter(|d| d.severity == 1).collect();
    if errors.is_empty() {
        return None;
    }
    let mut out = format!("\n\nThe language server reports {} error(s) in {path} after this change:\n", errors.len());
    for d in errors.iter().take(MAX_REPORTED) {
        let message = d.message.lines().next().unwrap_or_default();
        out.push_str(&format!("ERROR [{}:{}] {message}\n", d.line, d.column));
    }
    if errors.len() > MAX_REPORTED {
        out.push_str(&format!("... and {} more\n", errors.len() - MAX_REPORTED));
    }
    out.push_str("Fix them unless they are expected (e.g. code you are about to add).");
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_errors_are_reported() {
        let d = |severity, message: &str| Diagnostic { line: 3, column: 5, severity, message: message.into() };
        assert_eq!(format_errors("a.ts", &[d(2, "unused")]), None);
        let out = format_errors("a.ts", &[d(1, "Cannot find name 'x'.\nmore"), d(2, "unused")]).unwrap();
        assert!(out.contains("1 error(s) in a.ts"));
        assert!(out.contains("ERROR [3:5] Cannot find name 'x'."));
        assert!(!out.contains("unused") && !out.contains("more"));
    }

    #[tokio::test]
    async fn unknown_extensions_have_no_server() {
        let m = LspManager::new();
        assert!(m.diagnostics(Path::new("/tmp"), Path::new("/tmp/a.txt"), "x", true).await.is_none());
    }
}

/// Live checks against real servers (downloads via npx):
/// `cargo test -p jcode-tool-lsp live_ -- --ignored --nocapture`.
#[cfg(test)]
mod live {
    use super::*;

    async fn run(file: &str, text: &str) -> Option<Vec<Diagnostic>> {
        jcode_tool_terminal::user_path::init_from_login_shell();
        let dir = std::env::temp_dir().join(format!("jcode-lsp-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(file);
        std::fs::write(&path, text).unwrap();
        let m = LspManager::new();
        // First call starts the server; retry until it is ready.
        for _ in 0..40 {
            if let Some(d) = m.diagnostics(&dir, &path, text, true).await {
                return Some(d);
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        None
    }

    #[tokio::test]
    #[ignore]
    async fn live_php_syntax_error() {
        let d = run("broken.php", "<?php\nfunction a( {\n  return 1;\n}\n").await.expect("diagnostics");
        println!("{d:#?}");
        assert!(d.iter().any(|d| d.severity == 1));
    }

    #[tokio::test]
    #[ignore]
    async fn live_typescript_type_error() {
        let d = run("broken.ts", "const n: number = \"text\";\n").await.expect("diagnostics");
        println!("{d:#?}");
        assert!(d.iter().any(|d| d.severity == 1));
    }
}
