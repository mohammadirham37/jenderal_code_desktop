//! JSON-RPC over stdio (Content-Length framing) with just enough of the
//! Language Server Protocol to open/change documents and collect
//! `textDocument/publishDiagnostics`.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Notify};
use tokio::time::Instant;

/// A diagnostic with 1-based line/column.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub line: u32,
    pub column: u32,
    /// 1 error, 2 warning, 3 information, 4 hint.
    pub severity: u8,
    pub message: String,
}

#[derive(Default)]
struct DiagState {
    /// Bumped on every publish for the document.
    seq: u64,
    items: Vec<Diagnostic>,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;
type Writer = Arc<tokio::sync::Mutex<Box<dyn AsyncWrite + Send + Unpin>>>;

pub struct LspClient {
    writer: Writer,
    next_id: AtomicI64,
    pending: Pending,
    diagnostics: Arc<Mutex<HashMap<String, DiagState>>>,
    published: Arc<Notify>,
    versions: Mutex<HashMap<String, i32>>,
    alive: Arc<AtomicBool>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl LspClient {
    /// Start reading `reader` in the background; messages go to `writer`.
    pub fn start<R, W>(reader: R, writer: W) -> Arc<Self>
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let client = Arc::new(Self {
            writer: Arc::new(tokio::sync::Mutex::new(Box::new(writer))),
            next_id: AtomicI64::new(1),
            pending: Arc::default(),
            diagnostics: Arc::default(),
            published: Arc::new(Notify::new()),
            versions: Mutex::default(),
            alive: Arc::new(AtomicBool::new(true)),
        });
        let (writer, pending, diagnostics, published, alive) = (
            client.writer.clone(),
            client.pending.clone(),
            client.diagnostics.clone(),
            client.published.clone(),
            client.alive.clone(),
        );
        tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            while let Ok(Some(msg)) = read_message(&mut reader).await {
                handle_message(msg, &writer, &pending, &diagnostics, &published).await;
            }
            alive.store(false, Ordering::SeqCst);
            for (_, tx) in lock(&pending).drain() {
                let _ = tx.send(Err("language server exited".into()));
            }
            published.notify_waiters();
        });
        client
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        write_message(&self.writer, &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).await?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => r.map_err(|e| anyhow!("{method}: {e}")),
            Ok(Err(_)) => bail!("{method}: language server exited"),
            Err(_) => {
                lock(&self.pending).remove(&id);
                bail!("{method}: no answer within {}s", timeout.as_secs())
            }
        }
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        write_message(&self.writer, &json!({"jsonrpc": "2.0", "method": method, "params": params})).await
    }

    pub async fn initialize(&self, root: &Path, timeout: Duration) -> Result<()> {
        let root_uri = file_uri(root);
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "rootPath": root.display().to_string(),
                "workspaceFolders": [{"uri": root_uri, "name": name}],
                "capabilities": {
                    "textDocument": {
                        "synchronization": {"didSave": true, "dynamicRegistration": false},
                        "publishDiagnostics": {"versionSupport": true, "relatedInformation": false}
                    },
                    "workspace": {"configuration": true, "workspaceFolders": true},
                    "window": {"workDoneProgress": false}
                }
            }),
            timeout,
        )
        .await?;
        self.notify("initialized", json!({})).await
    }

    /// Send the document's current text and wait (at most `wait`) for the
    /// server's diagnostics for it.
    pub async fn diagnostics(&self, path: &Path, language_id: &str, text: &str, wait: Duration) -> Result<Vec<Diagnostic>> {
        if !self.is_alive() {
            bail!("language server is not running");
        }
        let uri = file_uri(path);
        let seq_before = lock(&self.diagnostics).get(&uri).map_or(0, |d| d.seq);
        let version = {
            let mut versions = lock(&self.versions);
            let v = versions.entry(uri.clone()).or_insert(0);
            *v += 1;
            *v
        };
        if version == 1 {
            self.notify(
                "textDocument/didOpen",
                json!({"textDocument": {"uri": uri, "languageId": language_id, "version": version, "text": text}}),
            )
            .await?;
        } else {
            self.notify(
                "textDocument/didChange",
                json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}),
            )
            .await?;
        }
        self.notify("textDocument/didSave", json!({"textDocument": {"uri": uri}})).await?;

        // Wait for a fresh publish, then let follow-up publishes (servers
        // often send syntax errors first, type errors later) settle.
        let deadline = Instant::now() + wait;
        let settle = Duration::from_millis(400);
        let mut seen = seq_before;
        loop {
            let notified = self.published.notified();
            let seq = lock(&self.diagnostics).get(&uri).map_or(0, |d| d.seq);
            if seq > seen {
                seen = seq;
                let until = (Instant::now() + settle).min(deadline);
                if tokio::time::timeout_at(until, notified).await.is_err() {
                    break;
                }
                continue;
            }
            if !self.is_alive() || tokio::time::timeout_at(deadline, notified).await.is_err() {
                break;
            }
        }
        if seen == seq_before {
            bail!("no diagnostics within {}s", wait.as_secs());
        }
        Ok(lock(&self.diagnostics).get(&uri).map(|d| d.items.clone()).unwrap_or_default())
    }
}

