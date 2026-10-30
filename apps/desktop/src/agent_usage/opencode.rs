//! OpenCode runs on API providers, so it has no subscription limit. Its
//! database records tokens and cost per reply; report recent activity.

use super::format_tokens;
use jcode_common::{AgentUsageReport, AgentUsageStatus, UsageWindow};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

const HOUR_MS: i64 = 3_600_000;

pub fn db_path(home: &Path) -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"))
        .join("opencode/opencode.db")
}

pub fn report(db: &Path, now: i64) -> AgentUsageReport {
    let base = AgentUsageReport::new("opencode", "OpenCode", AgentUsageStatus::Ok);
    if !db.is_file() {
        return AgentUsageReport { status: AgentUsageStatus::Unavailable, ..base }
            .with_note("No OpenCode data yet.");
    }
    match read(db, now) {
        Ok(windows) => AgentUsageReport { windows, ..base }
            .with_note("OpenCode uses API providers, so there is no subscription limit. Showing recent activity."),
        Err(e) => AgentUsageReport { status: AgentUsageStatus::Error, ..base }
            .with_note(format!("Could not read OpenCode data: {e}")),
    }
}

fn read(db: &Path, now: i64) -> rusqlite::Result<Vec<UsageWindow>> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(std::time::Duration::from_millis(500))?;
    let mut stmt = conn.prepare(
        "SELECT
           COALESCE(SUM(COALESCE(json_extract(data,'$.tokens.input'),0)
                      + COALESCE(json_extract(data,'$.tokens.output'),0)
                      + COALESCE(json_extract(data,'$.tokens.reasoning'),0)), 0),
           COALESCE(SUM(json_extract(data,'$.cost')), 0),
           COUNT(*)
         FROM message
         WHERE time_created >= ?1 AND json_extract(data,'$.role') = 'assistant'",
    )?;
    [("Last 5 hours", 5 * HOUR_MS), ("Last 7 days", 7 * 24 * HOUR_MS)]
        .into_iter()
        .map(|(label, span)| {
            let (tokens, cost, replies): (i64, f64, i64) =
                stmt.query_row([now - span], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(UsageWindow {
                label: label.into(),
                used_percent: None,
                resets_at: None,
                detail: Some(format!(
                    "{} tokens · ${cost:.2} · {replies} {}",
                    format_tokens(tokens.max(0) as u64),
                    if replies == 1 { "reply" } else { "replies" }
                )),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_recent_assistant_messages() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);",
        )
        .unwrap();
        let now = 10 * 24 * HOUR_MS;
        let rows = [
            (now - HOUR_MS, r#"{"role":"assistant","cost":0.5,"tokens":{"input":1000,"output":200,"reasoning":50}}"#),
            (now - 2 * 24 * HOUR_MS, r#"{"role":"assistant","cost":1.0,"tokens":{"input":2000,"output":0}}"#),
            (now - HOUR_MS, r#"{"role":"user"}"#),
            (now - 9 * 24 * HOUR_MS, r#"{"role":"assistant","cost":9,"tokens":{"input":9}}"#),
        ];
        for (i, (t, data)) in rows.iter().enumerate() {
            conn.execute("INSERT INTO message VALUES (?1, 's', ?2, ?2, ?3)", rusqlite::params![i.to_string(), t, data])
                .unwrap();
        }
        drop(conn);

        let r = report(&db, now);
        assert_eq!(r.status, AgentUsageStatus::Ok);
        assert_eq!(r.windows[0].detail.as_deref(), Some("1.2k tokens · $0.50 · 1 reply"));
        assert_eq!(r.windows[1].detail.as_deref(), Some("3.2k tokens · $1.50 · 2 replies"));
        assert_eq!(report(&dir.path().join("missing.db"), now).status, AgentUsageStatus::Unavailable);
    }
}
