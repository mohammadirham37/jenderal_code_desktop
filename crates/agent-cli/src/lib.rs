//! External coding agent CLI adapters (PRD §6.1, experimental for MVP).
//!
//! External CLIs (OpenCode, Codex, Claude Code, ...) run as separate
//! processes. Each CLI has its own protocol, so a config describes how to
//! pass the prompt and read the result. Tool execution stays under the
//! external agent's control — that is the documented trade-off versus
//! direct providers (PRD §6.2).

use anyhow::{Context, Result};
use jcode_tool_terminal::{CancelToken, ExecSpec};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// How a CLI expects the prompt.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptMode {
    /// Pass the prompt as the value of a flag, e.g. `claude -p "<prompt>"`.
    Flag(String),
    /// Pipe the prompt to stdin.
    #[default]
    Stdin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliAgentConfig {
    pub id: String,
    pub name: String,
    /// Executable name, resolved via PATH.
    pub command: String,
    #[serde(default)]
    pub prompt_mode: PromptMode,
    /// Extra static arguments (e.g. `--output-format`, `text`).
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// Hard timeout for the whole agent run.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    300_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliAgentResult {
    pub output: String,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub duration_ms: u64,
    pub timed_out: bool,
}

/// Run an external CLI agent once with a prompt. The result text is whatever
/// the CLI prints to stdout (agents are expected to run non-interactively;
/// configure `extra_args` accordingly, e.g. `-p`/`--print` modes).
pub async fn run_cli_agent(
    config: &CliAgentConfig,
    prompt: &str,
    cwd: impl AsRef<Path>,
    cancel: &CancelToken,
) -> Result<CliAgentResult> {
    let cwd = cwd.as_ref();
    let mut spec = ExecSpec::new("", cwd).with_timeout_ms(config.timeout_ms);
    let (program, args): (String, Vec<String>) = match &config.prompt_mode {
        PromptMode::Flag(flag) => {
            let mut args = config.extra_args.clone();
            args.push(flag.clone());
            args.push(prompt.to_string());
            (config.command.clone(), args)
        }
        PromptMode::Stdin => (config.command.clone(), config.extra_args.clone()),
    };
    spec.command = build_shell_command(&program, &args, matches!(config.prompt_mode, PromptMode::Stdin), prompt);
    let res = jcode_tool_terminal::run_exec(&spec, cancel)
        .await
        .with_context(|| format!("running external agent `{}`", config.command))?;
    Ok(CliAgentResult {
        output: res.output,
        exit_code: res.exit_code,
        success: res.success,
        duration_ms: res.duration_ms,
        timed_out: res.timed_out,
    })
}

fn build_shell_command(program: &str, args: &[String], stdin_prompt: bool, prompt: &str) -> String {
    let mut cmd = shell_quote(program);
    for a in args {
        cmd.push(' ');
        cmd.push_str(&shell_quote(a));
    }
    if stdin_prompt {
        // pipe a heredoc into the command
        cmd = format!("printf %s {} | {}", shell_quote(prompt), cmd);
    }
    cmd
}

fn shell_quote(s: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Well-known external agent presets for the UI.
pub fn cli_agent_presets() -> Vec<CliAgentConfig> {
    vec![
        CliAgentConfig {
            id: "claude-code".into(),
            name: "Claude Code".into(),
            command: "claude".into(),
            prompt_mode: PromptMode::Flag("-p".into()),
            extra_args: vec!["--output-format".into(), "text".into()],
            timeout_ms: default_timeout(),
        },
        CliAgentConfig {
            id: "codex".into(),
            name: "Codex CLI".into(),
            command: "codex".into(),
            prompt_mode: PromptMode::Flag("exec".into()),
            extra_args: vec![],
            timeout_ms: default_timeout(),
        },
        CliAgentConfig {
            id: "opencode".into(),
            name: "OpenCode CLI".into(),
            command: "opencode".into(),
            prompt_mode: PromptMode::Flag("run".into()),
            extra_args: vec![],
            timeout_ms: default_timeout(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_a_cli_and_captures_stdout() {
        // `echo` stands in for an agent binary: prompt via stdin mode.
        let config = CliAgentConfig {
            id: "t".into(),
            name: "test".into(),
            command: "cat".into(),
            prompt_mode: PromptMode::Stdin,
            extra_args: vec![],
            timeout_ms: 30_000,
        };
        let dir = std::env::temp_dir();
        let res = run_cli_agent(&config, "agent says hi", &dir, &CancelToken::new())
            .await
            .unwrap();
        assert!(res.success);
        assert!(res.output.contains("agent says hi"));
    }

    #[test]
    fn presets_are_valid() {
        assert!(!cli_agent_presets().is_empty());
    }
}
