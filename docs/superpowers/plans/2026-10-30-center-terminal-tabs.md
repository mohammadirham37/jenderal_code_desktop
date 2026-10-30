# Center Terminal & Agent CLI Tabs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the Chat layout's centre column an Orca-style tab strip: a permanent Chat tab plus terminal tabs and agent-CLI tabs (Claude, Codex, OpenCode, Gemini, Goose, Aider, custom) opened from a "+" menu.

**Architecture:** The backend `TerminalManager::create` gains an optional `command` that runs through the user's login shell; a new `detect_agent_clis` command checks which CLIs are on the login-shell PATH. Agent presets live in `AppSettings.agent_clis`. On the frontend, xterm instances move into a module-level registry (so scrollback survives remounts), a zustand store `useCenterTabs` owns the centre tabs per workspace, and `ChatLayout` renders a new `CenterColumn`.

**Tech Stack:** Rust (Tauri 2, portable-pty 0.9, serde), React 19 + TypeScript, zustand, xterm.js 5, Tailwind 4, lucide-react.

**Spec:** `docs/superpowers/specs/2026-10-30-center-terminal-tabs-design.md`

**Commits:** end every commit message with the trailer `Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`.

**Note on names:** `crates/agent-cli` already has `CliAgentConfig` for one-shot, non-interactive runs. The new `AgentCli` type is unrelated (interactive terminal tabs). Do not merge them.

---

## File map

| File | Change | Responsibility |
|---|---|---|
| `crates/common/src/lib.rs` | modify | `AgentCli` type, presets, `AppSettings.agent_clis` |
| `apps/desktop/src/terminal.rs` | modify | `user_shell`, `build_command`, `create(..., command)` |
| `apps/desktop/src/agent_detect.rs` | create | Pure parsing helpers + login-shell detection |
| `apps/desktop/src/commands.rs` | modify | `terminal_create` `command` arg, `detect_agent_clis` |
| `apps/desktop/src/main.rs` | modify | `mod agent_detect`, register command |
| `frontend/src/lib/ipc.ts` | modify | `AgentCli`, `agent_clis`, `terminalCreate(command)`, `detectAgentClis` |
| `frontend/src/stores/app.ts` | modify | default `agent_clis: []` |
| `frontend/src/features/terminal/xtermRegistry.ts` | create | xterm instances outside React |
| `frontend/src/features/terminal/XtermView.tsx` | create | Attach a registry xterm to a host div |
| `frontend/src/features/terminal/TerminalPanel.tsx` | modify | Use registry + `XtermView` |
| `frontend/src/features/workspace/lazy.tsx` | modify | `LazyXtermView` |
| `frontend/src/stores/centerTabs.ts` | create | Centre tab state, spawn/restart/close/reset, detection cache |
| `frontend/src/features/workspace/CenterColumn.tsx` | create | Tab strip, "+" menu, tab bodies, ⌘T |
| `frontend/src/features/workspace/ChatLayout.tsx` | modify | Render `CenterColumn` |
| `frontend/src/features/settings/AgentCliSettings.tsx` | create | Edit the agent list |
| `frontend/src/features/settings/SettingsPage.tsx` | modify | "Agent CLIs" section |
| `CLAUDE.md` | modify | Document the feature |

---

### Task 1: `AgentCli` settings type with presets

**Files:**
- Modify: `crates/common/src/lib.rs` (struct `AppSettings` ~line 452, `impl Default for AppSettings` ~line 502, `mod tests` ~line 556)

- [ ] **Step 1: Write the failing test**

Add inside `mod tests` in `crates/common/src/lib.rs`:

```rust
    /// Settings saved before agent CLIs existed must load with the presets.
    #[test]
    fn settings_without_agent_clis_get_presets() {
        let s: AppSettings = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(s.agent_clis, default_agent_clis());
        let ids: Vec<&str> = s.agent_clis.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["claude", "codex", "opencode", "gemini", "goose", "aider"]);
        assert!(s.agent_clis.iter().all(|a| a.enabled));
        assert_eq!(AppSettings::default().agent_clis, default_agent_clis());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p jcode-common settings_without_agent_clis_get_presets`
Expected: compile error, `no field agent_clis` / `cannot find function default_agent_clis`.

- [ ] **Step 3: Implement**

Directly above `#[derive(Debug, Clone, Serialize, Deserialize)] pub struct AppSettings`, add:

```rust
/// A terminal agent CLI offered in the chat column's "+" menu. `command`
/// comes only from the user's settings (never from model output) and runs
/// in a terminal tab through the user's login shell.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentCli {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default = "default_agent_enabled")]
    pub enabled: bool,
}

fn default_agent_enabled() -> bool {
    true
}

pub fn default_agent_clis() -> Vec<AgentCli> {
    [
        ("claude", "Claude", "claude"),
        ("codex", "Codex", "codex"),
        ("opencode", "OpenCode", "opencode"),
        ("gemini", "Gemini", "gemini"),
        ("goose", "Goose", "goose session"),
        ("aider", "Aider", "aider"),
    ]
    .into_iter()
    .map(|(id, name, command)| AgentCli {
        id: id.into(),
        name: name.into(),
        command: command.into(),
        enabled: true,
    })
    .collect()
}
```

Add as the last field of `AppSettings` (after `editor_word_wrap`):

```rust
    /// Agent CLIs for the chat column's "+" menu.
    #[serde(default = "default_agent_clis")]
    pub agent_clis: Vec<AgentCli>,
```

Add to `impl Default for AppSettings` after `editor_word_wrap: default_editor_word_wrap(),`:

```rust
            agent_clis: default_agent_clis(),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p jcode-common`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/common/src/lib.rs
