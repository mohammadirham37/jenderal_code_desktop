//! Terminal process manager (PRD §7, FR-07).
//!
//! Runs agent/user commands through the OS shell with:
//! - hard timeouts and output caps,
//! - cancellation via a `CancellationToken` (watch-channel based),
//! - process-group kill so shell children die with the shell.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::watch;

pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;
pub const MAX_TIMEOUT_MS: u64 = 600_000;
pub const DEFAULT_MAX_OUTPUT: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecSpec {
    pub command: String,
    pub cwd: PathBuf,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_max_output")]
    pub max_output_bytes: usize,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_MS
}
fn default_max_output() -> usize {
    DEFAULT_MAX_OUTPUT
}

impl ExecSpec {
    pub fn new(command: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            command: command.into(),
            cwd: cwd.into(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_output_bytes: DEFAULT_MAX_OUTPUT,
        }
    }

    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = ms.clamp(1_000, MAX_TIMEOUT_MS);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub exit_code: Option<i32>,
    pub success: bool,
    pub output: String,
    pub truncated: bool,
    pub timed_out: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
}

/// Simple cancellation token (watch channel based).
#[derive(Debug, Clone)]
pub struct CancelToken {
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelToken {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self { tx, rx }
    }

    pub fn cancel(&self) {
        let _ = self.tx.send(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }

    pub async fn cancelled(&self) {
        let mut rx = self.rx.clone();
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                return;
            }
        }
    }
}

fn shell_invocation(command: &str) -> (&'static str, Vec<String>) {
    if cfg!(windows) {
        return ("cmd", vec!["/C".into(), command.into()]);
    }
    let bash_exists = if cfg!(target_os = "macos") {
        true // /bin/bash ships with macOS
    } else {
        ["/bin/bash", "/usr/bin/bash"]
            .iter()
            .any(|p| std::path::Path::new(p).exists())
    };
    if bash_exists {
        ("bash", vec!["-c".into(), command.into()])
    } else {
        ("sh", vec!["-c".into(), command.into()])
    }
}

/// True when an environment variable name looks like it carries a secret
/// (API keys, tokens, passwords, provider credentials). Such variables are
/// removed from the environment of agent-run commands so the model cannot
/// exfiltrate them via `env`/`printenv`. Also usable by the interactive PTY.
pub fn is_secret_env_name(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    const PREFIXES: &[&str] = &["ANTHROPIC_", "OPENAI_", "GEMINI_", "JENDERALCODE_"];
    const EXACT: &[&str] = &["AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN", "GOOGLE_API_KEY", "GITHUB_TOKEN", "GH_TOKEN"];
    EXACT.contains(&n.as_str())
        || PREFIXES.iter().any(|p| n.starts_with(p))
        || n.ends_with("_KEY")
        || n.ends_with("_TOKEN")
        || n.contains("SECRET")
        || n.contains("PASSWORD")
        || n.contains("PASSWD")
}

/// Names of variables in the current process environment that
/// [`is_secret_env_name`] would scrub.
pub fn scrubbed_env_keys() -> Vec<String> {
    std::env::vars_os()
        .filter_map(|(k, _)| k.into_string().ok())
        .filter(|k| is_secret_env_name(k))
        .collect()
}

