//! MCP client: JSON-RPC over a child's stdio (one message per line) or over
//! streamable HTTP (POST, answered with JSON or an SSE stream).

use crate::config::{ServerConfig, Transport};
use anyhow::{anyhow, bail, Context, Result};
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Lines of a stdio server's stderr kept for error reports.
const STDERR_LINES: usize = 40;

/// A tool a server offers.
#[derive(Debug, Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// What `initialize` told us.
#[derive(Debug, Clone, Default)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    /// Usage instructions the server wants in the model's context.
    pub instructions: Option<String>,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Client {
    conn: Conn,
    next_id: AtomicI64,
    alive: Arc<AtomicBool>,
    /// Set when the server announces `notifications/tools/list_changed`.
    tools_changed: Arc<AtomicBool>,
}

enum Conn {
    Stdio(StdioConn),
    Http(HttpConn),
}

type SharedStdin = Arc<tokio::sync::Mutex<ChildStdin>>;

struct StdioConn {
    stdin: SharedStdin,
    pending: Pending,
    stderr: Arc<Mutex<VecDeque<String>>>,
    child: Mutex<Option<Child>>,
    pid: Option<u32>,
}

struct HttpConn {
    http: reqwest::Client,
    url: String,
    headers: Vec<(String, String)>,
    session: Mutex<Option<String>>,
    /// Sent after initialization (`MCP-Protocol-Version`).
    version: Mutex<Option<String>>,
}

impl Client {
    /// Start (stdio) or prepare (HTTP) a connection. Nothing is exchanged
    /// until [`Client::initialize`].
    pub fn connect(config: &ServerConfig, root: &Path) -> Result<Arc<Self>> {
        let alive = Arc::new(AtomicBool::new(true));
        let tools_changed = Arc::new(AtomicBool::new(false));
        let conn = match config.transport {
            Transport::Stdio => Conn::Stdio(spawn_stdio(config, root, alive.clone(), tools_changed.clone())?),
            Transport::Http => Conn::Http(HttpConn {
                http: reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(15))
                    .build()?,
                url: config.url.clone(),
                headers: config.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                session: Mutex::new(None),
                version: Mutex::new(None),
            }),
            Transport::Sse => bail!(
                "the legacy SSE transport is not supported; use the server's streamable HTTP endpoint (often ending in /mcp)"
            ),
        };
        Ok(Arc::new(Self {
            conn,
            next_id: AtomicI64::new(1),
            alive,
            tools_changed,
        }))
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Whether the tool list changed since the last call (and reset it).
    pub fn take_tools_changed(&self) -> bool {
        self.tools_changed.swap(false, Ordering::SeqCst)
    }

    /// The server's recent stderr output (stdio servers), for error reports.
    pub fn stderr_tail(&self) -> String {
        match &self.conn {
            Conn::Stdio(c) => lock(&c.stderr).iter().cloned().collect::<Vec<_>>().join("\n"),
            Conn::Http(_) => String::new(),
        }
    }

