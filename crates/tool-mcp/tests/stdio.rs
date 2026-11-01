//! End-to-end over stdio against a small fake server (needs `python3`).

use jcode_tool_mcp::{McpManager, Scope, ServerConfig, ServerEntry, ServerState, Transport};
use serde_json::json;

const FAKE_SERVER: &str = r#"
import json, sys
def send(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
print("not json: servers sometimes log to stdout", flush=True)
for line in sys.stdin:
    m = json.loads(line)
    method, mid = m.get("method"), m.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {
            "protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
            "serverInfo": {"name": "fake", "version": "1.0"},
            "instructions": "Use echo to repeat text."}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": mid, "result": {"tools": [
            {"name": "echo", "description": "Repeat text", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
            {"name": "fail", "description": "Always fails"}]}})
    elif method == "tools/call":
        # Ask the client something first; the call is answered after the pong.
        send({"jsonrpc": "2.0", "id": "srv-1", "method": "ping"})
        pong = json.loads(sys.stdin.readline())
        assert pong.get("id") == "srv-1" and "result" in pong, pong
        name, args = m["params"]["name"], m["params"]["arguments"]
        if name == "echo":
            send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": "echo: " + args["text"]}]}})
        else:
            send({"jsonrpc": "2.0", "id": mid, "result": {"isError": True, "content": [{"type": "text", "text": "boom"}]}})
"#;

fn python() -> Option<&'static str> {
    std::process::Command::new("python3").arg("--version").output().ok().map(|_| "python3")
}

#[tokio::test]
async fn stdio_server_end_to_end() {
    let Some(py) = python() else {
        eprintln!("python3 not found; skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("server.py"), FAKE_SERVER).unwrap();
    let entry = |args: Vec<&str>| ServerEntry {
        scope: Scope::Global,
        config: ServerConfig {
            name: "fake".into(),
            transport: Transport::Stdio,
            command: py.into(),
            args: args.into_iter().map(String::from).collect(),
            env: Default::default(),
            cwd: None,
            url: String::new(),
            headers: Default::default(),
        },
    };
    let mgr = McpManager::new();
    let status = mgr.sync(dir.path(), &[entry(vec!["server.py"])]).await;
    assert!(matches!(status[0].state, ServerState::Ready), "{status:?}");
    assert_eq!(status[0].server_name.as_deref(), Some("fake"));
    let tools = mgr.tools(dir.path());
    assert_eq!(tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["mcp__fake__echo", "mcp__fake__fail"]);
    assert_eq!(mgr.instructions(dir.path())[0].1, "Use echo to repeat text.");

    let (text, is_error) = mgr.call(dir.path(), "mcp__fake__echo", json!({"text": "hi"})).await.unwrap();
    assert_eq!((text.as_str(), is_error), ("echo: hi", false));
    let (text, is_error) = mgr.call(dir.path(), "mcp__fake__fail", json!({})).await.unwrap();
    assert_eq!((text.as_str(), is_error), ("boom", true));

    // A broken config fails with the process's stderr in the error.
    let status = mgr.sync(dir.path(), &[entry(vec!["missing.py"])]).await;
    assert!(matches!(status[0].state, ServerState::Failed), "{status:?}");
    assert!(status[0].error.as_deref().unwrap_or_default().contains("missing.py"), "{status:?}");
    assert!(mgr.tools(dir.path()).is_empty());

    // Removing the server stops it.
    assert!(mgr.sync(dir.path(), &[]).await.is_empty());
}

#[tokio::test]
async fn missing_program_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = McpManager::new();
    let status = mgr
        .sync(
            dir.path(),
            &[ServerEntry {
                scope: Scope::Project,
                config: ServerConfig {
                    name: "nope".into(),
                    transport: Transport::Stdio,
                    command: "definitely-not-a-real-program-xyz".into(),
                    args: vec![],
                    env: Default::default(),
                    cwd: None,
                    url: String::new(),
                    headers: Default::default(),
                },
            }],
        )
        .await;
    assert!(status[0].error.as_deref().unwrap().contains("not found on your PATH"), "{status:?}");
}

/// Live: `cargo test -p jcode-tool-mcp live_ -- --ignored` (downloads
/// @playwright/mcp with npx).
#[tokio::test]
#[ignore]
async fn live_playwright_lists_and_calls_tools() {
    jcode_tool_terminal::user_path::init_from_login_shell();
    let dir = tempfile::tempdir().unwrap();
    let mgr = McpManager::new();
    let status = mgr
        .sync(
            dir.path(),
            &[ServerEntry {
                scope: Scope::Global,
                config: ServerConfig {
                    name: "playwright".into(),
                    transport: Transport::Stdio,
                    command: "npx".into(),
                    args: vec!["-y".into(), "@playwright/mcp@latest".into(), "--headless".into()],
                    env: Default::default(),
                    cwd: None,
                    url: String::new(),
                    headers: Default::default(),
                },
            }],
        )
        .await;
    assert!(matches!(status[0].state, ServerState::Ready), "{status:?}");
    let names: Vec<String> = mgr.tools(dir.path()).into_iter().map(|t| t.name).collect();
    eprintln!("{} tools: {names:?}", names.len());
    assert!(names.iter().any(|n| n == "mcp__playwright__browser_navigate"));
    let (text, is_error) = mgr
        .call(dir.path(), "mcp__playwright__browser_navigate", json!({"url": "data:text/html,<title>JC</title><h1>Hello MCP</h1>"}))
        .await
        .unwrap();
    eprintln!("{text}");
    assert!(!is_error, "{text}");
    assert!(text.contains("Hello MCP"), "{text}");
    mgr.shutdown();
}
