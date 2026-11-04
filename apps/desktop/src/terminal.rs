//! Interactive PTY terminals for the workspace terminal panel (xterm.js on
//! the frontend, portable-pty on the backend).
//!
//! Ownership model: the handle map keeps the real `Child` (so `kill` works),
//! the reader thread owns a cloned reader, and shell exit is detected via
//! reader EOF (the slave end is dropped after spawn, so EOF is reliable).

use anyhow::{Context, Result};
use jcode_common::{TerminalDataEvent, TerminalExitEvent, EVENT_TERMINAL_DATA, EVENT_TERMINAL_EXIT};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

struct PtyHandle {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Distinguishes successive terminals reusing the same id, so a stale
    /// reader thread never removes its replacement.
    generation: u64,
}

type HandleMap = Arc<Mutex<HashMap<String, PtyHandle>>>;

/// Cheap to clone (shared state), so commands can move it into
/// `spawn_blocking`.
#[derive(Clone, Default)]
pub struct TerminalManager {
    handles: HandleMap,
    next_generation: Arc<AtomicU64>,
}

fn lock(map: &HandleMap) -> std::sync::MutexGuard<'_, HashMap<String, PtyHandle>> {
    map.lock().unwrap_or_else(|e| e.into_inner())
}

/// The user's login shell (Unix).
pub(crate) fn user_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(target_os = "macos") {
            "/bin/zsh".to_string()
        } else {
            "/bin/bash".to_string()
        }
    })
}

fn default_shell() -> CommandBuilder {
    if cfg!(windows) {
        if let Some(pwsh) = which("pwsh.exe").or_else(|| which("powershell.exe")) {
            return CommandBuilder::new(pwsh);
        }
        return CommandBuilder::new("cmd.exe");
    }
    let shell = user_shell();
    // Started like the OS terminal (login + interactive on macOS, `bash -i`
    // on Linux), so the user's profile and rc files set the same PATH.
    let mut cmd = CommandBuilder::new(&shell);
    cmd.args(jcode_tool_terminal::user_path::interactive_shell_args(&shell));
    cmd
}

/// The process for a terminal: an interactive login shell, or (for agent
/// tabs) `command` run through that shell so the user's PATH and aliases
/// apply. Apps started from Finder do not inherit the terminal's PATH.
fn build_command(command: Option<&str>) -> CommandBuilder {
    let Some(command) = command.map(str::trim).filter(|c| !c.is_empty()) else {
        return default_shell();
    };
    if cfg!(windows) {
        if let Some(pwsh) = which("pwsh.exe").or_else(|| which("powershell.exe")) {
            let mut cmd = CommandBuilder::new(pwsh);
            cmd.args(["-NoLogo", "-Command", command]);
            return cmd;
        }
        let mut cmd = CommandBuilder::new("cmd.exe");
        cmd.args(["/C", command]);
        return cmd;
    }
    let mut cmd = default_shell();
    cmd.args(["-c", command]);
    cmd
}

pub(crate) fn which(prog: &str) -> Option<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(if cfg!(windows) { ';' } else { ':' }) {
        let candidate = Path::new(dir).join(prog);
        if candidate.is_file() {
            return candidate.to_str().map(String::from);
        }
    }
    None
}

