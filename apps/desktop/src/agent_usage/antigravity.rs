//! Antigravity quota comes from the CLI itself: `agy -p /usage
//! --output-format json` runs the `/usage` slash command (no model turn, no
//! tokens) and prints the quota groups as JSON. Versions before 1.1.11 would
//! treat `/usage` as a prompt and spend quota, so older CLIs are skipped.

use jcode_common::{AgentUsageReport, AgentUsageStatus, UsageWindow};
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MIN_VERSION: (u64, u64, u64) = (1, 1, 11);
const TIMEOUT: Duration = Duration::from_secs(30);

pub fn report(home: &Path) -> AgentUsageReport {
    let account = std::fs::read_to_string(home.join(".gemini/jetski-standalone-oauth-token"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|t| t["id_token_email"].as_str().map(String::from));
    let mut r = match account {
        None => {
            return AgentUsageReport::new("antigravity", "Antigravity", AgentUsageStatus::NotSignedIn)
                .with_note("Sign in to Antigravity (agy) first.")
        }
        Some(_) => AgentUsageReport::new("antigravity", "Antigravity", AgentUsageStatus::Ok),
    };
    r.account = account;

    let version = run_agy(home, "agy --version").and_then(|v| parse_version(&v));
    if version.is_some_and(|v| v < MIN_VERSION) {
        r.status = AgentUsageStatus::Unavailable;
        r.note = Some("Update agy (1.1.11 or newer) to see its quota here.".into());
        return r;
    }
    match run_agy(home, "agy -p /usage --output-format json --print-timeout 20s").and_then(|out| parse_usage(&out)) {
        Some(windows) => r.windows = windows,
        None => {
            r.status = AgentUsageStatus::Error;
            r.note = Some("Could not read the quota from agy. Open agy and run /usage.".into());
        }
    }
    r
}

/// Run an `agy` command line through the login shell (agy usually lives in
/// `~/.local/bin`), without letting it self-update.
fn run_agy(home: &Path, command: &str) -> Option<String> {
    let shell = crate::terminal::user_shell();
    let mut child = Command::new(&shell)
        .args(jcode_tool_terminal::user_path::interactive_shell_args(&shell))
        .args(["-c", command])
        .current_dir(home)
        .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(50)),
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

/// The last `x.y.z` in the output.
fn parse_version(out: &str) -> Option<(u64, u64, u64)> {
    out.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter_map(|w| {
            let mut p = w.split('.').map(|n| n.parse::<u64>().ok());
            match (p.next()??, p.next()??, p.next()??) {
                v if p.next().is_none() => Some(v),
                _ => None,
            }
        })
        .last()
}

/// Windows from the JSON line of a successful `/usage` command.
fn parse_usage(out: &str) -> Option<Vec<UsageWindow>> {
    let v = out
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('{'))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["status"] == "SUCCESS" && v.pointer("/command/name") == Some(&Value::from("usage")))?;
    let groups = v.pointer("/command/data/groups")?.as_array()?;
    let mut windows = Vec::new();
    for g in groups {
        let Some(group) = g["name"].as_str() else { continue };
        let buckets: Vec<&Value> = g["buckets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["disabled"] != true)
            .collect();
        for b in &buckets {
            let Some(remaining) = b["remaining_fraction"].as_f64() else { continue };
            let label = match (buckets.len(), b["window"].as_str()) {
                (1, _) => group.to_string(),
                (_, Some("5h")) => format!("{group} · 5-hour"),
                (_, Some("weekly")) => format!("{group} · Weekly"),
                _ => format!("{group} · {}", b["name"].as_str().unwrap_or("limit")),
            };
            windows.push(UsageWindow {
                label,
                used_percent: Some(((1.0 - remaining) * 100.0).clamp(0.0, 100.0)),
                resets_at: b["reset_time"]
                    .as_str()
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .map(|t| t.timestamp_millis()),
                detail: None,
            });
        }
    }
    (!windows.is_empty() || groups.is_empty()).then_some(windows)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"conversation_id":"","status":"SUCCESS","num_turns":0,"command":{"name":"usage","data":{"groups":[
        {"name":"Gemini Models","buckets":[
          {"id":"gemini-weekly","name":"Weekly Limit Remaining","window":"weekly","remaining_fraction":0.99,"reset_time":"2026-11-04T00:03:01Z"},
          {"id":"gemini-5h","name":"Five Hour Limit Remaining","window":"5h","remaining_fraction":0.75,"reset_time":"2026-10-30T16:11:59Z"}]},
        {"name":"Gemini 4 Argon","buckets":[
          {"id":"argon-weekly","name":"Gemini 4 Argon Weekly Limit","window":"weekly","remaining_fraction":0.7,"reset_time":"2026-11-03T03:54:38Z"}]},
        {"name":"Off","buckets":[{"id":"x","window":"5h","remaining_fraction":1,"disabled":true}]}]}}}"#;

    #[test]
    fn parses_usage_groups() {
        let out = format!("some banner\n{}\n", SAMPLE.replace('\n', ""));
        let w = parse_usage(&out).unwrap();
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].label, "Gemini Models · Weekly");
        assert_eq!(w[0].used_percent.map(f64::round), Some(1.0));
        assert_eq!(w[1].label, "Gemini Models · 5-hour");
        assert_eq!(w[1].used_percent, Some(25.0));
        assert_eq!(w[1].resets_at, Some(1_793_376_719_000));
        assert_eq!(w[2].label, "Gemini 4 Argon");
        assert_eq!(w[2].used_percent.map(f64::round), Some(30.0));
        assert!(parse_usage(r#"{"status":"ERROR","error":"x"}"#).is_none());
    }

    #[test]
    fn version_gate() {
        assert_eq!(parse_version("1.4.11\n"), Some((1, 4, 11)));
        assert_eq!(parse_version("agy version 1.0.9"), Some((1, 0, 9)));
        assert!(parse_version("1.0.9").unwrap() < MIN_VERSION);
        assert_eq!(parse_version("no version"), None);
    }

    #[test]
    fn not_signed_in_without_token() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(report(home.path()).status, AgentUsageStatus::NotSignedIn);
    }
}
