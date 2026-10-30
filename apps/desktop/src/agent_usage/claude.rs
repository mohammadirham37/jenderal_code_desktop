//! Claude Code limits come from Anthropic's OAuth usage endpoint (what
//! Claude Code's `/usage` shows), called with the login token Claude Code
//! stores in the macOS Keychain (`~/.claude/.credentials.json` elsewhere).
//! The token is only read and only sent to api.anthropic.com; it is never
//! refreshed, because rotating it would sign Claude Code out.

use jcode_common::{AgentUsageReport, AgentUsageStatus, UsageWindow};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

#[derive(Debug, PartialEq)]
struct Credentials {
    access_token: String,
    expires_at: Option<i64>,
    subscription: Option<String>,
    tier: Option<String>,
}

pub async fn report(home: Option<&Path>, now: i64) -> AgentUsageReport {
    let home = home.map(Path::to_path_buf);
    let creds = tokio::task::spawn_blocking(move || read_credentials(home.as_deref()))
        .await
        .ok()
        .flatten();
    report_for(creds, now).await
}

async fn report_for(creds: Option<Credentials>, now: i64) -> AgentUsageReport {
    let base = AgentUsageReport::new("claude", "Claude Code", AgentUsageStatus::Ok);
    let Some(creds) = creds else {
        return AgentUsageReport { status: AgentUsageStatus::NotSignedIn, ..base }
            .with_note("Sign in to Claude Code to see its limits.");
    };
    let base = AgentUsageReport { plan: plan_label(creds.subscription.as_deref(), creds.tier.as_deref()), ..base };
    if creds.expires_at.is_some_and(|t| t <= now) {
        return expired(base);
    }
    match fetch(&creds.access_token).await {
        Ok(v) => AgentUsageReport { windows: parse_usage(&v), ..base },
        Err(FetchError::Unauthorized) => expired(base),
        Err(FetchError::Other(e)) => AgentUsageReport { status: AgentUsageStatus::Error, ..base }
            .with_note(format!("Could not load Claude usage: {e}")),
    }
}

fn expired(base: AgentUsageReport) -> AgentUsageReport {
    AgentUsageReport { status: AgentUsageStatus::Expired, ..base }
        .with_note("The Claude Code login token expired. Use Claude Code once to refresh it, then reload.")
}

enum FetchError {
    Unauthorized,
    Other(String),
}

async fn fetch(token: &str) -> Result<Value, FetchError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| FetchError::Other(e.to_string()))?;
    let resp = client
        .get(USAGE_URL)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("user-agent", "JenderalCode")
        .send()
        .await
        .map_err(|e| FetchError::Other(e.to_string()))?;
    match resp.status().as_u16() {
        401 | 403 => Err(FetchError::Unauthorized),
        s if !(200..300).contains(&s) => Err(FetchError::Other(format!("HTTP {s}"))),
        _ => resp.json().await.map_err(|e| FetchError::Other(e.to_string())),
    }
}

fn read_credentials(home: Option<&Path>) -> Option<Credentials> {
    let from_keychain = if cfg!(target_os = "macos") {
        std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
    } else {
        None
    };
    let raw = from_keychain.or_else(|| std::fs::read_to_string(home?.join(".claude/.credentials.json")).ok())?;
    parse_credentials(&raw)
}

fn parse_credentials(raw: &str) -> Option<Credentials> {
    let v: Value = serde_json::from_str(raw.trim()).ok()?;
    let o = &v["claudeAiOauth"];
    Some(Credentials {
        access_token: o["accessToken"].as_str().filter(|t| !t.is_empty())?.to_string(),
        expires_at: o["expiresAt"].as_i64(),
        subscription: o["subscriptionType"].as_str().map(String::from),
        tier: o["rateLimitTier"].as_str().map(String::from),
    })
}