async fn handle_message(
    msg: Value,
    writer: &Writer,
    pending: &Pending,
    diagnostics: &Mutex<HashMap<String, DiagState>>,
    published: &Notify,
) {
    let method = msg.get("method").and_then(Value::as_str);
    match (method, msg.get("id")) {
        // Response to one of our requests.
        (None, Some(id)) => {
            if let Some(tx) = id.as_i64().and_then(|id| lock(pending).remove(&id)) {
                let result = match msg.get("error") {
                    Some(e) => Err(e["message"].as_str().unwrap_or("error").to_string()),
                    None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(result);
            }
        }
        // Request from the server: answer so it does not wait forever.
        (Some(method), Some(id)) => {
            let result = match method {
                "workspace/configuration" => {
                    let n = msg.pointer("/params/items").and_then(Value::as_array).map_or(0, Vec::len);
                    Value::Array(vec![Value::Null; n])
                }
                "workspace/workspaceFolders" => Value::Array(vec![]),
                _ => Value::Null,
            };
            let _ = write_message(writer, &json!({"jsonrpc": "2.0", "id": id, "result": result})).await;
        }
        (Some("textDocument/publishDiagnostics"), None) => {
            let Some(uri) = msg.pointer("/params/uri").and_then(Value::as_str) else { return };
            let items = msg
                .pointer("/params/diagnostics")
                .and_then(Value::as_array)
                .map(|d| d.iter().filter_map(parse_diagnostic).collect())
                .unwrap_or_default();
            {
                let mut map = lock(diagnostics);
                let state = map.entry(normalize_uri(uri)).or_default();
                state.seq += 1;
                state.items = items;
            }
            published.notify_waiters();
        }
        _ => {}
    }
}

fn parse_diagnostic(d: &Value) -> Option<Diagnostic> {
    Some(Diagnostic {
        line: d.pointer("/range/start/line")?.as_u64()? as u32 + 1,
        column: d.pointer("/range/start/character")?.as_u64()? as u32 + 1,
        severity: d.get("severity").and_then(Value::as_u64).unwrap_or(1) as u8,
        message: d.get("message")?.as_str()?.trim().to_string(),
    })
}

async fn read_message<R: AsyncRead + Unpin>(reader: &mut BufReader<R>) -> Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:").or_else(|| line.strip_prefix("content-length:")) {
            length = v.trim().parse::<usize>().ok();
        }
    }
    let length = length.context("missing Content-Length")?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    Ok(Some(serde_json::from_slice(&body)?))
}

async fn write_message(writer: &Writer, msg: &Value) -> Result<()> {
    let body = serde_json::to_vec(msg)?;
    let mut w = writer.lock().await;
    w.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await?;
    Ok(())
}

/// `file://` URI with the characters servers commonly escape encoded.
pub fn file_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    let s = path.to_string_lossy().replace('\\', "/");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Re-encode a server URI the way [`file_uri`] does (servers differ in
/// which characters they escape, e.g. `:`).
fn normalize_uri(uri: &str) -> String {
    match uri.strip_prefix("file://") {
        Some(rest) => file_uri(Path::new(&percent_decode(rest))),
        None => uri.to_string(),
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    /// A fake server: answers `initialize`, asks the client for
    /// configuration, and publishes one error per didOpen/didChange.
    async fn fake_server(io: tokio::io::DuplexStream) {
        let (r, w) = tokio::io::split(io);
        let writer: Writer = Arc::new(tokio::sync::Mutex::new(Box::new(w)));
        let mut reader = BufReader::new(r);
        while let Ok(Some(msg)) = read_message(&mut reader).await {
            match msg["method"].as_str() {
                Some("initialize") => {
                    write_message(&writer, &json!({"jsonrpc":"2.0","id":msg["id"],"result":{"capabilities":{}}}))
                        .await
                        .unwrap();
                    write_message(&writer, &json!({"jsonrpc":"2.0","id":99,"method":"workspace/configuration","params":{"items":[{},{}]}}))
                        .await
                        .unwrap();
                }
                Some(m @ ("textDocument/didOpen" | "textDocument/didChange")) => {
                    let uri = msg.pointer("/params/textDocument/uri").unwrap().clone();
                    let text = if m.ends_with("Open") {
                        msg.pointer("/params/textDocument/text").unwrap().as_str().unwrap().to_string()
                    } else {
                        msg.pointer("/params/contentChanges/0/text").unwrap().as_str().unwrap().to_string()
                    };
                    let diags = if text.contains("bad") {
                        json!([{"range":{"start":{"line":2,"character":4},"end":{"line":2,"character":7}},"severity":1,"message":"Cannot find name 'bad'."}])
                    } else {
                        json!([])
                    };
                    write_message(&writer, &json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":uri,"diagnostics":diags}}))
                        .await
                        .unwrap();
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn collects_diagnostics_for_open_and_change() {
        let (client_io, server_io) = duplex(64 * 1024);
        tokio::spawn(fake_server(server_io));
        let (r, w) = tokio::io::split(client_io);
        let client = LspClient::start(r, w);
        client.initialize(Path::new("/tmp/proj dir"), Duration::from_secs(5)).await.unwrap();

        let file = Path::new("/tmp/proj dir/src/a.ts");
        let d = client.diagnostics(file, "typescript", "x\ny\n    bad\n", Duration::from_secs(5)).await.unwrap();
        assert_eq!(d, vec![Diagnostic { line: 3, column: 5, severity: 1, message: "Cannot find name 'bad'.".into() }]);
        let d = client.diagnostics(file, "typescript", "fixed\n", Duration::from_secs(5)).await.unwrap();
        assert!(d.is_empty());
    }

    #[test]
    fn uris_round_trip() {
        let p = Path::new("/Users/me/My Project/a+b.ts");
        let uri = file_uri(p);
        assert_eq!(uri, "file:///Users/me/My%20Project/a%2Bb.ts");
        assert_eq!(normalize_uri("file:///Users/me/My%20Project/a+b.ts"), uri);
    }
}
