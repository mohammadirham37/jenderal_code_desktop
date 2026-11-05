//! "Enhance prompt": rewrites the user's draft into a clearer, more complete
//! request for the coding agent, with the session's own model, before it is
//! sent. One model call without tools; the result only replaces the draft in
//! the composer (the user reviews and sends it).

use crate::{create_provider, record_usage, AgentRuntime};
use anyhow::{bail, Context, Result};
use futures::StreamExt;
use jcode_common::*;
use std::time::Duration;

/// Instructions for the rewrite. The draft and context go in the user
/// message, marked as data.
pub const ENHANCE_INSTRUCTIONS: &str = "You improve prompts that a developer is about to send to an AI coding agent working in their project.

Rewrite the draft so the agent can act on it well:
- Keep the user's intent, scope and every concrete detail (file names, errors, commands, values). Never invent facts, files, requirements or code that the draft and context do not support.
- State the goal first, then the relevant context, constraints and what \"done\" looks like (expected behaviour, tests to pass, things to avoid) when the draft implies them.
- Turn vague words into specific asks when the meaning is clear from the draft or the recent conversation; otherwise leave them as they are rather than guessing.
- Use short paragraphs or a brief list when it helps; keep it concise. Do not pad a request that is already clear.
- Write in the same language as the draft (e.g. Indonesian stays Indonesian), in the user's voice, addressed to the agent.
- Keep `@path` mentions, code blocks and `/skill` commands exactly as written.

Reply with the improved prompt only: no preamble, no explanation, no quotes or code fence around it.";

/// Longest draft accepted (characters).
pub const MAX_DRAFT_CHARS: usize = 20_000;
const RECENT_MESSAGES: usize = 6;
const RECENT_CHARS: usize = 800;
const TIMEOUT: Duration = Duration::from_secs(120);

/// The user message for the rewrite: project name, recent conversation and
/// the draft.
pub fn enhance_request(project: &str, recent: &[MessageRecord], draft: &str) -> String {
    let mut out = format!("Project: {project}\n");
    let turns: Vec<&MessageRecord> = recent
        .iter()
        .filter(|m| matches!(m.role, Role::User | Role::Assistant) && !m.content.trim().is_empty())
        .collect();
    let start = turns.len().saturating_sub(RECENT_MESSAGES);
    if start < turns.len() {
        out.push_str("\n<recent_conversation>\n");
        for m in &turns[start..] {
            let who = if m.role == Role::User { "User" } else { "Agent" };
            let mut text: String = m.content.trim().chars().take(RECENT_CHARS).collect();
            if m.content.trim().chars().count() > RECENT_CHARS {
                text.push_str(" …");
            }
            out.push_str(&format!("{who}: {text}\n\n"));
        }
        out.push_str("</recent_conversation>\n");
    }
    out.push_str(&format!("\n<draft>\n{}\n</draft>", draft.trim()));
    out
}

/// The model's reply as a prompt: trimmed, without a wrapping code fence or
/// quotes or a "Here is…" line some models add anyway.
pub fn clean_reply(reply: &str) -> String {
    let mut s = reply.trim();
    if let Some(rest) = s.strip_prefix("```") {
        // ```lang\n…\n```
        if let (Some(nl), true) = (rest.find('\n'), rest.trim_end().ends_with("```")) {
            s = rest[nl + 1..].trim_end().trim_end_matches("```").trim();
        }
    }
    if let Some(first) = s.lines().next() {
        let l = first.trim().to_lowercase();
        let preamble = (l.starts_with("here is") || l.starts_with("here's") || l.starts_with("improved prompt"))
            && l.ends_with(':');
        if preamble {
            s = s[first.len()..].trim_start();
        }
    }
    for (open, close) in [('"', '"'), ('“', '”')] {
        if s.starts_with(open) && s.ends_with(close) && s.chars().count() > 1 {
            let inner = &s[open.len_utf8()..s.len() - close.len_utf8()];
            if !inner.contains(open) {
                s = inner.trim();
            }
        }
    }
    s.to_string()
}