git commit -m "Add agent CLI presets to app settings"
```

---

### Task 2: Terminals can run a command through the login shell

**Files:**
- Modify: `apps/desktop/src/terminal.rs` (`default_shell` ~line 41, `which` ~line 62, `create` ~line 75, add tests at end)
- Modify: `apps/desktop/src/commands.rs` (`terminal_create` ~line 935)

- [ ] **Step 1: Write the failing test**

Append to `apps/desktop/src/terminal.rs`:

```rust
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn argv(cmd: &CommandBuilder) -> Vec<String> {
        cmd.get_argv()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn plain_terminal_is_interactive_login_shell() {
        let a = argv(&build_command(None));
        assert_eq!(a[0], user_shell());
        assert_eq!(&a[1..], ["-l", "-i"]);
    }

    #[test]
    fn agent_command_runs_through_login_shell() {
        let a = argv(&build_command(Some("goose session")));
        assert_eq!(a[0], user_shell());
        assert_eq!(&a[1..], ["-l", "-i", "-c", "goose session"]);
    }

    #[test]
    fn blank_command_is_a_plain_shell() {
        let a = argv(&build_command(Some("   ")));
        assert_eq!(&a[1..], ["-l", "-i"]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p jenderalcode-desktop terminal::tests`
Expected: compile error, `cannot find function build_command` / `user_shell`.

- [ ] **Step 3: Implement**

Replace `default_shell` with `user_shell` + `default_shell` + `build_command`, and make `which` crate-visible:

```rust
/// The user's login shell (Unix).
pub(crate) fn user_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(target_os = "macos") {
            "/bin/zsh".to_string()
        } else {
            "/bin/bash".to_string()
        }
    })
}

fn default_shell() -> CommandBuilder {
    if cfg!(windows) {
        if let Some(pwsh) = which("pwsh.exe").or_else(|| which("powershell.exe")) {
            return CommandBuilder::new(pwsh);
        }
        return CommandBuilder::new("cmd.exe");
    }
    let mut cmd = CommandBuilder::new(user_shell());
    // Login + interactive, like Terminal.app: loads the user's profile.
    cmd.arg("-l");
    cmd.arg("-i");
    cmd
}

