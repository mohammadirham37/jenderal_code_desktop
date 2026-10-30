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
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &script])
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

    #[cfg(unix)]
    #[test]
    fn login_shell_reports_a_path() {
        let p = login_shell_path().expect("login shell PATH");
        assert!(p.contains("/usr/bin") || p.contains("/bin"), "{p}");
    }
}
