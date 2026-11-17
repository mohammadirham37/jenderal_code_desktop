//! The user's login-shell PATH for agent commands. Apps started from Finder
//! inherit a minimal PATH (`/usr/bin:/bin:...`), so tools installed through
//! Homebrew, nvm, Herd, etc. are missing. The PATH is resolved once in the
//! background and stored here; until then (or if it fails) commands use the
//! process PATH.

use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const START: &str = "__JC_PATH_START__";
const END: &str = "__JC_PATH_END__";
const TIMEOUT: Duration = Duration::from_secs(3);

static USER_PATH: OnceLock<String> = OnceLock::new();

/// Flags that start `shell` the way the OS terminal does, so it sees the
/// same PATH: login + interactive (`-l -i`), except bash on Linux, which
/// gets `-i` only. Terminal emulators there run a non-login bash, which reads
/// `~/.bashrc` (where nvm, fnm, etc. add themselves); a login bash reads
/// `~/.bash_profile` instead when it exists and then skips `~/.bashrc`. The
/// desktop session has already applied `~/.profile`.
pub fn interactive_shell_args(shell: &str) -> &'static [&'static str] {
    shell_args_for(shell, cfg!(target_os = "linux"))
}

fn shell_args_for(shell: &str, linux: bool) -> &'static [&'static str] {
    let name = shell.rsplit('/').next().unwrap_or(shell);
    if linux && name == "bash" {
        &["-i"]
    } else {
        &["-l", "-i"]
    }
}

/// The resolved PATH, if [`init_from_login_shell`] has finished.
pub fn user_path() -> Option<&'static str> {
    USER_PATH.get().map(String::as_str)
}

/// Resolve the login-shell PATH and store it merged with the process PATH.
/// Blocking (up to a few seconds): call it from a background thread.
pub fn init_from_login_shell() {
    if cfg!(windows) || USER_PATH.get().is_some() {
        return;
    }
    let current = std::env::var("PATH").unwrap_or_default();
    if let Some(login) = login_shell_path() {
        let _ = USER_PATH.set(merge_paths(&login, &current));
    }
}

fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let script = format!("printf '{START}%s{END}' \"$PATH\"");
    let mut child = Command::new(&shell)
        .args(interactive_shell_args(&shell))
        .args(["-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    parse_marked_path(&String::from_utf8_lossy(&out.stdout))
}

/// Windows: the PATH a newly opened terminal would get, read from the
/// registry (machine, then user `Path`, `%VAR%`s expanded) and followed by
/// process entries it lacks. A running app keeps the PATH it started with,
/// so without this a tool installed meanwhile (Node.js, an agent CLI via
/// npm or winget) stays "not installed" until the app restarts. `None`
/// elsewhere or when the registry cannot be read.
pub fn fresh_windows_path() -> Option<String> {
    #[cfg(windows)]
    {
        let machine = reg_query(r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment");
        let user = reg_query(r"HKCU\Environment");
        if machine.is_none() && user.is_none() {
            return None;
        }
        let current = std::env::var("PATH").unwrap_or_default();
        let fresh = [machine, user].into_iter().flatten().collect::<Vec<_>>().join(";");
        Some(merge_windows_paths(&expand_env_vars(&fresh, |k| std::env::var(k).ok()), &current))
    }
    #[cfg(not(windows))]
    None
}

#[cfg(windows)]
fn reg_query(key: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = Command::new("reg")
        .args(["query", key, "/v", "Path"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    parse_reg_path(&String::from_utf8_lossy(&out.stdout))
}

/// The value of `reg query <key> /v Path`:
/// `    Path    REG_EXPAND_SZ    C:\a;%USERPROFILE%\b`.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_reg_path(output: &str) -> Option<String> {
    output.lines().find_map(|l| {
        let l = l.trim_start();
        let rest = l.strip_prefix("Path").or_else(|| l.strip_prefix("PATH"))?;
        let rest = rest.trim_start();
        let rest = rest.strip_prefix("REG_EXPAND_SZ").or_else(|| rest.strip_prefix("REG_SZ"))?;
        let v = rest.trim();
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// `%NAME%` replaced by `var(NAME)`; unknown names are left as they are.
#[cfg_attr(not(windows), allow(dead_code))]
fn expand_env_vars(s: &str, var: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) if j > 0 && !after[..j].contains(';') => {
                match var(&after[..j]) {
                    Some(v) => out.push_str(&v),
                    None => out.push_str(&rest[i..i + j + 2]),
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Like [`merge_paths`] for `;`-separated Windows paths, comparing entries
/// without case or a trailing backslash.
#[cfg_attr(not(windows), allow(dead_code))]
fn merge_windows_paths(fresh: &str, current: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    fresh
        .split(';')
        .chain(current.split(';'))
        .map(str::trim)
        .filter(|p| !p.is_empty() && seen.insert(p.trim_end_matches('\\').to_lowercase()))
        .collect::<Vec<_>>()
        .join(";")
}

/// The PATH printed between the markers; anything the profile prints
/// around it is ignored.
fn parse_marked_path(output: &str) -> Option<String> {
    let rest = &output[output.rfind(START)? + START.len()..];
    let path = &rest[..rest.find(END)?];
    (!path.trim().is_empty()).then(|| path.to_string())
}

/// Login-shell entries first, then process entries it lacks; no duplicates
/// or empty entries.
fn merge_paths(login: &str, current: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    login
        .split(':')
        .chain(current.split(':'))
        .filter(|p| !p.is_empty() && seen.insert(*p))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_on_linux_is_not_a_login_shell() {
        assert_eq!(shell_args_for("/bin/bash", true), ["-i"]);
        assert_eq!(shell_args_for("/usr/bin/zsh", true), ["-l", "-i"]);
        assert_eq!(shell_args_for("/bin/bash", false), ["-l", "-i"]);
        assert_eq!(shell_args_for("/bin/zsh", false), ["-l", "-i"]);
    }

    #[test]
    fn parses_path_between_markers_ignoring_profile_noise() {
        let out = format!("Welcome!\n{START}/opt/homebrew/bin:/usr/bin{END}\nbye");
        assert_eq!(parse_marked_path(&out).as_deref(), Some("/opt/homebrew/bin:/usr/bin"));
        assert_eq!(parse_marked_path("no markers"), None);
        assert_eq!(parse_marked_path(&format!("{START}{END}")), None);
    }

    #[test]
    fn merge_puts_login_first_without_duplicates() {
        assert_eq!(
            merge_paths("/opt/homebrew/bin:/usr/bin", "/usr/bin:/bin::/usr/bin"),
            "/opt/homebrew/bin:/usr/bin:/bin"
        );
    }

    #[test]
    fn reads_the_windows_registry_path() {
        let out = "\r\nHKEY_CURRENT_USER\\Environment\r\n    Path    REG_EXPAND_SZ    %USERPROFILE%\\AppData\\Roaming\\npm;C:\\x\r\n\r\n";
        assert_eq!(parse_reg_path(out).as_deref(), Some("%USERPROFILE%\\AppData\\Roaming\\npm;C:\\x"));
        assert_eq!(parse_reg_path("    Path    REG_SZ    C:\\a").as_deref(), Some("C:\\a"));
        assert_eq!(parse_reg_path("ERROR: not found"), None);
    }

    #[test]
    fn expands_known_variables_only() {
        let var = |k: &str| (k == "USERPROFILE").then(|| "C:\\Users\\me".to_string());
        assert_eq!(
            expand_env_vars("%USERPROFILE%\\npm;%NOPE%\\x;50%;%", var),
            "C:\\Users\\me\\npm;%NOPE%\\x;50%;%"
        );
    }

    #[test]
    fn windows_merge_ignores_case_and_trailing_backslash() {
        assert_eq!(
            merge_windows_paths("C:\\Program Files\\nodejs\\;C:\\Windows", "c:\\windows;D:\\old;;"),
            "C:\\Program Files\\nodejs\\;C:\\Windows;D:\\old"
        );
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_reports_a_path() {
        let p = login_shell_path().expect("login shell PATH");
        assert!(p.contains("/usr/bin") || p.contains("/bin"), "{p}");
    }
}
