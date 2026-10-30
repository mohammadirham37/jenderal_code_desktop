//! `todo_write`: the model keeps a checklist for multi-step work. The list
//! lives in the tool call arguments (persisted with the tool run), so the UI
//! renders the latest one; execution only validates and echoes it.

use anyhow::{bail, Result};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct Todo {
    content: String,
    status: String,
}

/// `(summary for the UI, result for the model)`.
pub fn apply(args: &Value) -> Result<(String, String)> {
    let todos: Vec<Todo> = serde_json::from_value(args.get("todos").cloned().unwrap_or(Value::Null))
        .map_err(|e| anyhow::anyhow!("`todos` must be a list of {{content, status}} items: {e}"))?;
    if todos.is_empty() {
        bail!("`todos` is empty");
    }
    let mut out = String::new();
    for t in &todos {
        let mark = match t.status.as_str() {
            "completed" => "[x]",
            "in_progress" => "[>]",
            "pending" => "[ ]",
            other => bail!("unknown status `{other}` (use pending, in_progress or completed)"),
        };
        out.push_str(&format!("{mark} {}\n", t.content.trim()));
    }
    let done = todos.iter().filter(|t| t.status == "completed").count();
    let total = todos.len();
    Ok((
        format!("Todos: {done}/{total} done"),
        format!("Todo list updated ({done}/{total} done):\n{out}Keep it current: mark items in_progress/completed as you go."),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_and_validates() {
        let (summary, text) = apply(&json!({"todos": [
            {"content": "Read config", "status": "completed"},
            {"content": "Add route", "status": "in_progress"},
            {"content": "Run tests", "status": "pending"}
        ]}))
        .unwrap();
        assert_eq!(summary, "Todos: 1/3 done");
        assert!(text.contains("[x] Read config\n[>] Add route\n[ ] Run tests"));
        assert!(apply(&json!({"todos": []})).is_err());
        assert!(apply(&json!({"todos": [{"content": "x", "status": "done"}]})).is_err());
        assert!(apply(&json!({})).is_err());
    }
}