impl TerminalManager {
    /// Spawn a new terminal. `id` is chosen by the frontend; `command`, when
    /// set, runs instead of an interactive shell (agent CLI tabs).
    pub fn create(
        &self,
        app: &AppHandle,
        id: &str,
        cwd: &Path,
        cols: u16,
        rows: u16,
        command: Option<&str>,
    ) -> Result<()> {
        {
            let mut handles = lock(&self.handles);
            if let Some(h) = handles.get_mut(id) {
                // Reuse only a live shell; replace one that has exited.
                match h.child.try_wait() {
                    Ok(None) => return Ok(()), // already running
                    _ => {
                        if let Some(mut dead) = handles.remove(id) {
                            let _ = dead.child.kill();
                        }
                    }
                }
            }
        }
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("opening pty")?;

        let mut cmd = build_command(command);
        cmd.cwd(cwd);
        // Apps launched from Finder/launchd inherit an environment WITHOUT
        // TERM. Without a terminal description the shell cannot move the
        // cursor left, so Backspace echoes a space and the cursor moves
        // RIGHT instead of erasing (verified with examples/pty_probe.rs).
        // xterm.js speaks xterm-256color.
        if cmd.get_env("TERM").is_none() {
            cmd.env("TERM", "xterm-256color");
        }
        cmd.env("COLORTERM", "truecolor");
        // Never hand provider API keys / tokens from the app's environment
        // to the interactive shell (or anything it runs).
        for key in jcode_tool_terminal::scrubbed_env_keys() {
            cmd.env_remove(key);
        }
        let child = pair.slave.spawn_command(cmd).context("spawning terminal process")?;
        drop(pair.slave); // close the slave end in the parent so EOF works

        let mut reader = pair
            .master
            .try_clone_reader()
            .context("cloning pty reader")?;
        let writer = pair.master.take_writer().context("taking pty writer")?;

        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        // Insert before the reader starts so its EOF cleanup always finds it.
        lock(&self.handles).insert(
            id.to_string(),
            PtyHandle {
                writer,
                master: pair.master,
                child,
                generation,
            },
        );

        // Reader thread: forward PTY output; EOF means the shell exited.
        // UTF-8 sequences can be split across read() chunks — keep the
        // incomplete tail in a buffer instead of corrupting it with lossy
        // conversion (which would inject U+FFFD into the terminal stream).
        let app_reader = app.clone();
        let term_id = id.to_string();
        let handles = self.handles.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut pending: Vec<u8> = Vec::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        pending.extend_from_slice(&buf[..n]);
                        let data = match std::str::from_utf8(&pending) {
                            Ok(s) => {
                                let s = s.to_string();
                                pending.clear();
                                s
                            }
                            Err(e) => {
                                let valid = e.valid_up_to();
                                let s = String::from_utf8_lossy(&pending[..valid]).to_string();
                                pending.drain(..valid);
                                if let Some(invalid_len) = e.error_len() {
                                    // Truly invalid bytes (not just an
                                    // incomplete tail): drop them to resync.
                                    pending.drain(..invalid_len);
                                }
                                s
                            }
                        };
                        if !data.is_empty() {
                            let _ = app_reader.emit(
                                EVENT_TERMINAL_DATA,
                                TerminalDataEvent {
                                    terminal_id: term_id.clone(),
                                    data,
                                },
                            );
                        }
                    }
                }
            }
            // Shell exited: drop our handle (unless it was already killed or
            // replaced) and reap the child to get its exit code.
            let removed = {
                let mut map = lock(&handles);
                if map.get(&term_id).is_some_and(|h| h.generation == generation) {
                    map.remove(&term_id)
                } else {
                    None
                }
            };
            let code = removed.and_then(|mut h| {
                h.child.wait().ok().map(|s| s.exit_code() as i32)
            });
            let _ = app_reader.emit(
                EVENT_TERMINAL_EXIT,
                TerminalExitEvent {
                    terminal_id: term_id,
                    code,
                },
            );
        });

        Ok(())
    }

    pub fn write(&self, id: &str, data: &str) -> Result<()> {
        let mut handles = lock(&self.handles);
        let handle = handles
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("terminal {id} not found"))?;
        handle
            .writer
            .write_all(data.as_bytes())
            .context("writing to pty")?;
        handle.writer.flush().ok();
        Ok(())
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<()> {
        let handles = lock(&self.handles);
        let handle = handles
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("terminal {id} not found"))?;
        handle
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resizing pty")?;
        Ok(())
    }

    pub fn kill(&self, id: &str) -> Result<()> {
        let removed = lock(&self.handles).remove(id);
        if let Some(mut handle) = removed {
            let _ = handle.child.kill();
            let _ = handle.child.wait();
        }
        Ok(())
    }

    pub fn kill_all(&self) {
        let drained: Vec<PtyHandle> = lock(&self.handles).drain().map(|(_, h)| h).collect();
        for mut h in drained {
            let _ = h.child.kill();
            let _ = h.child.wait();
        }
    }

    pub fn list(&self) -> Vec<String> {
        lock(&self.handles).keys().cloned().collect()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn argv(cmd: &CommandBuilder) -> Vec<String> {
        cmd.get_argv()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn plain_terminal_is_interactive_login_shell() {
        let a = argv(&build_command(None));
        assert_eq!(a[0], user_shell());
        assert_eq!(&a[1..], shell_args());
    }

    fn shell_args() -> Vec<&'static str> {
        jcode_tool_terminal::user_path::interactive_shell_args(&user_shell()).to_vec()
    }

    #[test]
    fn agent_command_runs_through_login_shell() {
        let a = argv(&build_command(Some("goose session")));
        assert_eq!(a[0], user_shell());
        assert_eq!(&a[1..], [shell_args(), vec!["-c", "goose session"]].concat());
    }

    #[test]
    fn blank_command_is_a_plain_shell() {
        let a = argv(&build_command(Some("   ")));
        assert_eq!(&a[1..], shell_args());
    }
}
