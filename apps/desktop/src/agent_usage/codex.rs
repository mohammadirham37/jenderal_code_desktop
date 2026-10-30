//! Codex writes its rate limits into every session log: `token_count`
//! events in `~/.codex/sessions/**/rollout-*.jsonl` carry `rate_limits`.
//! The newest event per limit is the latest known state.

use jcode_common::{AgentUsageReport, AgentUsageStatus, UsageWindow};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Newest session logs to scan; limits for other models may only appear in
/// an older session.
const MAX_FILES: usize = 40;

#[derive(Debug, Clone, PartialEq)]
struct Limit {
    name: Option<String>,
    plan: Option<String>,
    timestamp: i64,
    /// (window minutes, used percent, resets at in Unix seconds)
    windows: Vec<(u64, f64, Option<i64>)>,
}

pub fn report(codex_dir: &Path, now: i64) -> AgentUsageReport {
    let mut limits: BTreeMap<String, Limit> = BTreeMap::new();
    for file in newest_session_files(&codex_dir.join("sessions"), MAX_FILES) {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let mut seen_in_file = std::collections::HashSet::new();
        for line in text.lines().rev().filter(|l| l.contains("\"rate_limits\"")) {
            let Some((id, limit)) = serde_json::from_str::<Value>(line).ok().and_then(|v| parse_event(&v)) else {
                continue;
            };
            if !seen_in_file.insert(id.clone()) {
                continue;
            }
            match limits.get(&id) {
                Some(existing) if existing.timestamp >= limit.timestamp => {}
                _ => {
                    limits.insert(id, limit);
                }
            }
        }
    }
    build_report(limits, now)
}

/// `(limit id, limit)` from a `token_count` event with non-null rate limits.
fn parse_event(v: &Value) -> Option<(String, Limit)> {
    let rl = v.pointer("/payload/rate_limits").filter(|r| r.is_object())?;
    let timestamp = v["timestamp"]
        .as_str()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.timestamp_millis())
        .unwrap_or(0);
    let windows: Vec<(u64, f64, Option<i64>)> = ["primary", "secondary"]
        .iter()
        .filter_map(|k| {
            let w = rl.get(*k).filter(|w| w.is_object())?;
            Some((w["window_minutes"].as_u64()?, w["used_percent"].as_f64()?, w["resets_at"].as_i64()))
        })
        .collect();
    if windows.is_empty() {
        return None;
    }
    let id = rl["limit_id"].as_str().unwrap_or("codex").to_string();
    Some((
        id,
        Limit {
            name: rl["limit_name"].as_str().map(String::from),
            plan: rl["plan_type"].as_str().map(String::from),
            timestamp,
            windows,
        },
    ))
}

fn build_report(limits: BTreeMap<String, Limit>, now: i64) -> AgentUsageReport {
    let mut report = AgentUsageReport::new("codex", "Codex", AgentUsageStatus::Ok);
    if limits.is_empty() {
        report.status = AgentUsageStatus::Unavailable;
        report.note = Some("No Codex session yet. Limits appear after you use Codex.".into());
        return report;
    }
    // The main "codex" limit first, then per-model limits.
    let mut ordered: Vec<(String, Limit)> = limits.into_iter().collect();
    ordered.sort_by_key(|(id, _)| (id != "codex", id.clone()));
    for (id, limit) in &ordered {
        let prefix = if id == "codex" { String::new() } else { format!("{} · ", limit.name.as_deref().unwrap_or(id)) };
        for &(minutes, used, resets_at) in &limit.windows {
            let resets_ms = resets_at.map(|s| s * 1000);
            let reset_passed = resets_ms.is_some_and(|r| r <= now);
            report.windows.push(UsageWindow {
                label: format!("{prefix}{}", window_label(minutes)),
                used_percent: Some(if reset_passed { 0.0 } else { used }),
                resets_at: if reset_passed { None } else { resets_ms },
                detail: reset_passed.then(|| "Reset since your last Codex session".to_string()),
            });
        }
    }
    report.plan = ordered.iter().find_map(|(_, l)| l.plan.as_deref()).map(plan_label);
    report.updated_at = ordered.iter().map(|(_, l)| l.timestamp).max().filter(|t| *t > 0);
    report.note = Some("From your latest Codex session.".into());
    report
}

fn window_label(minutes: u64) -> String {
    match minutes {
        10_080 => "Weekly limit".into(),
        m if m % 1_440 == 0 => format!("{}-day limit", m / 1_440),
        m if m % 60 == 0 => format!("{}-hour limit", m / 60),
        m => format!("{m}-minute limit"),
    }
}

fn plan_label(plan: &str) -> String {
    match plan {
        "prolite" => "Pro Lite".into(),
        other => super::title_words(other),
    }
}

/// `.jsonl` files under `dir`, newest modification first.
fn newest_session_files(dir: &Path, limit: usize) -> Vec<PathBuf> {
    let mut files: Vec<(SystemTime, PathBuf)> = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let path = e.path();
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() && path.extension().is_some_and(|x| x == "jsonl") {
                let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                files.push((mtime, path));
            }
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().take(limit).map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(ts: &str, id: &str, name: &str, used: f64, minutes: u64, resets: i64) -> String {
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","rate_limits":{{"limit_id":"{id}","limit_name":{name},"primary":{{"used_percent":{used},"window_minutes":{minutes},"resets_at":{resets}}},"secondary":null,"plan_type":"prolite"}}}}}}"#
        )
    }

    #[test]
    fn reads_newest_limits_per_id_from_session_logs() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("sessions/2026/10/02");
        std::fs::create_dir_all(&day).unwrap();
        let now = 1_790_000_000_000; // ms
        let lines = [
            event("2026-10-02T10:00:00Z", "codex", "null", 5.0, 10_080, 1_790_500_000),
            "{\"type\":\"other\"}".to_string(),
            event("2026-10-02T11:00:00Z", "codex", "null", 13.0, 10_080, 1_790_500_000),
            event("2026-10-02T11:30:00Z", "codex_spark", "\"Spark\"", 40.0, 300, 1_789_000_000),
        ];
        std::fs::write(day.join("rollout-a.jsonl"), lines.join("\n")).unwrap();

        let r = report(dir.path(), now);
        assert_eq!(r.status, AgentUsageStatus::Ok);
        assert_eq!(r.plan.as_deref(), Some("Pro Lite"));
        assert_eq!(r.windows.len(), 2);
        assert_eq!(r.windows[0].label, "Weekly limit");
        assert_eq!(r.windows[0].used_percent, Some(13.0));
        assert_eq!(r.windows[0].resets_at, Some(1_790_500_000_000));
        // The Spark window reset before "now": shown as 0% with a note.
        assert_eq!(r.windows[1].label, "Spark · 5-hour limit");
        assert_eq!(r.windows[1].used_percent, Some(0.0));
        assert_eq!(r.windows[1].resets_at, None);
        assert!(r.windows[1].detail.is_some());
    }

    #[test]
    fn no_sessions_is_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(report(dir.path(), 0).status, AgentUsageStatus::Unavailable);
    }

    #[test]
    fn window_labels() {
        assert_eq!(window_label(300), "5-hour limit");
        assert_eq!(window_label(10_080), "Weekly limit");
        assert_eq!(window_label(2_880), "2-day limit");
        assert_eq!(window_label(45), "45-minute limit");
    }
}