/// The process for a terminal: an interactive login shell, or (for agent
/// tabs) `command` run through that shell so the user's PATH and aliases
/// apply. Apps started from Finder do not inherit the terminal's PATH.
fn build_command(command: Option<&str>) -> CommandBuilder {
    let Some(command) = command.map(str::trim).filter(|c| !c.is_empty()) else {
        return default_shell();
    };
    if cfg!(windows) {
        if let Some(pwsh) = which("pwsh.exe").or_else(|| which("powershell.exe")) {
            let mut cmd = CommandBuilder::new(pwsh);
            cmd.args(["-NoLogo", "-Command", command]);
            return cmd;
        }
        let mut cmd = CommandBuilder::new("cmd.exe");
        cmd.args(["/C", command]);
        return cmd;
    }
    let mut cmd = default_shell();
    cmd.args(["-c", command]);
    cmd
}
```

Change `fn which(prog: &str)` to `pub(crate) fn which(prog: &str)`.

Change the `create` signature and its shell line:

```rust
    /// Spawn a new terminal. `id` is chosen by the frontend; `command`, when
    /// set, runs instead of an interactive shell (agent CLI tabs).
    pub fn create(
        &self,
        app: &AppHandle,
        id: &str,
        cwd: &Path,
        cols: u16,
        rows: u16,
        command: Option<&str>,
    ) -> Result<()> {
```

and replace `let mut cmd = default_shell();` inside `create` with:

```rust
        let mut cmd = build_command(command);
```

Change the `.context("spawning shell")` message to `.context("spawning terminal process")`.

In `apps/desktop/src/commands.rs`, update `terminal_create`:

```rust
#[tauri::command]
pub async fn terminal_create(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    cols: Option<u16>,
    rows: Option<u16>,
    command: Option<String>,
) -> CmdResult<()> {
    let cwd = state.active_root().unwrap_or_else(|_| PathBuf::from("."));
    let terminals = state.terminals.clone();
    blocking(move || {
        terminals
            .create(
                &app,
                &id,
                &cwd,
                cols.unwrap_or(80),
                rows.unwrap_or(24),
                command.as_deref(),
            )
            .map_err(err)
    })
    .await
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p jenderalcode-desktop terminal::tests`
Expected: 3 PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/terminal.rs apps/desktop/src/commands.rs
git commit -m "Let terminals run a command through the login shell"
```

---

### Task 3: Detect installed agent CLIs

**Files:**
- Create: `apps/desktop/src/agent_detect.rs`
- Modify: `apps/desktop/src/commands.rs` (after `terminal_list`)
- Modify: `apps/desktop/src/main.rs` (`mod` list near line 7, `generate_handler!` near line 229)

- [ ] **Step 1: Write the module with failing tests**

Create `apps/desktop/src/agent_detect.rs` with the tests and stub signatures first:

```rust
//! Which agent CLIs from the settings are installed, for the chat column's
//! "+" menu. The lookup runs in a login shell so it sees the same PATH as
//! the terminal tabs that launch them.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

/// First word of a command line if it is a plain program name or path that
/// is safe inside a single-quoted shell word.
pub fn program_name(command: &str) -> Option<&str> {
    todo!()
}

fn build_script(names: &[Option<&str>]) -> String {
    todo!()
}

fn parse_output(output: &str, len: usize) -> Vec<bool> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_name_takes_first_plain_word() {
        assert_eq!(program_name("goose session"), Some("goose"));
        assert_eq!(program_name("  claude  "), Some("claude"));
        assert_eq!(program_name("/opt/bin/aider --model x"), Some("/opt/bin/aider"));
        assert_eq!(program_name("rm'x"), None);
        assert_eq!(program_name("$(evil)"), None);
        assert_eq!(program_name("-v"), None);
        assert_eq!(program_name(""), None);
    }

    #[test]
    fn script_checks_only_valid_names() {
        assert_eq!(
            build_script(&[Some("claude"), None, Some("codex")]),
            "command -v 'claude' >/dev/null 2>&1 && echo jc:0;\
             command -v 'codex' >/dev/null 2>&1 && echo jc:2;"
        );
    }

    #[test]
    fn output_markers_map_to_indices_and_noise_is_ignored() {
        assert_eq!(
            parse_output("Welcome back!\njc:2\njc:0\njc:9\njc:x\n", 3),
            vec![true, false, true]
        );
    }

    #[cfg(unix)]
    #[test]
    fn detects_real_and_missing_programs() {
        let found = detect(&["sh -c true".into(), "jc-missing-program-xyz".into(), "".into()]);
        assert_eq!(found, vec![true, false, false]);
    }
}
```

Add `mod agent_detect;` next to `mod terminal;` in `apps/desktop/src/main.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p jenderalcode-desktop agent_detect`
Expected: compile error, `cannot find function detect` (after adding `detect`, the `todo!()` tests panic).

- [ ] **Step 3: Implement**

Replace the three stubs and add `detect` + `run_script`:

```rust
pub fn program_name(command: &str) -> Option<&str> {
    let name = command.split_whitespace().next()?;
    let plain = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-' | '/'));
    (plain && !name.starts_with('-')).then_some(name)
}

/// One `command -v` per valid name; a found program prints `jc:<index>`.
fn build_script(names: &[Option<&str>]) -> String {
    names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.map(|n| format!("command -v '{n}' >/dev/null 2>&1 && echo jc:{i};")))
        .collect()
}

/// Only `jc:<index>` lines count, so anything the user's profile prints is
/// ignored.
fn parse_output(output: &str, len: usize) -> Vec<bool> {
    let mut found = vec![false; len];
    for line in output.lines() {
        if let Some(i) = line.trim().strip_prefix("jc:").and_then(|s| s.parse::<usize>().ok()) {
            if i < len {
                found[i] = true;
            }
        }
    }
    found
}

/// For each command line, whether its program is installed. Errors and
/// timeouts report `false` (the menu still lets the user launch it).
pub fn detect(commands: &[String]) -> Vec<bool> {
    let names: Vec<Option<&str>> = commands.iter().map(|c| program_name(c)).collect();
    if cfg!(windows) {
        return names
            .iter()
            .map(|n| {
                n.is_some_and(|n| {
                    ["", ".exe", ".cmd", ".bat"]
                        .iter()
                        .any(|ext| crate::terminal::which(&format!("{n}{ext}")).is_some())
                })
            })
            .collect();
    }
    let script = build_script(&names);
    if script.is_empty() {
        return vec![false; commands.len()];
    }
    run_script(&script)
        .map(|out| parse_output(&out, commands.len()))
        .unwrap_or_else(|| vec![false; commands.len()])
}

fn run_script(script: &str) -> Option<String> {
    let mut child = Command::new(crate::terminal::user_shell())
        .args(["-l", "-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}
```

Add to `apps/desktop/src/commands.rs` right after `terminal_list`:

```rust
#[tauri::command]
pub async fn detect_agent_clis(commands: Vec<String>) -> CmdResult<Vec<bool>> {
    blocking(move || Ok(crate::agent_detect::detect(&commands))).await
}
```

Register it in `apps/desktop/src/main.rs` after `commands::terminal_list,`:

```rust
            commands::detect_agent_clis,
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p jenderalcode-desktop agent_detect`
Expected: 4 PASS. Then `cargo test --workspace` — all PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/agent_detect.rs apps/desktop/src/commands.rs apps/desktop/src/main.rs
git commit -m "Detect installed agent CLIs via the login shell"
```

---

### Task 4: IPC types and wrappers

**Files:**
- Modify: `frontend/src/lib/ipc.ts` (`AppSettings` ~line 138, `terminalCreate` ~line 459)
- Modify: `frontend/src/stores/app.ts` (default `settings` ~line 62)

- [ ] **Step 1: Update `ipc.ts`**

Above `export interface AppSettings`, add:

```ts
/** Terminal agent CLI in the chat column's "+" menu (Rust `AgentCli`). */
export interface AgentCli {
  id: string;
  name: string;
  command: string;
  enabled: boolean;
}
```

Add as the last field of `AppSettings`:

```ts
  /** Agent CLIs for the chat column's "+" menu. */
  agent_clis: AgentCli[];
```

Replace `terminalCreate` and add `detectAgentClis` after `terminalList`:

```ts
  terminalCreate: (id: string, cols?: number, rows?: number, command?: string) =>
    invoke<void>("terminal_create", { id, cols: cols ?? null, rows: rows ?? null, command: command ?? null }),
```

```ts
  detectAgentClis: (commands: string[]) => invoke<boolean[]>("detect_agent_clis", { commands }),
```

- [ ] **Step 2: Update the default settings in `stores/app.ts`**

After `editor_word_wrap: true,` in the initial `settings` object add:

```ts
    agent_clis: [],
```

- [ ] **Step 3: Type-check**

Run: `cd frontend && npx tsc --noEmit`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/lib/ipc.ts frontend/src/stores/app.ts
git commit -m "Add agent CLI types and terminal command to IPC"
```

---

### Task 5: xterm registry and `XtermView`; port `TerminalPanel`

**Files:**
- Create: `frontend/src/features/terminal/xtermRegistry.ts`
- Create: `frontend/src/features/terminal/XtermView.tsx`
- Modify: `frontend/src/features/terminal/TerminalPanel.tsx` (whole file)
- Modify: `frontend/src/features/workspace/lazy.tsx`

- [ ] **Step 1: Create `xtermRegistry.ts`**

```ts
/** xterm.js instances that live outside React. A terminal is created before
 *  its PTY (so early output is not lost) and its DOM element moves between
 *  hosts, so scrollback survives remounts (page or layout switches). */
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { api, events } from "@/lib/ipc";

interface Entry {
  term: XTerm;
  fit: FitAddon;
  el: HTMLDivElement;
  opened: boolean;
}

const entries = new Map<string, Entry>();
let listening = false;

const THEME = {
  background: "#0c0f0e",
  foreground: "#d6ddda",
  cursor: "#2fc3a5",
  cursorAccent: "#0c0f0e",
  selectionBackground: "#13a68b55",
  black: "#151a18",
  brightBlack: "#67746f",
  green: "#2fc3a5",
  brightGreen: "#62dbbf",
  cyan: "#56b6c2",
  brightCyan: "#7fd3dc",
};

function listen() {
  if (listening) return;
  listening = true;
  void events.terminalData((e) => entries.get(e.terminal_id)?.term.write(e.data));
  void events.terminalExit((e) =>
    entries.get(e.terminal_id)?.term.write("\r\n\x1b[90m[process exited]\x1b[0m\r\n"),
  );
}

export function newTerminalId(): string {
  return `term-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
}

/** Get or create the xterm for `id`. Call before `api.terminalCreate`. */
export function ensureXterm(id: string, fontSize: number): Entry {
  listen();
  let entry = entries.get(id);
  if (!entry) {
    const term = new XTerm({ fontSize, fontFamily: "var(--font-mono)", cursorBlink: true, theme: THEME });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.onData((data) => void api.terminalWrite(id, data).catch(() => {}));
    term.onResize(({ cols, rows }) => void api.terminalResize(id, cols, rows).catch(() => {}));
    const el = document.createElement("div");
    el.className = "h-full w-full";
    entry = { term, fit, el, opened: false };
    entries.set(id, entry);
  }
  return entry;
}

/** Move the terminal into `host` (opening it on first attach) and size it. */
export function attachXterm(id: string, host: HTMLElement, fontSize: number) {
  const entry = ensureXterm(id, fontSize);
  if (entry.el.parentElement !== host) host.appendChild(entry.el);
  if (!entry.opened) {
    entry.term.open(entry.el);
    entry.opened = true;
  }
  entry.term.options.fontSize = fontSize;
  fitXterm(id);
}

/** Refit to the host; skipped while hidden (zero size). */
export function fitXterm(id: string) {
  const entry = entries.get(id);
  if (entry?.opened && entry.el.clientWidth > 0 && entry.el.clientHeight > 0) entry.fit.fit();
}

export function focusXterm(id: string) {
  entries.get(id)?.term.focus();
}

export function writeXterm(id: string, text: string) {
  entries.get(id)?.term.write(text);
}

export function disposeXterm(id: string) {
  const entry = entries.get(id);
  if (!entry) return;
  entry.term.dispose();
  entry.el.remove();
  entries.delete(id);
}
```

- [ ] **Step 2: Create `XtermView.tsx`**

```tsx
/** Shows a registry-owned xterm; refits when the host resizes or becomes
 *  visible, and focuses it when it becomes the active tab. */
import { useEffect, useRef } from "react";
import { useApp } from "@/stores/app";
import { attachXterm, fitXterm, focusXterm } from "./xtermRegistry";

export function XtermView({ terminalId, active }: { terminalId: string; active: boolean }) {
  const fontSize = useApp((s) => s.settings.terminal_font_size);
  const hostRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    attachXterm(terminalId, host, fontSize);
    const ro = new ResizeObserver(() => fitXterm(terminalId));
    ro.observe(host);
    return () => ro.disconnect();
  }, [terminalId, fontSize]);

  useEffect(() => {
    if (!active) return;
    fitXterm(terminalId);
    focusXterm(terminalId);
  }, [active, terminalId]);

  return (
    <div
      ref={hostRef}
      className="terminal-host absolute inset-0"
      style={{ display: active ? "block" : "none" }}
    />
  );
}
```

- [ ] **Step 3: Rewrite `TerminalPanel.tsx` on top of the registry**

Replace the whole file:

```tsx
/** Interactive terminal panel backed by a backend PTY. */
import { useEffect, useRef, useState } from "react";
import { Plus, X } from "lucide-react";
import { api } from "@/lib/ipc";
import { useApp } from "@/stores/app";
import { IconButton } from "@/components/ui";
import { XtermView } from "./XtermView";
import { disposeXterm, ensureXterm, newTerminalId } from "./xtermRegistry";