    pub async fn initialize(&self, timeout: Duration) -> Result<ServerInfo> {
        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "roots": { "listChanged": false } },
                    "clientInfo": { "name": "JenderalCode", "version": env!("CARGO_PKG_VERSION") },
                }),
                timeout,
            )
            .await?;
        if let Conn::Http(h) = &self.conn {
            let version = result["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION).to_string();
            *lock(&h.version) = Some(version);
        }
        self.notify("notifications/initialized", json!({})).await?;
        Ok(ServerInfo {
            name: result["serverInfo"]["name"].as_str().unwrap_or_default().to_string(),
            version: result["serverInfo"]["version"].as_str().unwrap_or_default().to_string(),
            instructions: result["instructions"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from),
        })
    }

    pub async fn list_tools(&self, timeout: Duration) -> Result<Vec<Tool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..50 {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let page = self.request("tools/list", params, timeout).await?;
            for t in page["tools"].as_array().into_iter().flatten() {
                let Some(name) = t["name"].as_str() else { continue };
                let mut schema = t["inputSchema"].clone();
                if !schema.is_object() {
                    schema = json!({ "type": "object", "properties": {} });
                }
                tools.push(Tool {
                    name: name.to_string(),
                    description: t["description"].as_str().unwrap_or_default().trim().to_string(),
                    input_schema: schema,
                });
            }
            cursor = page["nextCursor"].as_str().filter(|c| !c.is_empty()).map(String::from);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    /// Call a tool. Returns the text for the model and whether the server
    /// reported an error.
    pub async fn call_tool(&self, name: &str, arguments: Value, timeout: Duration) -> Result<(String, bool)> {
        let arguments = if arguments.is_object() { arguments } else { json!({}) };
        let result = self
            .request("tools/call", json!({ "name": name, "arguments": arguments }), timeout)
            .await?;
        Ok((format_content(&result), result["isError"].as_bool().unwrap_or(false)))
    }

    pub async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        if !self.is_alive() {
            bail!("the server is not running");
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let fut = async {
            match &self.conn {
                Conn::Stdio(c) => c.request(id, &msg).await,
                Conn::Http(h) => h.request(id, &msg, method == "initialize").await,
            }
        };
        match tokio::time::timeout(timeout, fut).await {
            Ok(r) => r.map_err(|e| anyhow!("{method}: {e:#}")),
            Err(_) => {
                if let Conn::Stdio(c) = &self.conn {
                    lock(&c.pending).remove(&id);
                }
                bail!("{method}: no answer within {}s", timeout.as_secs())
            }
        }
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        match &self.conn {
            Conn::Stdio(c) => c.write(&msg).await,
            Conn::Http(h) => h.post(&msg).await.map(drop),
        }
    }

    /// Stop the server (kills a stdio server's whole process group).
    pub fn shutdown(&self) {
        self.alive.store(false, Ordering::SeqCst);
        if let Conn::Stdio(c) = &self.conn {
            if let Some(pid) = c.pid {
                jcode_tool_terminal::kill_tree_now(pid);
            }
            if let Some(mut child) = lock(&c.child).take() {
                let _ = child.start_kill();
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------
// stdio
// ---------------------------------------------------------------------------

fn search_path() -> String {
    jcode_tool_terminal::user_path::user_path()
        .map(String::from)
        .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
}

/// Resolve a bare program name on the login-shell PATH (apps started from
/// Finder have a minimal PATH, without Homebrew, nvm, Herd, ...).
fn find_program(name: &str, path: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return Some(PathBuf::from(name));
    }
    std::env::split_paths(path).map(|d| d.join(name)).find(|p| p.is_file())
}

fn spawn_stdio(
    config: &ServerConfig,
    root: &Path,
    alive: Arc<AtomicBool>,
    tools_changed: Arc<AtomicBool>,
) -> Result<StdioConn> {
    let path = search_path();
    let program = find_program(&config.command, &path)
        .ok_or_else(|| anyhow!("`{}` was not found on your PATH", config.command))?;
    let cwd = match &config.cwd {
        Some(dir) if Path::new(dir).is_absolute() => PathBuf::from(dir),
        Some(dir) => root.join(dir),
        None => root.to_path_buf(),
    };
    let mut cmd = Command::new(&program);
    cmd.args(&config.args)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // The user's API keys stay out of servers unless the config passes them.
    for key in jcode_tool_terminal::scrubbed_env_keys() {
        cmd.env_remove(key);
    }
    cmd.env("PATH", &path);
    cmd.envs(&config.env);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd
        .spawn()
        .with_context(|| format!("starting `{}` in {}", config.summary(), cwd.display()))?;
    let pid = child.id();
    let stdin = child.stdin.take().context("no stdin")?;
    let stdout = child.stdout.take().context("no stdout")?;
    let stderr_pipe = child.stderr.take().context("no stderr")?;

    let stderr: Arc<Mutex<VecDeque<String>>> = Arc::default();
    {
        let stderr = stderr.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr_pipe).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut buf = lock(&stderr);
                if buf.len() == STDERR_LINES {
                    buf.pop_front();
                }
                buf.push_back(line);
            }
        });
    }

    let pending: Pending = Arc::default();
    let stdin: SharedStdin = Arc::new(tokio::sync::Mutex::new(stdin));
    let conn = StdioConn {
        stdin: stdin.clone(),
        pending: pending.clone(),
        stderr,
        child: Mutex::new(Some(child)),
        pid,
    };
    let root_uri = file_uri(root);
    let root_name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
                continue; // servers sometimes log to stdout
            };
            let batch = match msg {
                Value::Array(items) => items,
                other => vec![other],
            };
            for msg in batch {
                if let Some(reply) = handle_incoming(msg, &pending, &tools_changed, &root_uri, &root_name) {
                    let stdin = stdin.clone();
                    tokio::spawn(async move {
                        let _ = write_line(&stdin, &reply).await;
                    });
                }
            }
        }
        alive.store(false, Ordering::SeqCst);
        for (_, tx) in lock(&pending).drain() {
            let _ = tx.send(Err("the server exited".into()));
        }
    });
    Ok(conn)
}

async fn write_line(stdin: &SharedStdin, msg: &Value) -> Result<()> {
    let mut stdin = stdin.lock().await;
    stdin.write_all(format!("{msg}\n").as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

/// Route a message from the server. Returns the reply to a server request.
fn handle_incoming(
    msg: Value,
    pending: &Pending,
    tools_changed: &AtomicBool,
    root_uri: &str,
    root_name: &str,
) -> Option<Value> {
    let method = msg["method"].as_str();
    match (method, msg.get("id")) {
        // A response to one of our requests.
        (None, Some(id)) => {
            if let Some(tx) = id.as_i64().and_then(|id| lock(pending).remove(&id)) {
                let result = match msg.get("error") {
                    Some(e) => Err(e["message"].as_str().unwrap_or("error").to_string()),
                    None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(result);
            }
            None
        }
        // A request from the server.
        (Some(method), Some(id)) => {
            Some(match method {
                "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
                "roots/list" => json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": { "roots": [{ "uri": root_uri, "name": root_name }] },
                }),
                _ => json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": { "code": -32601, "message": format!("{method} is not supported by this client") },
                }),
            })
        }
        (Some("notifications/tools/list_changed"), None) => {
            tools_changed.store(true, Ordering::SeqCst);
            None
        }
        _ => None,
    }
}

