//! Streamable HTTP against a minimal in-process server: JSON for
//! `initialize` (which sets the session id), SSE for everything else.

use jcode_tool_mcp::{McpManager, Scope, ServerConfig, ServerEntry, ServerState, Transport};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

async fn serve(listener: TcpListener, seen_sessions: Arc<Mutex<Vec<Option<String>>>>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else { return };
        let seen = seen_sessions.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stream);
            let mut len = 0usize;
            let mut session = None;
            let mut auth = None;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap_or((line, ""));
                match k.to_ascii_lowercase().as_str() {
                    "content-length" => len = v.trim().parse().unwrap(),
                    "mcp-session-id" => session = Some(v.trim().to_string()),
                    "authorization" => auth = Some(v.trim().to_string()),
                    _ => {}
                }
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).await.unwrap();
            let msg: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(auth.as_deref(), Some("Bearer secret"));
            let method = msg["method"].as_str().unwrap_or_default().to_string();
            if method != "initialize" {
                seen.lock().unwrap().push(session);
            }
            let id = msg["id"].clone();
            let (status, ctype, extra, payload) = match method.as_str() {
                "initialize" => (
                    "200 OK",
                    "application/json",
                    "Mcp-Session-Id: sess-42\r\n",
                    json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": "2025-06-18", "serverInfo": {"name": "remote"}}}).to_string(),
                ),
                "notifications/initialized" => ("202 Accepted", "application/json", "", String::new()),
                "tools/list" => (
                    "200 OK",
                    "text/event-stream",
                    "",
                    format!(
                        "event: message\ndata: {}\n\n",
                        json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "lookup", "inputSchema": {"type": "object"}}]}})
                    ),
                ),
                _ => (
                    "200 OK",
                    "text/event-stream",
                    "",
                    // A notification first, then the response.
                    format!(
                        "data: {}\n\ndata: {}\n\n",
                        json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}}),
                        json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": "found it"}]}})
                    ),
                ),
            };
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let mut stream = reader.into_inner();
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }
}

#[tokio::test]
async fn streamable_http_end_to_end() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let sessions: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
    tokio::spawn(serve(listener, sessions.clone()));

    let dir = tempfile::tempdir().unwrap();
    let mgr = McpManager::new();
    let status = mgr
        .sync(
            dir.path(),
            &[ServerEntry {
                scope: Scope::Global,
                config: ServerConfig {
                    name: "remote".into(),
                    transport: Transport::Http,
                    command: String::new(),
                    args: vec![],
                    env: Default::default(),
                    cwd: None,
                    url,
                    headers: [("Authorization".to_string(), "Bearer secret".to_string())].into(),
                },
            }],
        )
        .await;
    assert!(matches!(status[0].state, ServerState::Ready), "{status:?}");
    assert_eq!(status[0].tools[0].name, "mcp__remote__lookup");

    let (text, is_error) = mgr.call(dir.path(), "mcp__remote__lookup", json!({"q": 1})).await.unwrap();
    assert_eq!((text.as_str(), is_error), ("found it", false));
    let sessions = sessions.lock().unwrap();
    assert!(!sessions.is_empty());
    assert!(sessions.iter().all(|s| s.as_deref() == Some("sess-42")), "{sessions:?}");
}