/// "Team · Max 5x" from `subscriptionType` + `rateLimitTier`
/// (`default_claude_max_5x`).
fn plan_label(subscription: Option<&str>, tier: Option<&str>) -> Option<String> {
    let tier = tier
        .map(|t| t.trim_start_matches("default_").trim_start_matches("claude_"))
        .filter(|t| !t.is_empty() && *t != "default")
        .map(super::title_words);
    let parts: Vec<String> = subscription.map(super::title_words).into_iter().chain(tier).collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Windows from the `limits` list, or the older `five_hour` / `seven_day*`
/// fields when it is absent.
fn parse_usage(v: &Value) -> Vec<UsageWindow> {
    let from_limits: Vec<UsageWindow> = v["limits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| {
            let percent = l["percent"].as_f64()?;
            let model = l.pointer("/scope/model/display_name").and_then(Value::as_str);
            let label = match (l["group"].as_str(), model) {
                (Some("session"), _) => "5-hour session".to_string(),
                (Some("weekly"), Some(m)) => format!("Weekly · {m}"),
                (Some("weekly"), None) => "Weekly".to_string(),
                (group, _) => super::title_words(l["kind"].as_str().or(group).unwrap_or("limit")),
            };
            Some(UsageWindow { label, used_percent: Some(percent), resets_at: parse_time(&l["resets_at"]), detail: None })
        })
        .collect();
    if !from_limits.is_empty() {
        return from_limits;
    }
    [
        ("five_hour", "5-hour session"),
        ("seven_day", "Weekly"),
        ("seven_day_opus", "Weekly · Opus"),
        ("seven_day_sonnet", "Weekly · Sonnet"),
    ]
    .into_iter()
    .filter_map(|(key, label)| {
        let w = v.get(key).filter(|w| w.is_object())?;
        Some(UsageWindow {
            label: label.into(),
            used_percent: Some(w["utilization"].as_f64()?),
            resets_at: parse_time(&w["resets_at"]),
            detail: None,
        })
    })
    .collect()
}

fn parse_time(v: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(v.as_str()?).ok().map(|t| t.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_limits_list() {
        let v = json!({
            "five_hour": {"utilization": 2.0, "resets_at": "2026-10-30T15:39:59.727072+00:00"},
            "limits": [
                {"kind": "session", "group": "session", "percent": 2, "resets_at": "2026-10-30T15:39:59.727072+00:00", "scope": null},
                {"kind": "weekly_scoped", "group": "weekly", "percent": 0, "resets_at": "2026-11-03T20:00:00+00:00",
                 "scope": {"model": {"id": null, "display_name": "Fable"}}}
            ]
        });
        let w = parse_usage(&v);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].label, "5-hour session");
        assert_eq!(w[0].used_percent, Some(2.0));
        assert_eq!(w[0].resets_at, Some(1_793_374_799_727));
        assert_eq!(w[1].label, "Weekly · Fable");
    }

    #[test]
    fn falls_back_to_named_windows() {
        let v = json!({"five_hour": {"utilization": 40.0, "resets_at": null}, "seven_day": null});
        let w = parse_usage(&v);
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].label.as_str(), w[0].used_percent), ("5-hour session", Some(40.0)));
    }

    #[test]
    fn credentials_and_plan() {
        let c = parse_credentials(
            r#"{"claudeAiOauth":{"accessToken":"tok","expiresAt":5,"subscriptionType":"team","rateLimitTier":"default_claude_max_5x"}}"#,
        )
        .unwrap();
        assert_eq!(c.access_token, "tok");
        assert_eq!(c.expires_at, Some(5));
        assert_eq!(plan_label(c.subscription.as_deref(), c.tier.as_deref()).as_deref(), Some("Team · Max 5x"));
        assert_eq!(plan_label(Some("pro"), Some("default")).as_deref(), Some("Pro"));
        assert!(parse_credentials(r#"{"claudeAiOauth":{"accessToken":""}}"#).is_none());
    }

    #[tokio::test]
    async fn expired_or_missing_login_needs_no_request() {
        let creds = parse_credentials(r#"{"claudeAiOauth":{"accessToken":"tok","expiresAt":1,"subscriptionType":"pro"}}"#);
        let r = report_for(creds, 10).await;
        assert_eq!(r.status, AgentUsageStatus::Expired);
        assert_eq!(r.plan.as_deref(), Some("Pro"));
        assert_eq!(report_for(None, 10).await.status, AgentUsageStatus::NotSignedIn);
    }

    /// Live check against this machine's CLIs: `cargo test -p
    /// jenderalcode-desktop live_agent_usage -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_agent_usage() {
        for r in super::super::collect().await {
            println!("{r:#?}");
        }
    }
}