impl StdioConn {
    async fn write(&self, msg: &Value) -> Result<()> {
        write_line(&self.stdin, msg).await
    }

    async fn request(&self, id: i64, msg: &Value) -> Result<Value> {
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        self.write(msg).await?;
        match rx.await {
            Ok(r) => r.map_err(|e| anyhow!(e)),
            Err(_) => bail!("the server exited"),
        }
    }
}

fn file_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// streamable HTTP
// ---------------------------------------------------------------------------

impl HttpConn {
    async fn post(&self, msg: &Value) -> Result<reqwest::Response> {
        let mut req = self
            .http
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        if let Some(session) = lock(&self.session).clone() {
            req = req.header("Mcp-Session-Id", session);
        }
        if let Some(version) = lock(&self.version).clone() {
            req = req.header("MCP-Protocol-Version", version);
        }
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }
        let resp = req.body(msg.to_string()).send().await?;
        if let Some(session) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            *lock(&self.session) = Some(session.to_string());
        }
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let body: String = body.chars().take(300).collect();
            match status.as_u16() {
                401 | 403 => bail!("HTTP {status}: check the server's Authorization header. {body}"),
                _ => bail!("HTTP {status}: {body}"),
            }
        }
        Ok(resp)
    }

    async fn request(&self, id: i64, msg: &Value, initialize: bool) -> Result<Value> {
        if initialize {
            *lock(&self.session) = None;
        }
        let resp = self.post(msg).await?;
        let is_sse = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("text/event-stream"));
        if !is_sse {
            let body: Value = resp.json().await.context("invalid JSON response")?;
            let found = if body.is_array() {
                body.as_array().and_then(|a| a.iter().find(|m| m["id"].as_i64() == Some(id)).cloned())
            } else {
                Some(body)
            };
            return response_result(found.ok_or_else(|| anyhow!("no response in the reply"))?);
        }
        // SSE: read events until the response to our id arrives.
        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk?));
            while let Some(end) = buf.find("\n\n").or_else(|| buf.find("\r\n\r\n")) {
                let event: String = buf.drain(..end).collect();
                buf.drain(..buf.find(|c| c != '\r' && c != '\n').unwrap_or(buf.len()));
                let data: String = event
                    .lines()
                    .filter_map(|l| l.strip_prefix("data:"))
                    .map(|d| d.strip_prefix(' ').unwrap_or(d))
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Ok(m) = serde_json::from_str::<Value>(&data) {
                    if m["id"].as_i64() == Some(id) && m.get("method").is_none() {
                        return response_result(m);
                    }
                }
            }
        }
        bail!("the stream ended without a response")
    }
}

fn response_result(msg: Value) -> Result<Value> {
    if let Some(e) = msg.get("error") {
        bail!("{}", e["message"].as_str().unwrap_or("error"));
    }
    Ok(msg.get("result").cloned().unwrap_or(Value::Null))
}

// ---------------------------------------------------------------------------
// results
// ---------------------------------------------------------------------------

/// Turn a `tools/call` result into text for the model. Images and audio
/// cannot be passed on (tool results are text), so they are named instead.
pub fn format_content(result: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    for item in result["content"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("text") => parts.push(item["text"].as_str().unwrap_or_default().to_string()),
            Some(kind @ ("image" | "audio")) => parts.push(format!(
                "[{kind} omitted ({}): this client passes only text to the model]",
                item["mimeType"].as_str().unwrap_or("unknown type")
            )),
            Some("resource") => {
                let r = &item["resource"];
                match r["text"].as_str() {
                    Some(text) => parts.push(text.to_string()),
                    None => parts.push(format!("[resource {}]", r["uri"].as_str().unwrap_or_default())),
                }
            }
            Some("resource_link") => parts.push(format!(
                "[resource link: {} {}]",
                item["uri"].as_str().unwrap_or_default(),
                item["name"].as_str().unwrap_or_default()
            )),
            _ => {}
        }
    }
    if parts.is_empty() {
        if let Some(structured) = result.get("structuredContent") {
            return serde_json::to_string_pretty(structured).unwrap_or_default();
        }
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_is_flattened_to_text() {
        let r = json!({"content": [
            {"type": "text", "text": "page title: Home"},
            {"type": "image", "data": "...", "mimeType": "image/png"},
            {"type": "resource", "resource": {"uri": "file:///a", "text": "body"}},
        ]});
        let text = format_content(&r);
        assert!(text.starts_with("page title: Home"));
        assert!(text.contains("[image omitted (image/png)"));
        assert!(text.ends_with("body"));
        assert_eq!(format_content(&json!({"content": [], "structuredContent": {"a": 1}})), "{\n  \"a\": 1\n}");
    }

    #[test]
    fn file_uris_are_escaped() {
        assert_eq!(file_uri(Path::new("/Users/me/my app")), "file:///Users/me/my%20app");
    }
}