/// How long to wait for output pipes to drain after the main process has
/// exited and its process group has been killed.
const PUMP_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Run a command to completion under the given limits.
pub async fn run_exec(spec: &ExecSpec, cancel: &CancelToken) -> Result<ExecResult> {
    let started = Instant::now();
    let (prog, args) = shell_invocation(&spec.command);
    let mut cmd = Command::new(prog);
    cmd.args(&args)
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in scrubbed_env_keys() {
        cmd.env_remove(key);
    }

    #[cfg(unix)]
    {
        // Own process group so we can kill the whole tree.
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning `{}`", spec.command))?;
    let pid = child.id();

    // Pump stdout+stderr concurrently into one capped buffer.
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let cap = spec.max_output_bytes;
    let mut pump = tokio::spawn(async move {
        let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
        let mut truncated = false;
        let mut out_open = true;
        let mut err_open = true;
        let mut out_chunk = vec![0u8; 8 * 1024];
        let mut err_chunk = vec![0u8; 8 * 1024];
        while out_open || err_open {
            tokio::select! {
                n = stdout.read(&mut out_chunk), if out_open => match n {
                    Ok(0) | Err(_) => out_open = false,
                    Ok(n) => {
                        append_capped(&mut buf, &out_chunk[..n], cap, &mut truncated);
                    }
                },
                n = stderr.read(&mut err_chunk), if err_open => match n {
                    Ok(0) | Err(_) => err_open = false,
                    Ok(n) => {
                        append_capped(&mut buf, &err_chunk[..n], cap, &mut truncated);
                    }
                },
                else => break,
            }
        }
        (buf, truncated)
    });

    // Wait with timeout + cancellation.
    let timeout = Duration::from_millis(spec.timeout_ms);
    let mut timed_out = false;
    let mut cancelled = false;
    let status = tokio::select! {
        s = child.wait() => Some(s.context("waiting for process")?),
        _ = tokio::time::sleep(timeout) => { timed_out = true; None }
        _ = cancel.cancelled() => { cancelled = true; None }
    };

    // Always kill the process group: background jobs (`npm run dev &`)
    // outlive the shell and would hold stdout/stderr open forever. On
    // Windows the PID may already be recycled after a normal exit, so only
    // kill there when the process is still running; the drain timeout below
    // covers lingering pipes.
    if status.is_none() || cfg!(unix) {
        kill_process_tree(pid).await;
    }
    if status.is_none() {
        let _ = child.wait().await;
    }

    let (buf, mut truncated) = match tokio::time::timeout(PUMP_DRAIN_TIMEOUT, &mut pump).await {
        Ok(res) => res.unwrap_or_default(),
        Err(_) => {
            // A grandchild escaped the process group and still holds the
            // pipes; give up on the remaining output.
            pump.abort();
            (b"\n[jenderalcode] output pipes still open after exit; output may be incomplete".to_vec(), true)
        }
    };
    if status.is_none() {
        truncated = true;
        let note = if timed_out {
            format!(
                "\n[jenderalcode] command timed out after {}ms and was killed",
                spec.timeout_ms
            )
        } else {
            "\n[jenderalcode] command was cancelled".to_string()
        };
        let mut out = String::from_utf8_lossy(&buf).to_string();
        out.push_str(&note);
        return Ok(ExecResult {
            exit_code: None,
            success: false,
            output: out,
            truncated,
            timed_out,
            cancelled,
            duration_ms: started.elapsed().as_millis() as u64,
        });
    }

    let status = status.unwrap();
    Ok(ExecResult {
        exit_code: status.code(),
        success: status.success(),
        output: String::from_utf8_lossy(&buf).to_string(),
        truncated,
        timed_out: false,
        cancelled: false,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn append_capped(buf: &mut Vec<u8>, chunk: &[u8], cap: usize, truncated: &mut bool) {
    if buf.len() < cap {
        let room = cap - buf.len();
        buf.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if chunk.len() > room {
            *truncated = true;
        }
    } else {
        *truncated = true;
    }
}

async fn kill_process_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        // Negative pid = the whole process group (we spawned with group 0).
        let target = Pid::from_raw(-(pid as i32));
        if kill(target, Signal::SIGKILL).is_err() {
            let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jcode-exec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn captures_output_and_exit_code() {
        let dir = tmp();
        let spec = ExecSpec::new("echo hello-term", &dir);
        let res = run_exec(&spec, &CancelToken::new()).await.unwrap();
        assert!(res.success);
        assert_eq!(res.exit_code, Some(0));
        assert!(res.output.contains("hello-term"));
    }

    #[tokio::test]
    async fn timeout_kills_process() {
        let dir = tmp();
        let spec = ExecSpec::new("sleep 30", &dir).with_timeout_ms(1_500);
        let started = std::time::Instant::now();
        let res = run_exec(&spec, &CancelToken::new()).await.unwrap();
        assert!(res.timed_out);
        assert!(!res.success);
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    #[tokio::test]
    async fn cancellation_kills_process() {
        let dir = tmp();
        let spec = ExecSpec::new("sleep 30", &dir);
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            c2.cancel();
        });
        let res = run_exec(&spec, &cancel).await.unwrap();
        assert!(res.cancelled);
    }

    #[tokio::test]
    async fn output_is_capped() {
        let dir = tmp();
        let mut spec = ExecSpec::new("seq 1 100000", &dir);
        spec.max_output_bytes = 4 * 1024;
        let res = run_exec(&spec, &CancelToken::new()).await.unwrap();
        assert!(res.truncated);
        assert!(res.output.len() <= 8 * 1024);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn background_process_does_not_hang() {
        let dir = tmp();
        let spec = ExecSpec::new("sleep 30 & echo started-bg", &dir).with_timeout_ms(20_000);
        let started = std::time::Instant::now();
        let res = run_exec(&spec, &CancelToken::new()).await.unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(8), "took {:?}", started.elapsed());
        assert!(!res.timed_out);
        assert!(res.output.contains("started-bg"));
    }

    #[test]
    fn secret_env_names_detected() {
        for n in ["FOO_API_KEY", "foo_api_key", "MY_KEY", "GH_TOKEN", "DB_PASSWORD", "CLIENT_SECRET_X",
                  "AWS_SECRET_ACCESS_KEY", "ANTHROPIC_BASE_URL", "OPENAI_ORG", "GEMINI_X", "GOOGLE_API_KEY"] {
            assert!(is_secret_env_name(n), "{n}");
        }
        for n in ["PATH", "HOME", "SSH_AUTH_SOCK", "KEYBOARD", "LANG", "TERM"] {
            assert!(!is_secret_env_name(n), "{n}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn env_scrub_hides_api_keys() {
        // Set once for this test process; harmless for the other tests.
        std::env::set_var("JCODE_TEST_FOO_API_KEY", "sk-should-not-leak");
        std::env::set_var("JCODE_TEST_VISIBLE", "visible-value");
        let dir = tmp();
        let spec = ExecSpec::new("env", &dir);
        let res = run_exec(&spec, &CancelToken::new()).await.unwrap();
        assert!(res.output.contains("visible-value"));
        assert!(!res.output.contains("sk-should-not-leak"));
    }

    #[tokio::test]
    async fn nonzero_exit_reported() {
        let dir = tmp();
        let spec = ExecSpec::new("exit 42", &dir);
        let res = run_exec(&spec, &CancelToken::new()).await.unwrap();
        assert!(!res.success);
        assert_eq!(res.exit_code, Some(42));
    }
}
