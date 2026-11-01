//! MCP server configuration in the `.mcp.json` format Claude Code (and
//! Laravel Boost, Cursor, ...) use: `{"mcpServers": {"name": {...}}}`.
//! VS Code's `{"servers": {...}}` is read too.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// A local process speaking JSON-RPC over stdin/stdout.
    Stdio,
    /// Streamable HTTP (one endpoint, JSON or SSE responses).
    Http,
    /// The legacy HTTP+SSE transport (not supported; reported on connect).
    Sse,
}

/// One server. Flat so the settings UI can edit it directly; `command`/
/// `args`/`env`/`cwd` apply to stdio, `url`/`headers` to HTTP.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub name: String,
    pub transport: Transport,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Working directory, relative to the project root (default: the root).
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl ServerConfig {
    /// Stable hash of the whole config: a changed config restarts the server.
    pub fn fingerprint(&self) -> u64 {
        fnv1a(serde_json::to_string(self).unwrap_or_default().as_bytes())
    }

    /// Short description for the UI (`npx @playwright/mcp@latest`, a URL).
    pub fn summary(&self) -> String {
        match self.transport {
            Transport::Stdio => std::iter::once(self.command.as_str())
                .chain(self.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
            Transport::Http | Transport::Sse => self.url.clone(),
        }
    }
}

/// FNV-1a: deterministic across builds (unlike `DefaultHasher`), so stored
/// fingerprints stay valid after an update.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Parse a config file's content (servers sorted by name).
pub fn parse(text: &str) -> Result<Vec<ServerConfig>> {
    let root: Value = serde_json::from_str(text).context("invalid JSON")?;
    let servers = root
        .get("mcpServers")
        .or_else(|| root.get("servers"))
        .and_then(Value::as_object);
    let Some(servers) = servers else {
        if root.as_object().is_some_and(|o| o.is_empty()) {
            return Ok(vec![]);
        }
        bail!("expected an object with \"mcpServers\"");
    };
    servers.iter().map(|(name, v)| parse_server(name, v)).collect()
}

fn parse_server(name: &str, v: &Value) -> Result<ServerConfig> {
    let strings = |key: &str| -> BTreeMap<String, String> {
        v.get(key)
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    };
    let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let url = text("url");
    let transport = match v.get("type").and_then(Value::as_str) {
        Some("stdio") => Transport::Stdio,
        Some("http" | "streamable-http" | "streamableHttp" | "streamable_http") => Transport::Http,
        Some("sse") => Transport::Sse,
        Some(other) => bail!("server \"{name}\": unknown type \"{other}\""),
        None if !url.is_empty() => Transport::Http,
        None => Transport::Stdio,
    };
    let config = ServerConfig {
        name: name.to_string(),
        transport,
        command: text("command"),
        args: v
            .get("args")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        env: strings("env"),
        cwd: v.get("cwd").and_then(Value::as_str).map(String::from),
        url,
        headers: strings("headers"),
    };
    validate(&config)?;
    Ok(config)
}

pub fn validate(c: &ServerConfig) -> Result<()> {
    if c.name.trim().is_empty() {
        bail!("a server needs a name");
    }
    match c.transport {
        Transport::Stdio if c.command.trim().is_empty() => bail!("server \"{}\": missing \"command\"", c.name),
        Transport::Http | Transport::Sse
            if !(c.url.starts_with("http://") || c.url.starts_with("https://")) =>
        {
            bail!("server \"{}\": \"url\" must be http(s)", c.name)
        }
        _ => Ok(()),
    }
}

/// Serialize back to the `.mcp.json` format.
pub fn to_json(servers: &[ServerConfig]) -> String {
    let mut map = Map::new();
    for s in servers {
        let mut o = Map::new();
        match s.transport {
            Transport::Stdio => {
                o.insert("command".into(), json!(s.command));
                if !s.args.is_empty() {
                    o.insert("args".into(), json!(s.args));
                }
                if !s.env.is_empty() {
                    o.insert("env".into(), json!(s.env));
                }
                if let Some(cwd) = &s.cwd {
                    o.insert("cwd".into(), json!(cwd));
                }
            }
            Transport::Http | Transport::Sse => {
                o.insert("type".into(), json!(if s.transport == Transport::Sse { "sse" } else { "http" }));
                o.insert("url".into(), json!(s.url));
                if !s.headers.is_empty() {
                    o.insert("headers".into(), json!(s.headers));
                }
            }
        }
        map.insert(s.name.clone(), Value::Object(o));
    }
    serde_json::to_string_pretty(&json!({ "mcpServers": map })).unwrap_or_default() + "\n"
}

/// The user's global servers: `~/.config/jenderalcode/mcp.json`.
pub fn global_path(home: &Path) -> PathBuf {
    home.join(".config/jenderalcode/mcp.json")
}

/// The project's servers: `.mcp.json` at the project root.
pub fn project_path(root: &Path) -> PathBuf {
    root.join(".mcp.json")
}

/// Read a config file; a missing file is an empty list.
pub fn load(path: &Path) -> Result<Vec<ServerConfig>> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).with_context(|| format!("reading {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write a config file readable only by the user (it may hold tokens).
pub fn save(path: &Path, servers: &[ServerConfig]) -> Result<()> {
    for s in servers {
        validate(s)?;
    }
    let mut names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    if names.windows(2).any(|w| w[0] == w[1]) {
        bail!("server names must be unique");
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, to_json(servers))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_claude_code_and_vscode_formats() {
        let servers = parse(
            r#"{"mcpServers": {
                "laravel-boost": {"command": "php", "args": ["artisan", "boost:mcp"]},
                "remote": {"type": "http", "url": "https://mcp.example.com/mcp", "headers": {"Authorization": "Bearer x"}},
                "implicit": {"url": "https://a.example/mcp"}
            }}"#,
        )
        .unwrap();
        assert_eq!(servers.len(), 3);
        assert_eq!(servers[0].name, "implicit"); // serde_json maps sort keys
        let boost = servers.iter().find(|s| s.name == "laravel-boost").unwrap();
        assert_eq!(boost.transport, Transport::Stdio);
        assert_eq!(boost.summary(), "php artisan boost:mcp");
        let remote = servers.iter().find(|s| s.name == "remote").unwrap();
        assert_eq!(remote.transport, Transport::Http);
        assert_eq!(remote.headers["Authorization"], "Bearer x");
        assert_eq!(servers[0].transport, Transport::Http);

        let vscode = parse(r#"{"servers": {"pw": {"type": "stdio", "command": "npx", "args": ["@playwright/mcp@latest"]}}}"#).unwrap();
        assert_eq!(vscode[0].summary(), "npx @playwright/mcp@latest");
    }

    #[test]
    fn rejects_incomplete_servers() {
        assert!(parse(r#"{"mcpServers": {"x": {"args": []}}}"#).is_err());
        assert!(parse(r#"{"mcpServers": {"x": {"type": "http", "url": "file:///etc"}}}"#).is_err());
        assert!(parse(r#"{"mcpServers": {"x": {"type": "carrier-pigeon"}}}"#).is_err());
        assert!(parse("{}").unwrap().is_empty());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/mcp.json");
        let servers = parse(
            r#"{"mcpServers": {
                "a": {"command": "npx", "args": ["-y", "pkg"], "env": {"K": "v"}, "cwd": "tools"},
                "b": {"type": "http", "url": "https://x.example/mcp"}
            }}"#,
        )
        .unwrap();
        save(&path, &servers).unwrap();
        assert_eq!(load(&path).unwrap(), servers);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let dup = vec![servers[0].clone(), servers[0].clone()];
        assert!(save(&path, &dup).is_err());
        assert!(load(&dir.path().join("missing.json")).unwrap().is_empty());
    }

    #[test]
    fn fingerprint_tracks_changes() {
        let a = parse(r#"{"mcpServers": {"a": {"command": "x"}}}"#).unwrap().remove(0);
        let mut b = a.clone();
        assert_eq!(a.fingerprint(), b.fingerprint());
        b.args.push("--flag".into());
        assert_ne!(a.fingerprint(), b.fingerprint());
    }
}
