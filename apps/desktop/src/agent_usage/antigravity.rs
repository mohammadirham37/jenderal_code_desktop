//! Antigravity keeps its model quota on Google's servers and only shows it
//! inside `agy`; the token on disk is short-lived and refreshing it needs
//! the CLI's own client credentials. Report the signed-in account only.

use jcode_common::{AgentUsageReport, AgentUsageStatus};
use serde_json::Value;
use std::path::Path;

pub fn report(home: &Path) -> AgentUsageReport {
    let token = std::fs::read_to_string(home.join(".gemini/jetski-standalone-oauth-token"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let Some(token) = token else {
        return AgentUsageReport::new("antigravity", "Antigravity", AgentUsageStatus::NotSignedIn)
            .with_note("Sign in to Antigravity (agy) first.");
    };
    let mut r = AgentUsageReport::new("antigravity", "Antigravity", AgentUsageStatus::Unavailable)
        .with_note("Antigravity only shows its model quota inside agy.");
    r.account = token["id_token_email"].as_str().map(String::from);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_account_without_quota() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(report(home.path()).status, AgentUsageStatus::NotSignedIn);
        std::fs::create_dir_all(home.path().join(".gemini")).unwrap();
        std::fs::write(
            home.path().join(".gemini/jetski-standalone-oauth-token"),
            r#"{"token":{"access_token":"x"},"auth_method":"consumer","id_token_email":"me@example.com"}"#,
        )
        .unwrap();
        let r = report(home.path());
        assert_eq!(r.status, AgentUsageStatus::Unavailable);
        assert_eq!(r.account.as_deref(), Some("me@example.com"));
    }
}
