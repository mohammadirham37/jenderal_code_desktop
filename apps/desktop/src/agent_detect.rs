//! Which agent CLIs from the settings are installed, for the chat column's
//! "+" menu. The lookup runs in a login shell so it sees the same PATH as
//! the terminal tabs that launch them.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

/// First word of a command line if it is a plain program name or path that
/// is safe inside a single-quoted shell word.
pub fn program_name(command: &str) -> Option<&str> {
    let name = command.split_whitespace().next()?;
    let plain = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-' | '/'));
    (plain && !name.starts_with('-')).then_some(name)
}

/// One `command -v` per valid name; a found program prints `jc:<index>`.
fn build_script(names: &[Option<&str>]) -> String {
    names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.map(|n| format!("command -v '{n}' >/dev/null 2>&1 && echo jc:{i};")))
        .collect()
}

/// Only `jc:<index>` lines count, so anything the user's profile prints is
/// ignored.
fn parse_output(output: &str, len: usize) -> Vec<bool> {
    let mut found = vec![false; len];
    for line in output.lines() {
        if let Some(i) = line.trim().strip_prefix("jc:").and_then(|s| s.parse::<usize>().ok()) {
            if i < len {
                found[i] = true;
            }
        }
    }
    found
}

/// For each command line, whether its program is installed. Errors and
/// timeouts report `false` (the menu still lets the user launch it).
pub fn detect(commands: &[String]) -> Vec<bool> {
    let names: Vec<Option<&str>> = commands.iter().map(|c| program_name(c)).collect();
    if cfg!(windows) {
        return names
            .iter()
            .map(|n| {
                n.is_some_and(|n| {
                    ["", ".exe", ".cmd", ".bat"]
                        .iter()
                        .any(|ext| crate::terminal::which(&format!("{n}{ext}")).is_some())
                })
            })
            .collect();
    }
    let script = build_script(&names);
    if script.is_empty() {
        return vec![false; commands.len()];
    }
    run_script(&script)
        .map(|out| parse_output(&out, commands.len()))
        .unwrap_or_else(|| vec![false; commands.len()])
}

/// Login *and* interactive, like the terminal tabs: many installers (nvm,
/// opencode, bun, ...) add themselves to PATH in `~/.zshrc`, which a
/// non-interactive login shell does not read.
fn shell_command(shell: &str, script: &str) -> Command {
    let mut cmd = Command::new(shell);
    cmd.args(["-l", "-i", "-c", script]);
    cmd
}

fn run_script(script: &str) -> Option<String> {
    run_command(shell_command(&crate::terminal::user_shell(), script))
}

fn run_command(mut cmd: Command) -> Option<String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_name_takes_first_plain_word() {
        assert_eq!(program_name("goose session"), Some("goose"));
        assert_eq!(program_name("  claude  "), Some("claude"));
        assert_eq!(program_name("/opt/bin/aider --model x"), Some("/opt/bin/aider"));
        assert_eq!(program_name("rm'x"), None);
        assert_eq!(program_name("$(evil)"), None);
        assert_eq!(program_name("-v"), None);
        assert_eq!(program_name(""), None);
    }

    #[test]
    fn script_checks_only_valid_names() {
        assert_eq!(
            build_script(&[Some("claude"), None, Some("codex")]),
            "command -v 'claude' >/dev/null 2>&1 && echo jc:0;\
             command -v 'codex' >/dev/null 2>&1 && echo jc:2;"
        );
    }

    #[test]
    fn output_markers_map_to_indices_and_noise_is_ignored() {
        assert_eq!(
            parse_output("Welcome back!\njc:2\njc:0\njc:9\njc:x\n", 3),
            vec![true, false, true]
        );
    }

    /// Regression: opencode adds its bin dir to PATH in `~/.zshrc`. Started
    /// from Finder (minimal PATH), detection must still find it.
    #[cfg(unix)]
    #[test]
    fn finds_programs_added_to_path_in_zshrc() {
        if !std::path::Path::new("/bin/zsh").exists() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("tool/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("jc-fake-agent");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        std::fs::write(home.path().join(".zshrc"), format!("export PATH={}:$PATH\n", bin.display())).unwrap();

        let script = build_script(&[Some("jc-fake-agent")]);
        let mut cmd = shell_command("/bin/zsh", &script);
        cmd.env_clear()
            .env("HOME", home.path())
            .env("ZDOTDIR", home.path())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
        let out = run_command(cmd).expect("shell ran");
        assert_eq!(parse_output(&out, 1), vec![true], "output: {out:?}");
    }

    #[cfg(unix)]
    #[test]
    fn detects_real_and_missing_programs() {
        let found = detect(&["sh -c true".into(), "jc-missing-program-xyz".into(), "".into()]);
        assert_eq!(found, vec![true, false, false]);
    }
}