impl AgentRuntime {
    /// Rewrite `draft` with the session's model (see [`ENHANCE_INSTRUCTIONS`]).
    pub async fn enhance_prompt(&self, session_id: &str, draft: &str) -> Result<String> {
        let draft = draft.trim();
        if draft.is_empty() {
            bail!("write a prompt first");
        }
        if draft.chars().count() > MAX_DRAFT_CHARS {
            bail!("the prompt is too long to enhance (max {MAX_DRAFT_CHARS} characters)");
        }
        let session = self.db.get_session(session_id)?.context("session not found")?;
        let provider_cfg = session
            .provider_id
            .as_deref()
            .and_then(|pid| self.db.get_provider(pid).ok().flatten())
            .context("no model selected for this session — pick one first")?;
        if !provider_cfg.enabled {
            bail!("provider {} is disabled", provider_cfg.name);
        }
        let model_name = session.model_name.clone().context("no model selected for this session")?;
        let project = self
            .db
            .get_workspace(&session.workspace_id)?
            .map(|w| w.name)
            .unwrap_or_else(|| "project".into());
        let recent = self.db.list_messages(session_id).unwrap_or_default();
        let model_info = self.db.get_model(&format!("{}/{}", provider_cfg.id, model_name)).ok().flatten();
        let provider = create_provider(&provider_cfg, self.creds.clone())?;
        let request = ModelRequest {
            model: model_name.clone(),
            messages: vec![ChatMessage::new(Role::User, enhance_request(&project, &recent, draft))],
            tools: vec![],
            system: Some(ENHANCE_INSTRUCTIONS.into()),
            temperature: None,
            max_tokens: model_info.as_ref().and_then(|m| m.capabilities.max_output_tokens).map(|m| m.min(4_096)),
        };
        let run = async {
            let mut stream = provider.stream(request).await?;
            let mut text = String::new();
            let mut usage = TokenUsage::default();
            while let Some(ev) = stream.next().await {
                match ev {
                    StreamEvent::TextDelta(t) => text.push_str(&t),
                    StreamEvent::Usage(u) => usage = u,
                    StreamEvent::Done { .. } => break,
                    _ => {}
                }
            }
            Ok::<_, anyhow::Error>((text, usage))
        };
        let (text, usage) = tokio::time::timeout(TIMEOUT, run).await.context("the model did not answer in time")??;
        record_usage(&self.db, &self.events, session_id, &provider_cfg, &model_name, &model_info, usage).await;
        let out = clean_reply(&text);
        if out.is_empty() {
            bail!("the model returned an empty prompt");
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: Role, content: &str) -> MessageRecord {
        MessageRecord {
            id: "m".into(),
            session_id: "s".into(),
            role,
            content: content.into(),
            images: vec![],
            tool_calls: vec![],
            tool_call_id: None,
            tool_name: None,
            reasoning: None,
            created_at: 0,
        }
    }

    #[test]
    fn request_has_project_recent_turns_and_draft() {
        let mut recent = vec![msg(Role::Tool, "tool output")];
        for i in 0..8 {
            recent.push(msg(if i % 2 == 0 { Role::User } else { Role::Assistant }, &format!("turn {i}")));
        }
        let r = enhance_request("shop", &recent, "  fix the bug  ");
        assert!(r.starts_with("Project: shop"));
        assert!(!r.contains("tool output"));
        assert!(!r.contains("turn 1\n"), "only the last 6 turns");
        assert!(r.contains("User: turn 2") && r.contains("Agent: turn 7"));
        assert!(r.ends_with("<draft>\nfix the bug\n</draft>"));
        // No conversation yet: no empty section.
        assert!(!enhance_request("shop", &[], "x").contains("recent_conversation"));
    }

    #[test]
    fn replies_are_cleaned() {
        assert_eq!(clean_reply("  Do X.\n"), "Do X.");
        assert_eq!(clean_reply("```markdown\nDo X.\n\nThen Y.\n```"), "Do X.\n\nThen Y.");
        assert_eq!(clean_reply("Here is the improved prompt:\nDo X."), "Do X.");
        assert_eq!(clean_reply("\"Do X.\""), "Do X.");
        assert_eq!(clean_reply("“Perbaiki bug login.”"), "Perbaiki bug login.");
        // Quotes inside are left alone; code blocks in the middle stay.
        assert_eq!(clean_reply("\"a\" and \"b\""), "\"a\" and \"b\"");
        assert_eq!(clean_reply("Run:\n```sh\nnpm test\n```"), "Run:\n```sh\nnpm test\n```");
    }
}
