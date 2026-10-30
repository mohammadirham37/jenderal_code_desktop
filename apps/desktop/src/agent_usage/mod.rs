//! Subscription usage of external agent CLIs (Claude Code, Codex,
//! Antigravity, OpenCode) for the chat column's Usage popover.
//!
//! Everything is read-only: each CLI's local files, plus Claude Code's usage
//! endpoint called with its stored login token. Tokens are never refreshed,
//! so the CLIs' own logins stay intact.

mod antigravity;
mod claude;
mod codex;
mod opencode;

use jcode_common::{now_ms, AgentUsageReport, AgentUsageStatus};
use std::path::PathBuf;

/// Reports in display order. Missing data for a CLI that is not installed
/// shows as "not installed".
pub async fn collect() -> Vec<AgentUsageReport> {
    let home = home_dir();
    let now = now_ms();

    let local = {
        let home = home.clone();
        tokio::task::spawn_blocking(move || {
            let installed = crate::agent_detect::detect(&["claude".into(), "codex".into(), "agy".into(), "opencode".into()]);
            let reports = home.map(|h| {
                (
                    codex::report(&h.join(".codex"), now),
                    antigravity::report(&h),
                    opencode::report(&opencode::db_path(&h), now),
                )
            });
            (installed, reports)
        })
    };
    let claude = claude::report(home.as_deref(), now).await;
    let (installed, reports) = local.await.unwrap_or_else(|_| (vec![false; 4], None));

    let mut out = vec![claude];
    match reports {
        Some((codex, antigravity, opencode)) => out.extend([codex, antigravity, opencode]),
        None => {
            for (id, name) in [("codex", "Codex"), ("antigravity", "Antigravity"), ("opencode", "OpenCode")] {
                out.push(AgentUsageReport::new(id, name, AgentUsageStatus::Error).with_note("Home directory not found."));
            }
        }
    }
    for (report, installed) in out.iter_mut().zip(installed) {
        if !installed && report.status != AgentUsageStatus::Ok {
            *report = AgentUsageReport::new(&report.id, &report.name, AgentUsageStatus::NotInstalled);
        }
    }
    out
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// "12", "12.3k", "1.2M".
pub(crate) fn format_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// "max_5x" -> "Max 5x", "team" -> "Team".
pub(crate) fn title_words(s: &str) -> String {
    s.split(['_', '-', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_helpers() {
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(83_019), "83.0k");
        assert_eq!(format_tokens(1_250_000), "1.2M");
        assert_eq!(title_words("max_5x"), "Max 5x");
        assert_eq!(title_words("team"), "Team");
    }
}
