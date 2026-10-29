# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

JenderalCode Desktop: a Tauri 2 desktop AI coding agent (Rust backend + React/TypeScript frontend). The spec is `JenderalCode-Desktop-PRD.md`. Code comments and docs reference PRD sections (`FR-xx`, `§n`). `README.md` and `docs/ARCHITECTURE.md` are written in Indonesian; `docs/ARCHITECTURE.md` maps the implementation to the PRD and is the best single overview.

## Commands

```bash
# Rust (Cargo workspace at repo root)
cargo test --workspace                       # all unit + integration tests
cargo test -p jcode-permissions              # one crate (crate names are jcode-<dir>)
cargo test -p jcode-storage migrations_are_idempotent   # one test
cargo test -p jcode-e2e-tests                # cross-crate e2e (tests/e2e/tests/e2e.rs)

# Frontend (frontend/)
cd frontend && npm install
npm run build        # tsc --noEmit + vite build; this is the frontend's only check (no test script)

# App (run from repo root or apps/desktop)
npx --yes @tauri-apps/cli@2 dev              # starts vite on :5173 + Tauri
npx --yes @tauri-apps/cli@2 build            # release bundle -> target/release/bundle/
npx --yes @tauri-apps/cli@2 build --bundles app   # skip DMG if Finder AppleEvent times out
```

`tauri.conf.json` locates `frontend/` by walking up from the cwd, so `tauri dev/build` works from the root or `apps/desktop`.

## Architecture

Workspace layout: `apps/desktop` (Tauri shell), `crates/*` (libraries, all named `jcode-*`), `frontend/` (React 19 + Vite + Tailwind 4 + zustand), `tests/e2e`.

**Agent turn flow** (the core loop, spread over several crates):
1. Frontend calls `invoke("send_message")` → `apps/desktop/src/commands.rs`.
2. `AgentRuntime::send_message` (`crates/agent-runtime/src/lib.rs`) spawns a tokio task per turn. `context.rs` builds the system prompt from the profile, file tree, git status, and project instruction files (marked UNTRUSTED).
3. `create_provider` dispatches on `ProviderKind` to an `AiProvider` impl (`provider-openai` / `-anthropic` / `-google` / `provider-core::mock`). `stream()` yields `StreamEvent`s (TextDelta, ToolCall*, Usage, Done).
4. For each tool call, `agent-runtime/src/executor.rs`: `classify` → `permissions::Action` → `PolicyEngine::check` (Allowed / Denied / NeedsApproval). NeedsApproval persists an approval, emits `chat://approval`, and waits on a oneshot resolved by the `decide_approval` command. Then `execute` runs the tool, snapshotting files to a checkpoint (`tool-filesystem::CheckpointStore`) before any write or delete.
5. Loop until no tool calls, the iteration limit, or cancellation.

Runtime events go through the `EventSink` trait (agent-runtime). `apps/desktop/src/sink.rs` (`TauriEventSink`) turns them into Tauri events: `chat://*`, `terminal://*`, `fs://changed`. Tests use `NoopSink` or custom sinks with the mock provider, so you can exercise the full approval flow without network.

**Cross-cutting conventions**
- Shared types (`ChatMessage`, `ToolSpec`, `StreamEvent`, provider/model configs, events) live in `crates/common`. `frontend/src/lib/ipc.ts` mirrors them by hand, plus every command signature. Changing a serialized Rust type or command means updating `ipc.ts` too.
- New Tauri command: implement it in `apps/desktop/src/commands.rs`, register it in `generate_handler![...]` in `apps/desktop/src/main.rs`, and add a typed wrapper in `ipc.ts`.
- New agent tool: add its spec in `executor::tool_specs`, its permission mapping in `executor::classify`, and its execution in `executor::execute`. Agent modes (Plan/Build/Review) are enforced twice: tools are filtered out of the specs sent to the model, and they are rejected again at execution time. Keep both in sync.
- Storage: a single SQLite connection behind a mutex (`crates/storage`). Migrations are SQL files in `crates/storage/migrations/` registered in the `MIGRATIONS` list via `include_str!` and tracked in a `_migrations` table. Add new numbered files; never edit applied ones.
- Provider adapters must report unknown model capabilities as `None` rather than guessing. All OpenAI-compatible vendors (OpenAI, DeepSeek, Mistral, xAI, Groq, Ollama, LM Studio) share `provider-openai`; URL presets live in `provider-core::provider_presets()`.
- API keys never go in the DB, only a credential reference. `provider-core/src/credentials.rs` supports a 0600 file backend (default) and the OS keychain, with migration between them.

**Security choke points** (keep changes routed through these):
- Path resolution: `tool-filesystem` `Workspace::resolve` (canonicalize + containment, rejects symlink escape).
- `permissions`: `is_sensitive_path`; `assess_command` tokenizes shell commands (splits on `; | & && ||`, follows `$(…)`, backticks, `bash -c`, skips wrappers and git global options) and feeds `is_catastrophic_command` (absolute deny) and `is_git_remote_command` (always asks). Balanced asks for every command; Yolo still asks for git remote, `sudo`, download-and-execute, and unparseable commands. Add new rules there with a test, not via substring checks.
- `tool-filesystem`: `write_no_follow` (O_NOFOLLOW) for all writes; `search`/`glob_files` skip symlinks and `is_sensitive_name` files. Read-only git calls run with `core.fsmonitor=false` and `core.hooksPath` disabled.
- Agent command execution: `tool-terminal::run_exec` (timeout, output cap, process-group kill). This is separate from the interactive human PTY in `apps/desktop/src/terminal.rs` (portable-pty → xterm.js).

`apps/desktop/src/watcher.rs` watches the workspace with `notify` (trailing 300 ms debounce, ignores `.git`/`target`/`node_modules`/build dirs) and emits `fs://changed`.

**Desktop shell rules**: Tauri commands doing IO or spawning processes are `async` and run their body in `spawn_blocking` (sync commands run on the UI thread). The webview has a strict CSP (`tauri.conf.json`) and a navigation lock in `main.rs`, so model output can never load remote resources or navigate away; the chat markdown renderer also blocks images and link navigation. `TauriEventSink` coalesces `chat://delta` events (~30 ms) and flushes before any other event for the session.

**Frontend layouts**: `useApp().layout` is `"chat"` (default, `ChatLayout`: sessions sidebar, full-width `ChatPanel variant="full"`, context panel with Changes/Files/Editor/Terminal/Git) or `"code"` (IDE layout in `WorkspacePage`, `ChatPanel variant="side"`), persisted in localStorage. Heavy panels (CodeMirror, xterm, git, secondary pages) are `React.lazy` loaded and kept mounted once visited so terminal PTYs survive tab switches. Subscribe to zustand stores with selectors (`useShallow`); streaming text lives in `StreamingBubble` only.

Runtime data lives in the OS app-data dir under `com.jenderalcode.desktop/` (`jenderalcode.db`, `checkpoints/`, `credentials.json`).
