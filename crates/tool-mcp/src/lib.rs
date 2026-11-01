//! Model Context Protocol servers as agent tools (Laravel Boost, Playwright,
//! ...). [`McpManager`] keeps one connection per project root + server,
//! restarts a server when its config changes, and exposes the tools under
//! model-safe names (`mcp__server__tool`).

pub mod client;
pub mod config;

pub use client::{Client, ServerInfo, Tool};
pub use config::{ServerConfig, Transport};

use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Starting a server may download it first (`npx`, `uvx`).
const START_TIMEOUT: Duration = Duration::from_secs(90);
const LIST_TIMEOUT: Duration = Duration::from_secs(30);
pub const CALL_TIMEOUT: Duration = Duration::from_secs(300);
/// Prefix of every MCP tool name the model sees.
pub const TOOL_PREFIX: &str = "mcp__";
/// OpenAI and Anthropic cap tool names at 64 characters.
const MAX_TOOL_NAME: usize = 64;

/// Where a server's config came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Global,
    Project,
}

/// A server to run for a project.
#[derive(Debug, Clone)]
pub struct ServerEntry {
    pub config: ServerConfig,
    pub scope: Scope,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerState {
    Starting,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolInfo {
    /// The name the model calls (`mcp__server__tool`).
    pub name: String,
    /// The server's own name for it.
    pub tool: String,
    pub description: String,
}

/// A server's state for the settings UI.
#[derive(Debug, Clone, Serialize)]
pub struct ServerStatus {
    pub name: String,
    pub scope: Scope,
    pub summary: String,
    pub state: ServerState,
    pub error: Option<String>,
    pub server_name: Option<String>,
    pub tools: Vec<ToolInfo>,
}

/// A tool ready to offer to the model.
#[derive(Debug, Clone)]
pub struct McpToolSpec {
    pub name: String,
    pub server: String,
    pub description: String,
    pub input_schema: Value,
}

struct Connected {
    client: Arc<Client>,
    info: ServerInfo,
    tools: Vec<Tool>,
}

enum Slot {
    Starting,
    Ready(Arc<Connected>),
    Failed(String),
}

struct Server {
    entry: ServerEntry,
    fingerprint: u64,
    slot: Slot,
}

type Key = (PathBuf, String);

#[derive(Default)]
pub struct McpManager {
    servers: Mutex<HashMap<Key, Server>>,
    /// One `sync` at a time, so a server is never started twice.
    syncing: tokio::sync::Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl McpManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make the running servers of `root` match `entries`: start new or
    /// changed ones (and ones that died), stop removed ones. Returns once
    /// every server is ready or has failed.
    pub async fn sync(&self, root: &Path, entries: &[ServerEntry]) -> Vec<ServerStatus> {
        let _one_at_a_time = self.syncing.lock().await;
        let mut to_start: Vec<ServerEntry> = Vec::new();
        {
            let mut servers = lock(&self.servers);
            servers.retain(|(r, name), server| {
                let keep = r != root || entries.iter().any(|e| &e.config.name == name);
                if !keep {
                    shutdown_slot(&server.slot);
                }
                keep
            });
            for entry in entries {
                let key = (root.to_path_buf(), entry.config.name.clone());
                let fingerprint = entry.config.fingerprint();
                let current = servers.get(&key);
                let fresh = current.is_some_and(|s| {
                    s.fingerprint == fingerprint
                        && match &s.slot {
                            Slot::Ready(c) => c.client.is_alive(),
                            Slot::Failed(_) => true, // retried via `restart`
                            Slot::Starting => true,
                        }
                });
                if fresh {
                    servers.get_mut(&key).unwrap().entry = entry.clone();
                    continue;
                }
                if let Some(old) = current {
                    shutdown_slot(&old.slot);
                }
                servers.insert(key, Server { entry: entry.clone(), fingerprint, slot: Slot::Starting });
                to_start.push(entry.clone());
            }
        }
        let started = futures::future::join_all(to_start.iter().map(|e| start(&e.config, root))).await;
        {
            let mut servers = lock(&self.servers);
            for (entry, result) in to_start.iter().zip(started) {
                let key = (root.to_path_buf(), entry.config.name.clone());
                if let Some(server) = servers.get_mut(&key) {
                    server.slot = match result {
                        Ok(c) => Slot::Ready(Arc::new(c)),
                        Err(e) => Slot::Failed(format!("{e:#}")),
                    };
                }
            }
        }
        self.refresh_changed_tools(root).await;
        self.status(root)
    }

    /// Restart one server (after a failure, or to pick up a new version).
    pub async fn restart(&self, root: &Path, name: &str) -> Vec<ServerStatus> {
        let entries: Vec<ServerEntry> = {
            let mut servers = lock(&self.servers);
            let entries = servers
                .iter()
                .filter(|((r, _), _)| r == root)
                .map(|(_, s)| s.entry.clone())
                .collect();
            if let Some(s) = servers.remove(&(root.to_path_buf(), name.to_string())) {
                shutdown_slot(&s.slot);
            }
            entries
        };
        self.sync(root, &entries).await
    }

    pub fn status(&self, root: &Path) -> Vec<ServerStatus> {
        let servers = lock(&self.servers);
        let mut list: Vec<ServerStatus> = servers
            .iter()
            .filter(|((r, _), _)| r == root)
            .map(|((_, name), s)| {
                let (state, error, server_name, tools) = match &s.slot {
                    Slot::Starting => (ServerState::Starting, None, None, vec![]),
                    Slot::Failed(e) => (ServerState::Failed, Some(e.clone()), None, vec![]),
                    Slot::Ready(c) if !c.client.is_alive() => {
                        let tail = c.client.stderr_tail();
                        let error = if tail.is_empty() { "the server exited".to_string() } else { format!("the server exited:\n{tail}") };
                        (ServerState::Failed, Some(error), Some(c.info.name.clone()), vec![])
                    }
                    Slot::Ready(c) => (
                        ServerState::Ready,
                        None,
                        Some(c.info.name.clone()),
                        c.tools
                            .iter()
                            .map(|t| ToolInfo {
                                name: tool_name(name, &t.name),
                                tool: t.name.clone(),
                                description: t.description.clone(),
                            })
                            .collect(),
                    ),
                };
                ServerStatus {
                    name: name.clone(),
                    scope: s.entry.scope,
                    summary: s.entry.config.summary(),
                    state,
                    error,
                    server_name,
                    tools,
                }
            })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    /// Tools of the ready servers of `root`, for the model.
    pub fn tools(&self, root: &Path) -> Vec<McpToolSpec> {
        let servers = lock(&self.servers);
        let mut out: Vec<McpToolSpec> = Vec::new();
        let mut names: Vec<(&String, &Server)> = servers
            .iter()
            .filter(|((r, _), _)| r == root)
            .map(|((_, n), s)| (n, s))
            .collect();
        names.sort_by(|a, b| a.0.cmp(b.0));
        for (server, s) in names {
            let Slot::Ready(c) = &s.slot else { continue };
            if !c.client.is_alive() {
                continue;
            }
            for t in &c.tools {
                out.push(McpToolSpec {
                    name: tool_name(server, &t.name),
                    server: server.clone(),
                    description: t.description.clone(),
                    input_schema: t.input_schema.clone(),
                });
            }
        }
        out
    }

    /// Usage instructions of the ready servers (for the system prompt).
    pub fn instructions(&self, root: &Path) -> Vec<(String, String)> {
        let servers = lock(&self.servers);
        let mut out: Vec<(String, String)> = servers
            .iter()
            .filter(|((r, _), _)| r == root)
            .filter_map(|((_, name), s)| match &s.slot {
                Slot::Ready(c) => c.info.instructions.clone().map(|i| (name.clone(), i)),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    /// Call a tool by its model-facing name.
    pub async fn call(&self, root: &Path, name: &str, arguments: Value) -> Result<(String, bool)> {
        let (client, tool) = {
            let servers = lock(&self.servers);
            servers
                .iter()
                .filter(|((r, _), _)| r == root)
                .find_map(|((_, server), s)| match &s.slot {
                    Slot::Ready(c) => c
                        .tools
                        .iter()
                        .find(|t| tool_name(server, &t.name) == name)
                        .map(|t| (c.client.clone(), t.name.clone())),
                    _ => None,
                })
                .ok_or_else(|| anyhow!("unknown MCP tool `{name}` (its server may have stopped)"))?
        };
        if !client.is_alive() {
            bail!("the MCP server for `{name}` is not running; restart it in Settings → MCP");
        }
        client.call_tool(&tool, arguments, CALL_TIMEOUT).await
    }

    /// Re-list tools of servers that announced a change.
    async fn refresh_changed_tools(&self, root: &Path) {
        let changed: Vec<(String, Arc<Connected>)> = {
            let servers = lock(&self.servers);
            servers
                .iter()
                .filter(|((r, _), _)| r == root)
                .filter_map(|((_, n), s)| match &s.slot {
                    Slot::Ready(c) if c.client.take_tools_changed() => Some((n.clone(), c.clone())),
                    _ => None,
                })
                .collect()
        };
        for (name, c) in changed {
            if let Ok(tools) = c.client.list_tools(LIST_TIMEOUT).await {
                let updated = Connected { client: c.client.clone(), info: c.info.clone(), tools };
                if let Some(s) = lock(&self.servers).get_mut(&(root.to_path_buf(), name)) {
                    s.slot = Slot::Ready(Arc::new(updated));
                }
            }
        }
    }

    /// Stop every server (app exit).
    pub fn shutdown(&self) {
        for (_, s) in lock(&self.servers).drain() {
            shutdown_slot(&s.slot);
        }
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn shutdown_slot(slot: &Slot) {
    if let Slot::Ready(c) = slot {
        c.client.shutdown();
    }
}

async fn start(config: &ServerConfig, root: &Path) -> Result<Connected> {
    let client = Client::connect(config, root)?;
    let init = async {
        let info = client.initialize(START_TIMEOUT).await?;
        let tools = client.list_tools(LIST_TIMEOUT).await?;
        anyhow::Ok((info, tools))
    };
    match init.await {
        Ok((info, tools)) => {
            client.take_tools_changed();
            Ok(Connected { client, info, tools })
        }
        Err(e) => {
            // Give the process a moment to print why it failed.
            tokio::time::sleep(Duration::from_millis(200)).await;
            let tail = client.stderr_tail();
            client.shutdown();
            if tail.trim().is_empty() {
                Err(e)
            } else {
                Err(anyhow!("{e:#}\n{tail}"))
            }
        }
    }
}

/// `mcp__server__tool`, restricted to `[A-Za-z0-9_-]` and 64 characters
/// (long names are cut and get a hash suffix so they stay unique).
pub fn tool_name(server: &str, tool: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect()
    };
    let full = format!("{TOOL_PREFIX}{}__{}", clean(server), clean(tool));
    if full.len() <= MAX_TOOL_NAME {
        return full;
    }
    let hash = format!("{:08x}", config::fnv1a(format!("{server}\0{tool}").as_bytes()) as u32);
    format!("{}_{hash}", &full[..MAX_TOOL_NAME - hash.len() - 1])
}

pub fn is_mcp_tool(name: &str) -> bool {
    name.starts_with(TOOL_PREFIX)
}

/// The server part of a model-facing tool name (for display).
pub fn server_of(name: &str) -> Option<&str> {
    name.strip_prefix(TOOL_PREFIX)?.split("__").next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_model_safe() {
        assert_eq!(tool_name("laravel-boost", "database-query"), "mcp__laravel-boost__database-query");
        assert_eq!(tool_name("my server", "do.thing"), "mcp__my_server__do_thing");
        let long = tool_name("playwright", &"x".repeat(80));
        assert_eq!(long.len(), 64);
        assert_ne!(long, tool_name("playwright", &"x".repeat(81)));
        assert_eq!(server_of("mcp__playwright__browser_click"), Some("playwright"));
        assert!(is_mcp_tool("mcp__a__b") && !is_mcp_tool("fs_read"));
    }
}