interface TermTab {
  id: string;
  title: string;
}

export function TerminalPanel() {
  const [tabs, setTabs] = useState<TermTab[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const tabsRef = useRef<TermTab[]>([]);
  tabsRef.current = tabs;
  const creatingRef = useRef(false);

  const newTerminal = async () => {
    if (creatingRef.current) return;
    creatingRef.current = true;
    const id = newTerminalId();
    try {
      ensureXterm(id, useApp.getState().settings.terminal_font_size);
      await api.terminalCreate(id, 80, 24);
      setTabs((t) => [...t, { id, title: `shell ${t.length + 1}` }]);
      setActiveId(id);
    } catch (e) {
      disposeXterm(id);
      throw e;
    } finally {
      creatingRef.current = false;
    }
  };

  const closeTerminal = async (id: string) => {
    await api.terminalKill(id);
    disposeXterm(id);
    setTabs((t) => {
      const next = t.filter((x) => x.id !== id);
      setActiveId(next.length ? next[next.length - 1].id : null);
      return next;
    });
  };

  // Auto-create the first terminal tab.
  useEffect(() => {
    if (tabs.length === 0) void newTerminal();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabs.length === 0]);

  // Kill this panel's shells on unmount, so PTYs don't leak when the panel
  // goes away.
  useEffect(() => {
    return () => {
      for (const t of tabsRef.current) {
        void api.terminalKill(t.id).catch(() => {});
        disposeXterm(t.id);
      }
    };
  }, []);

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-8 shrink-0 items-center border-b border-neutral-200 dark:border-neutral-800">
        {tabs.map((t) => (
          <div
            key={t.id}
            onClick={() => setActiveId(t.id)}
            className={`flex cursor-pointer items-center gap-1 px-3 text-[11px] ${
              activeId === t.id
                ? "border-b-2 border-b-accent-500 font-medium"
                : "text-neutral-500 hover:text-neutral-300"
            }`}
          >
            {t.title}
            <button
              className="rounded p-0.5 hover:bg-neutral-300/60 dark:hover:bg-neutral-700"
              onClick={(e) => {
                e.stopPropagation();
                void closeTerminal(t.id);
              }}
            >
              <X size={10} />
            </button>
          </div>
        ))}
        <IconButton title="New terminal" className="ml-1 h-6 w-6" onClick={() => void newTerminal()}>
          <Plus size={12} />
        </IconButton>
      </div>
      <div className="relative min-h-0 flex-1">
        {tabs.map((t) => (
          <XtermView key={t.id} terminalId={t.id} active={activeId === t.id} />
        ))}
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Add `LazyXtermView` to `lazy.tsx`**

After `LazyTerminalPanel`:

```ts
export const LazyXtermView = lazy(() =>
  import("@/features/terminal/XtermView").then((m) => ({ default: m.XtermView })),
);
```

- [ ] **Step 5: Build**

Run: `cd frontend && npm run build`
Expected: success.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/features/terminal frontend/src/features/workspace/lazy.tsx
git commit -m "Keep xterm instances in a registry outside React"
```

---

### Task 6: `useCenterTabs` store

**Files:**
- Create: `frontend/src/stores/centerTabs.ts`

- [ ] **Step 1: Create the store**

```ts
/** Terminal and agent-CLI tabs in the Chat layout's centre column. Tabs
 *  belong to the open workspace: they survive page and layout switches and
 *  are killed when the workspace changes or closes. */
import { create } from "zustand";
import { api, events, type AgentCli } from "@/lib/ipc";
import { useApp } from "./app";

export const CHAT_TAB = "chat";

export interface CenterTab {
  id: string;
  title: string;
  /** Agent CLI command; absent for a plain shell. */
  command?: string;
  agentId?: string;
  exited: boolean;
  error?: string;
}

interface CenterTabsState {
  tabs: CenterTab[];
  activeId: string;
  /** Installed state by agent id; `null` until detected. */
  installed: Record<string, boolean> | null;
  setActive: (id: string) => void;
  openTerminal: () => Promise<void>;
  openAgent: (agent: AgentCli) => Promise<void>;
  restart: (id: string) => Promise<void>;
  close: (id: string) => Promise<void>;
  detectInstalled: () => Promise<void>;
  reset: () => void;
}

// xterm.js stays out of the initial bundle until a tab is opened.
const registry = () => import("@/features/terminal/xtermRegistry");

const newTabId = () => `center-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;

async function spawn(tab: CenterTab) {
  (await registry()).ensureXterm(tab.id, useApp.getState().settings.terminal_font_size);
  await api.terminalCreate(tab.id, 80, 24, tab.command);
}

let exitListening = false;

export const useCenterTabs = create<CenterTabsState>((set, get) => {
  const patch = (id: string, p: Partial<CenterTab>) =>
    set((s) => ({ tabs: s.tabs.map((t) => (t.id === id ? { ...t, ...p } : t)) }));

  const open = async (tab: CenterTab) => {
    if (!exitListening) {
      exitListening = true;
      void events.terminalExit((e) => patch(e.terminal_id, { exited: true }));
    }
    set((s) => ({ tabs: [...s.tabs, tab], activeId: tab.id }));
    try {
      await spawn(tab);
    } catch (e) {
      patch(tab.id, { exited: true, error: String(e) });
    }
  };

  return {
    tabs: [],
    activeId: CHAT_TAB,
    installed: null,

    setActive: (activeId) => set({ activeId }),

    openTerminal: () => {
      const n = get().tabs.filter((t) => !t.command).length + 1;
      return open({ id: newTabId(), title: `shell ${n}`, exited: false });
    },

    openAgent: (agent) =>
      open({ id: newTabId(), title: agent.name, command: agent.command, agentId: agent.id, exited: false }),

    restart: async (id) => {
      const tab = get().tabs.find((t) => t.id === id);
      if (!tab) return;
      patch(id, { exited: false, error: undefined });
      (await registry()).writeXterm(id, "\r\n\x1b[90m[restarted]\x1b[0m\r\n");
      try {
        await spawn(tab);
      } catch (e) {
        patch(id, { exited: true, error: String(e) });
      }
    },

    close: async (id) => {
      await api.terminalKill(id).catch(() => {});
      (await registry()).disposeXterm(id);
      set((s) => {
        const idx = s.tabs.findIndex((t) => t.id === id);
        const tabs = s.tabs.filter((t) => t.id !== id);
        const activeId = s.activeId !== id ? s.activeId : idx > 0 ? tabs[idx - 1].id : CHAT_TAB;
        return { tabs, activeId };
      });
    },

    detectInstalled: async () => {
      const agents = useApp.getState().settings.agent_clis;
      let found: boolean[] = [];
      try {
        found = await api.detectAgentClis(agents.map((a) => a.command));
      } catch {
        // Unknown: every agent shows as not installed but stays clickable.
      }
      set({ installed: Object.fromEntries(agents.map((a, i) => [a.id, found[i] ?? false])) });
    },

    reset: () => {
      const ids = get().tabs.map((t) => t.id);
      set({ tabs: [], activeId: CHAT_TAB, installed: null });
      for (const id of ids) void api.terminalKill(id).catch(() => {});
      if (ids.length) void registry().then((r) => ids.forEach(r.disposeXterm));
    },
  };
});

// Tabs belong to the workspace; the detection cache follows the agent list.
useApp.subscribe((s, prev) => {
  if (s.workspace?.id !== prev.workspace?.id) useCenterTabs.getState().reset();
  else if (s.settings.agent_clis !== prev.settings.agent_clis) useCenterTabs.setState({ installed: null });
});
```

- [ ] **Step 2: Type-check**

Run: `cd frontend && npx tsc --noEmit`
Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/stores/centerTabs.ts
git commit -m "Add store for terminal and agent tabs in the chat column"
```

---

### Task 7: `CenterColumn` with tab strip, "+" menu and ⌘T

**Files:**
- Create: `frontend/src/features/workspace/CenterColumn.tsx`
- Modify: `frontend/src/features/workspace/ChatLayout.tsx` (imports ~line 3-30, centre block ~line 85-95)

- [ ] **Step 1: Create `CenterColumn.tsx`**

```tsx
/** Chat layout centre column: the built-in Chat tab plus terminal and
 *  agent-CLI tabs opened from the "+" menu. */
import { useEffect, useRef, useState, type ReactNode } from "react";
import { useShallow } from "zustand/react/shallow";
import { Bot, MessageSquare, Plus, RotateCcw, Settings, Terminal as TerminalIcon, X } from "lucide-react";
import { ChatPanel } from "@/features/chat/ChatPanel";
import { useApp } from "@/stores/app";
import { CHAT_TAB, useCenterTabs, type CenterTab } from "@/stores/centerTabs";
import { Button, IconButton } from "@/components/ui";
import { LazyXtermView, PanelSuspense } from "./lazy";

export function CenterColumn({ trailing }: { trailing?: ReactNode }) {
  const { tabs, activeId } = useCenterTabs(useShallow((s) => ({ tabs: s.tabs, activeId: s.activeId })));
  const showChat = activeId === CHAT_TAB;
  useNewTerminalShortcut();

  return (
    <div className="flex h-full min-w-0 flex-col">
      <div className="flex h-9 shrink-0 items-stretch border-b border-neutral-200 pl-1 pr-2 dark:border-neutral-800">
        <div className="flex min-w-0 items-stretch overflow-x-auto [scrollbar-width:none]">
          <TabButton
            active={showChat}
            icon={<MessageSquare size={12} />}
            label="Chat"
            onClick={() => useCenterTabs.getState().setActive(CHAT_TAB)}
          />
          {tabs.map((t) => (
            <TabButton
              key={t.id}
              active={t.id === activeId}
              icon={t.agentId ? <Bot size={12} /> : <TerminalIcon size={12} />}
              label={t.title}
              dim={t.exited}
              onClick={() => useCenterTabs.getState().setActive(t.id)}
              onClose={() => confirmClose(t)}
            />
          ))}
        </div>
        <NewTabMenu />
        <div className="ml-auto flex items-center">{trailing}</div>
      </div>
      <div className="relative min-h-0 flex-1">
        <div className={`absolute inset-0 ${showChat ? "block" : "hidden"}`}>
          <ChatPanel variant="full" />
        </div>
        {tabs.length > 0 && (
          <div className={`absolute inset-0 bg-neutral-950 text-neutral-200 ${showChat ? "hidden" : "block"}`}>
            <PanelSuspense>
              {tabs.map((t) => (
                <TerminalTabBody key={t.id} tab={t} active={!showChat && t.id === activeId} />
              ))}
            </PanelSuspense>
          </div>
        )}
      </div>
    </div>
  );
}

function confirmClose(tab: CenterTab) {
  if (tab.agentId && !tab.exited && !window.confirm(`Stop ${tab.title} and close its tab?`)) return;
  void useCenterTabs.getState().close(tab.id);
}

function TerminalTabBody({ tab, active }: { tab: CenterTab; active: boolean }) {
  return (
    <div className={`absolute inset-0 flex-col ${active ? "flex" : "hidden"}`}>
      {tab.exited && (
        <div className="flex shrink-0 items-center gap-2 border-b border-neutral-800 px-3 py-1 font-mono text-[11px] text-neutral-400">
          <span className={`min-w-0 flex-1 truncate ${tab.error ? "text-red-400" : ""}`}>
            {tab.error ?? "Process exited"}
          </span>
          <Button size="sm" variant="ghost" onClick={() => void useCenterTabs.getState().restart(tab.id)}>
            <RotateCcw size={11} /> Restart
          </Button>
        </div>
      )}
      <div className="relative min-h-0 flex-1">
        <LazyXtermView terminalId={tab.id} active={active} />
      </div>
    </div>
  );
}

function TabButton({
  active,
  icon,
  label,
  dim,
  onClick,
  onClose,
}: {
  active: boolean;
  icon: ReactNode;
  label: string;
  dim?: boolean;
  onClick: () => void;
  onClose?: () => void;
}) {
  return (
    <div
      onClick={onClick}
      className={`group -mb-px flex shrink-0 cursor-pointer items-center gap-1.5 border-b-2 px-2.5 font-mono text-[10.5px] tracking-wide transition-colors ${
        active
          ? "border-accent-500 text-neutral-900 dark:text-neutral-100"
          : "border-transparent text-neutral-500 hover:text-neutral-800 dark:hover:text-neutral-200"
      } ${dim ? "opacity-60" : ""}`}
    >
      {icon}
      <span className="max-w-[140px] truncate">{label}</span>
      {onClose && (
        <button
          title="Close tab"
          onClick={(e) => {
            e.stopPropagation();
            onClose();
          }}
          className="rounded p-0.5 opacity-0 transition-opacity group-hover:opacity-100 hover:bg-neutral-300/60 dark:hover:bg-neutral-700"
        >
          <X size={10} />
        </button>
      )}
    </div>
  );
}

function NewTabMenu() {
  const [open, setOpen] = useState(false);
  const agents = useApp((s) => s.settings.agent_clis);
  const installed = useCenterTabs((s) => s.installed);
  const ref = useRef<HTMLDivElement>(null);
  const enabled = agents.filter((a) => a.enabled && a.command.trim());

  useEffect(() => {
    if (!open) return;
    if (useCenterTabs.getState().installed === null) void useCenterTabs.getState().detectInstalled();
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const run = (action: () => void) => {
    setOpen(false);
    action();
  };

  return (
    <div ref={ref} className="relative flex items-center">
      <IconButton title="New tab" className="ml-0.5 h-6 w-6" onClick={() => setOpen((o) => !o)}>
        <Plus size={13} />
      </IconButton>
      {open && (
        <div className="animate-rise absolute left-0 top-full z-30 mt-1 w-60 rounded-[6px] border border-neutral-200 bg-white p-1 shadow-lg dark:border-neutral-800 dark:bg-neutral-900">
          <MenuItem
            icon={<TerminalIcon size={13} />}
            label="New Terminal"
            hint="⌘T"
            onClick={() => run(() => void useCenterTabs.getState().openTerminal())}
          />
          {enabled.length > 0 && <MenuDivider />}
          {enabled.map((a) => {
            const missing = installed?.[a.id] === false;
            return (
              <MenuItem
                key={a.id}
                icon={<Bot size={13} />}
                label={a.name}
                hint={missing ? "not installed" : installed === null ? "…" : undefined}
                dim={missing}
                onClick={() => run(() => void useCenterTabs.getState().openAgent(a))}
              />
            );
          })}
          <MenuDivider />
          <MenuItem icon={<Settings size={13} />} label="Agent settings…" dim onClick={() => run(openAgentSettings)} />
        </div>
      )}
    </div>
  );
}

function MenuItem({
  icon,
  label,
  hint,
  dim,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  hint?: string;
  dim?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`flex w-full items-center gap-2 rounded-[4px] px-2 py-1.5 text-left text-xs transition-colors hover:bg-neutral-100 dark:hover:bg-neutral-800 ${
        dim ? "text-neutral-500" : "text-neutral-800 dark:text-neutral-200"
      }`}
    >
      <span className="shrink-0 text-neutral-500">{icon}</span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {hint && <span className="shrink-0 font-mono text-[10px] text-neutral-500">{hint}</span>}
    </button>
  );
}

function MenuDivider() {
  return <div className="my-1 h-px bg-neutral-200 dark:bg-neutral-800" />;
}

function openAgentSettings() {
  useApp.getState().setView("settings");
  setTimeout(() => document.getElementById("agent-clis")?.scrollIntoView({ block: "start" }), 50);
}

/** ⌘T (Ctrl+Shift+T elsewhere) opens a terminal tab, except while typing in
 *  a text field. Terminals count as "not typing" (xterm uses a textarea). */
function useNewTerminalShortcut() {
  useEffect(() => {
    const isMac = navigator.platform.toUpperCase().includes("MAC");
    const onKey = (e: KeyboardEvent) => {
      const combo = isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && e.shiftKey;
      if (!combo || e.altKey || e.code !== "KeyT") return;
      const el = e.target as HTMLElement | null;
      if (el?.closest("input, textarea, [contenteditable='true']") && !el.closest(".xterm")) return;
      e.preventDefault();
      void useCenterTabs.getState().openTerminal();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
```

- [ ] **Step 2: Wire it into `ChatLayout.tsx`**

Replace the import `import { ChatPanel } from "@/features/chat/ChatPanel";` with:

```ts
import { CenterColumn } from "./CenterColumn";
```

Replace the centre block:

```tsx
      <div className="relative min-w-0 flex-1">
        <ChatPanel variant="full" />
        <div className="absolute right-2 top-1.5 z-10">
          <IconButton
            title={contextOpen ? "Hide context panel" : "Show context panel"}
            onClick={() => setContextOpen(!contextOpen)}
          >
            {contextOpen ? <PanelRightClose size={14} /> : <PanelRightOpen size={14} />}
          </IconButton>
        </div>
      </div>
```

with:

```tsx
      <div className="relative min-w-0 flex-1">
        <CenterColumn
          trailing={
            <IconButton
              title={contextOpen ? "Hide context panel" : "Show context panel"}
              onClick={() => setContextOpen(!contextOpen)}
            >
              {contextOpen ? <PanelRightClose size={14} /> : <PanelRightOpen size={14} />}
            </IconButton>
          }
        />
      </div>
```

Update the file's top doc comment to:

```ts
/** Chat-first layout: sessions sidebar | centre column (Chat plus terminal
 *  and agent-CLI tabs) | collapsible context panel (changed files, explorer,
 *  editor, terminal, git). */
```

- [ ] **Step 3: Build**

Run: `cd frontend && npm run build`
Expected: success (unused-import errors would fail `tsc`; fix any that appear).

- [ ] **Step 4: Commit**

```bash
git add frontend/src/features/workspace/CenterColumn.tsx frontend/src/features/workspace/ChatLayout.tsx
git commit -m "Add terminal and agent CLI tabs to the chat column"
```

---

### Task 8: Agent CLI settings section

**Files:**
- Create: `frontend/src/features/settings/AgentCliSettings.tsx`
- Modify: `frontend/src/features/settings/SettingsPage.tsx` (lucide import block, new section after Appearance ~line 213)

- [ ] **Step 1: Create `AgentCliSettings.tsx`**

```tsx
/** Edit the agent CLIs shown in the chat column's "+" menu. Text fields save
 *  on blur; toggles, removals and additions save immediately. */
import { useEffect, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import type { AgentCli } from "@/lib/ipc";
import { useApp } from "@/stores/app";
import { Button, IconButton, Toggle, inputClass } from "@/components/ui";

export function AgentCliSettings() {
  const agents = useApp((s) => s.settings.agent_clis);
  const [draft, setDraft] = useState<AgentCli[]>(agents);
  useEffect(() => setDraft(agents), [agents]);

  const save = (next: AgentCli[]) => void useApp.getState().updateSettings({ agent_clis: next });
  const edit = (i: number, p: Partial<AgentCli>) =>
    setDraft((d) => d.map((a, j) => (j === i ? { ...a, ...p } : a)));

  return (
    <div className="space-y-1.5">
      {draft.map((a, i) => (
        <div key={a.id} className="flex items-center gap-2">
          <Toggle
            checked={a.enabled}
            onChange={(enabled) => save(draft.map((x, j) => (j === i ? { ...x, enabled } : x)))}
          />
          <div className="w-36 shrink-0">
            <input
              className={inputClass}
              value={a.name}
              placeholder="Name"
              onChange={(e) => edit(i, { name: e.target.value })}
              onBlur={() => save(draft)}
            />
          </div>
          <div className="min-w-0 flex-1">
            <input
              className={`${inputClass} font-mono`}
              value={a.command}
              placeholder="command --flags"
              onChange={(e) => edit(i, { command: e.target.value })}
              onBlur={() => save(draft)}
            />
          </div>
          <IconButton title="Remove" onClick={() => save(draft.filter((_, j) => j !== i))}>
            <Trash2 size={12} />
          </IconButton>
        </div>
      ))}
      <Button
        size="sm"
        onClick={() =>
          save([...draft, { id: `custom-${Date.now().toString(36)}`, name: "New agent", command: "", enabled: true }])
        }
      >
        <Plus size={12} /> Add agent
      </Button>
    </div>
  );
}
```

- [ ] **Step 2: Add the section to `SettingsPage.tsx`**

Add `Bot` to the existing `lucide-react` import list and add:

```ts
import { AgentCliSettings } from "./AgentCliSettings";
```

Insert after the closing `</section>` of the Appearance section (just before `{/* Permissions */}`):

```tsx
          {/* Agent CLIs */}
          <section id="agent-clis">
            <h2 className="mb-1 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-neutral-500">
              <Bot size={13} /> Agent CLIs
            </h2>
            <p className="mb-3 text-[11px] text-neutral-500">
              Shown in the + menu of the chat column. Each command runs in a terminal tab through your login
              shell, so it uses your PATH and the CLI's own login.
            </p>
            <AgentCliSettings />
          </section>
```

- [ ] **Step 3: Build**

Run: `cd frontend && npm run build`
Expected: success.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/features/settings
git commit -m "Add agent CLI settings"
```

---

### Task 9: Docs and full verification

**Files:**
- Modify: `CLAUDE.md` (after the "Browser preview" paragraph)

- [ ] **Step 1: Document in `CLAUDE.md`**

Insert after the **Browser preview** paragraph:

```markdown
**Centre tabs (Chat layout)**: `features/workspace/CenterColumn.tsx` puts a tab strip over the chat column: the permanent Chat tab plus terminal and agent-CLI tabs (state in `stores/centerTabs.ts`). Agent tabs call `terminal_create` with a `command`, which `terminal.rs::build_command` runs as `$SHELL -l -i -c <command>`; presets live in `AppSettings.agent_clis` (user-editable, never model output), and `agent_detect.rs` checks the login-shell PATH for the "+" menu. xterm instances live in `features/terminal/xtermRegistry.ts`, outside React, so tabs keep their scrollback across page/layout switches; centre tabs are killed only when the workspace changes or closes. This is unrelated to `crates/agent-cli` (one-shot non-interactive runs).
```

- [ ] **Step 2: Run all checks**

Run: `cargo test --workspace`
Expected: all PASS.

Run: `cd frontend && npm run build`
Expected: success.

- [ ] **Step 3: Manual check in the app**

Run: `npx --yes @tauri-apps/cli@2 dev`

Verify:
1. Chat layout shows the tab strip with **Chat** and **+**; context-panel toggle is on the right.
2. **+** lists New Terminal, the six presets (missing ones dimmed with `not installed`), Agent settings….
3. New Terminal opens a shell in the workspace root; the first prompt is visible (no lost early output).
4. An installed agent (e.g. Claude) starts in its tab.
5. Switch chat sessions and back: terminal tabs still running.
6. Open Agent settings… then return to the workspace: tabs and scrollback still there.
7. Exit an agent: tab dims, "Process exited" bar with Restart; Restart relaunches it.
8. ⌘T opens a terminal from inside a terminal and from the chat area, but not while typing in the chat input.
9. Switch workspace: tabs are gone and `terminal_list` no longer contains their ids.
10. Context panel Terminal tab still works.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "Document chat column terminal and agent tabs"
```
