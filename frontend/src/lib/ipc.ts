/**
 * Typed IPC layer over Tauri invoke/listen.
 * Types mirror `crates/common` and command signatures in `apps/desktop`.
 */
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ---------------------------------------------------------------------------
// Shared types (mirror jcode-common)
// ---------------------------------------------------------------------------

export type ProviderKind = "openai_compatible" | "anthropic" | "google" | "mock" | "external_cli";

export interface ProviderConfig {
  id: string;
  name: string;
  kind: ProviderKind;
  base_url: string;
  enabled: boolean;
  created_at: number;
  updated_at: number;
  extra?: unknown;
  credential_ref?: string | null;
}

export interface ModelCapabilities {
  streaming?: boolean | null;
  tool_calling?: boolean | null;
  vision?: boolean | null;
  structured_output?: boolean | null;
  context_window?: number | null;
  max_output_tokens?: number | null;
}

export interface ModelInfo {
  id: string;
  provider_id: string;
  name: string;
  display_name: string;
  capabilities: ModelCapabilities;
  input_price_per_mtok?: number | null;
  output_price_per_mtok?: number | null;
  custom: boolean;
}

export interface WorkspaceInfo {
  id: string;
  name: string;
  root_path: string;
  last_opened_at: number;
  is_git_repo: boolean;
}

export type AgentMode = "plan" | "build" | "review" | "custom";
export type PolicyMode = "safe" | "balanced" | "yolo";

export interface AgentProfile {
  id: string;
  name: string;
  mode: AgentMode;
  system_prompt: string;
  allowed_tools?: string[] | null;
  policy_mode: PolicyMode;
  provider_id?: string | null;
  model_id?: string | null;
  built_in: boolean;
  created_at: number;
}

export interface SessionSummary {
  id: string;
  workspace_id: string;
  title: string;
  agent_profile_id?: string | null;
  provider_id?: string | null;
  model_name?: string | null;
  /** Per-session permission override ("safe" | "balanced" | "yolo"); null = profile default. */
  policy_mode?: PolicyMode | null;
  created_at: number;
  updated_at: number;
  message_count: number;
}

export type Role = "system" | "user" | "assistant" | "tool";

export interface ToolCall {
  id: string;
  name: string;
  arguments: string;
}

/** An image in a user message (base64 without a `data:` prefix). */
export interface ImageAttachment {
  mime: string;
  data: string;
}

export interface MessageRecord {
  id: string;
  session_id: string;
  role: Role;
  content: string;
  images?: ImageAttachment[];
  tool_calls: ToolCall[];
  tool_call_id?: string | null;
  tool_name?: string | null;
  /** Model reasoning ("thinking") streamed separately from the answer. */
  reasoning?: string | null;
  created_at: number;
}

export type ToolRunStatus =
  | "pending"
  | "awaiting_approval"
  | "running"
  | "success"
  | "denied"
  | "error"
  | "cancelled";

export interface ToolRunRecord {
  id: string;
  session_id: string;
  message_id?: string | null;
  tool_name: string;
  status: ToolRunStatus;
  args: Record<string, unknown>;
  result_summary?: string | null;
  error?: string | null;
  approval_id?: string | null;
  created_at: number;
  finished_at?: number | null;
}

export interface ApprovalRecord {
  id: string;
  session_id: string;
  tool_run_id: string;
  kind: string;
  description: string;
  preview?: string | null;
  status: "pending" | "approved" | "denied";
  created_at: number;
  decided_at?: number | null;
}

export type AgentUsageStatus = "ok" | "not_installed" | "not_signed_in" | "expired" | "unavailable" | "error";

/** One usage window (Rust `UsageWindow`). */
export interface UsageWindow {
  label: string;
  /** 0-100; null for activity without a limit. */
  used_percent: number | null;
  /** Unix ms. */
  resets_at: number | null;
  detail: string | null;
}

/** Subscription usage of one external agent CLI (Rust `AgentUsageReport`). */
export interface AgentUsageReport {
  id: string;
  name: string;
  status: AgentUsageStatus;
  plan: string | null;
  account: string | null;
  windows: UsageWindow[];
  note: string | null;
  /** Unix ms when the data was recorded, if not live. */
  updated_at: number | null;
}

/** Terminal agent CLI in the chat column's "+" menu (Rust `AgentCli`). */
export interface AgentCli {
  id: string;
  name: string;
  command: string;
  enabled: boolean;
  /** Flags appended when opened "with bypass permissions". */
  bypass_args?: string | null;
  /** Arguments that continue the last conversation (restored tabs). */
  resume_args?: string | null;
}

export interface AppSettings {
  theme: string;
  default_policy_mode: PolicyMode;
  editor_font_size: number;
  terminal_font_size: number;
  max_iterations: number;
  retention_days?: number | null;
  show_hidden_files: boolean;
  /** "file" (default, no OS password prompts) or "keychain". */
  credential_storage: string;
  /** Wrap long editor lines instead of horizontal scrolling. */
  editor_word_wrap: boolean;
  /** Agent CLIs for the chat column's "+" menu. */
  agent_clis: AgentCli[];
  /** Report language-server errors to the agent after edits. */
  lsp_diagnostics: boolean;
  /** Download missing language servers with npx. */
  lsp_auto_download: boolean;
  /** Skill names hidden from the agent. */
  disabled_skills: string[];
  /** MCP server names the agent does not use. */
  disabled_mcp_servers: string[];
  /** Projects whose `.mcp.json` the user allowed to run. */
  mcp_trusted_projects: { root: string; fingerprint: string }[];
  /** Remote control from a paired phone (docs/REMOTE_PROTOCOL.md). */
  remote_enabled: boolean;
  /** Paired phones may use full access, terminals and file/git writes. */
  remote_allow_full_access: boolean;
}

/** A phone paired for remote control (Rust `remote::RemoteDevice`). */
export interface RemoteDevice {
  device_id: string;
  name: string;
  user_email: string;
  created_at: number;
  last_seen?: number | null;
  online: boolean;
}

export type RemoteState = "off" | "no_relay" | "unpaired" | "connecting" | "online" | "offline" | "stopped";

export interface RemoteStatus {
  enabled: boolean;
  relay_url: string;
  state: RemoteState;
  error?: string | null;
  desktop_name: string;
  devices: RemoteDevice[];
  allow_full_access: boolean;
}

export interface RemotePairingStarted {
  pairing_id: string;
  /** SVG markup of the QR code. */
  qr_svg: string;
  expires_at: number;
}

export interface RemotePairingEvent {
  pairing_id: string;
  state: "approved" | "expired" | "error";
  /** 6-digit code to compare with the phone. */
  sas?: string | null;
  mobile_name?: string | null;
  user_email?: string | null;
  error?: string | null;
}

/** An MCP server config (Rust `jcode_tool_mcp::ServerConfig`). */
export interface McpServerConfig {
  name: string;
  transport: "stdio" | "http" | "sse";
  command: string;
  args: string[];
  env: Record<string, string>;
  cwd?: string | null;
  url: string;
  headers: Record<string, string>;
}

/** A configured server's runtime state (Rust `ServerStatus`). */
export interface McpServerStatus {
  name: string;
  scope: "global" | "project";
  summary: string;
  state: "starting" | "ready" | "failed";
  error?: string | null;
  server_name?: string | null;
  tools: { name: string; tool: string; description: string }[];
}

export interface McpOverview {
  global_path: string | null;
  global: McpServerConfig[];
  global_error: string | null;
  project: { path: string; servers: McpServerConfig[]; error: string | null; trusted: boolean } | null;
  disabled: string[];
  status: McpServerStatus[];
}

/** An installed skill (Rust `SkillInfo`). */
export interface SkillInfo {
  name: string;
  description: string;
  path: string;
  project: boolean;
  enabled: boolean;
  /** Installed into ~/.agents/skills by `npx skills` (removable here). */
  removable: boolean;
}

/** A skills.sh search hit. */
export interface SkillSearchResult {
  source: string;
  skill: string;
  installs: number;
}

export interface ProviderTestResult {
  ok: boolean;
  latency_ms: number;
  model_count?: number | null;
  error?: string | null;
}

export interface FileEntry {
  path: string;
  name: string;
  is_dir: boolean;
  size?: number | null;
}

export interface SearchHit {
  path: string;
  line_number?: number | null;
  line: string;
}

export interface FileStatus {
  path: string;
  index: string;
  worktree: string;
}

export interface BranchInfo {
  name: string;
  current: boolean;
}

export interface CommitInfo {
  hash: string;
  short_hash: string;
  subject: string;
  author: string;
  timestamp: number;
}

export interface ProviderUsage {
  provider_id: string;
  input_tokens: number;
  output_tokens: number;
  cost_estimate?: number | null;
}

export interface DayUsage {
  day: string;
  input_tokens: number;
  output_tokens: number;
  cost_estimate?: number | null;
}

export interface UsageSummary {
  total_input: number;
  total_output: number;
  total_cost?: number | null;
  sessions: number;
  by_provider: ProviderUsage[];
  by_day: DayUsage[];
}

/** Per-session token usage; `last_input` ≈ how full the context is now. */
export interface SessionUsage {
  total_input: number;
  total_output: number;
  total_cost?: number | null;
  requests: number;
  last_input: number;
  last_output: number;
}

export interface BrowserBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface BrowserStateEvent {
  id: string;
  url: string;
  loading: boolean;
}

export interface SessionDetail {
  session: SessionSummary;
  messages: MessageRecord[];
  tool_runs: ToolRunRecord[];
  approvals: ApprovalRecord[];
}

export interface CheckpointEntry {
  id: string;
  session_id: string;
  rel_path: string;
  stored_path: string;
  existed: boolean;
  created_at: number;
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

export type ChatStatus = "idle" | "thinking" | "streaming" | "running_tool" | "waiting_approval";

export interface ChatStatusEvent {
  session_id: string;
  status: ChatStatus;
  detail?: string | null;
}

export interface ChatDeltaEvent {
  session_id: string;
  message_id: string;
  delta: string;
  /** The delta is model reasoning, not answer text. */
  reasoning?: boolean;
}

export interface MessageEvent {
  session_id: string;
  message: MessageRecord;
}

export interface ToolRunEvent {
  session_id: string;
  tool_run: ToolRunRecord;
}

export interface ApprovalEvent {
  session_id: string;
  approval: ApprovalRecord;
}

export interface UsageEvent {
  session_id: string;
  usage: { input_tokens: number; output_tokens: number };
  cost_estimate?: number | null;
}

export interface ChatErrorEvent {
  session_id: string;
  error: string;
}

export interface TerminalDataEvent {
  terminal_id: string;
  data: string;
  /** Output bytes so far, including this chunk (see `terminalSnapshot`). */
  seq: number;
}

/** Recent output of a running terminal, its end `seq` and PTY size. */
export interface TerminalSnapshot {
  data: string;
  seq: number;
  cols: number;
  rows: number;
}

/** A paired phone opened a terminal (a shell or an agent CLI). */
export interface RemoteTerminalEvent {
  terminal_id: string;
  workspace_id: string;
  title: string;
  command: string | null;
  agent_id: string | null;
  bypass: boolean;
}

export interface TerminalExitEvent {
  terminal_id: string;
  code: number | null;
}

export interface FsChangedEvent {
  root: string;
  paths: string[];
}

// ---------------------------------------------------------------------------
// Invoke wrappers
// ---------------------------------------------------------------------------

const invoke = <T>(cmd: string, args?: Record<string, unknown>): Promise<T> =>
  tauriInvoke<T>(cmd, args).catch((e: unknown) => {
    throw new Error(typeof e === "string" ? e : (e as Error).message ?? String(e));
  });

export const api = {
  // Workspaces
  listWorkspaces: () => invoke<WorkspaceInfo[]>("list_workspaces"),
  openWorkspace: (path: string) => invoke<WorkspaceInfo>("open_workspace", { path }),
  getActiveWorkspace: () => invoke<WorkspaceInfo | null>("get_active_workspace"),
  closeWorkspace: () => invoke<void>("close_workspace"),
  removeWorkspace: (id: string) => invoke<void>("remove_workspace", { id }),

  // FS
  fsList: (path?: string, showHidden?: boolean) =>
    invoke<FileEntry[]>("fs_list", { path: path ?? null, showHidden: showHidden ?? null }),
  /** Project files matching `query`, best first (for `@` mentions). */
  fsFindFiles: (query: string) => invoke<string[]>("fs_find_files", { query }),
  /** Sort files picked in the attach dialog: project files (relative paths),
   *  images from anywhere, and the rest (refused). */
  resolvePickedAttachments: (paths: string[]) =>
    invoke<{ relative: string[]; images: ImageAttachment[]; rejected: string[] }>("resolve_picked_attachments", {
      paths,
    }),
  fsRead: (path: string) => invoke<{ path: string; content: string }>("fs_read", { path }),
  fsWrite: (path: string, content: string) => invoke<void>("fs_write", { path, content }),
  fsCreate: (path: string, isDir: boolean) => invoke<void>("fs_create", { path, isDir }),
  fsDelete: (path: string) => invoke<void>("fs_delete", { path }),
  fsRename: (from: string, to: string) => invoke<void>("fs_rename", { from, to }),
  fsSearch: (query: string, isRegex?: boolean, includeGlob?: string) =>
    invoke<SearchHit[]>("fs_search", {
      query,
      isRegex: isRegex ?? null,
      includeGlob: includeGlob ?? null,
    }),

  // Providers
  listProviders: () => invoke<ProviderConfig[]>("list_providers"),
  saveProvider: (provider: ProviderConfig, apiKey?: string) =>
    invoke<ProviderConfig>("save_provider", { provider, apiKey: apiKey ?? null }),
  deleteProvider: (id: string) => invoke<void>("delete_provider", { id }),
  testProvider: (id: string) => invoke<ProviderTestResult>("test_provider", { id }),
  refreshModels: (providerId: string) => invoke<ModelInfo[]>("refresh_models", { providerId }),
  listModels: (providerId?: string) => invoke<ModelInfo[]>("list_models", { providerId: providerId ?? null }),
  addCustomModel: (
    providerId: string,
    name: string,
    displayName?: string,
    inputPrice?: number,
    outputPrice?: number,
  ) =>
    invoke<ModelInfo>("add_custom_model", {
      providerId,
      name,
      displayName: displayName ?? null,
      inputPricePerMtok: inputPrice ?? null,
      outputPricePerMtok: outputPrice ?? null,
    }),
  deleteModel: (id: string) => invoke<void>("delete_model", { id }),
  providerPresets: () =>
    invoke<{ id: string; label: string; kind: ProviderKind; base_url: string; needs_key: boolean }[]>(
      "provider_presets",
    ),

  // Agent profiles
  listAgentProfiles: () => invoke<AgentProfile[]>("list_agent_profiles"),
  saveAgentProfile: (profile: AgentProfile) => invoke<AgentProfile>("save_agent_profile", { profile }),
  deleteAgentProfile: (id: string) => invoke<void>("delete_agent_profile", { id }),

  // Sessions & chat
  listSessions: (workspaceId?: string) =>
    invoke<SessionSummary[]>("list_sessions", { workspaceId: workspaceId ?? null }),
  searchSessions: (query: string) => invoke<SessionSummary[]>("search_sessions", { query }),
  createSession: (
    workspaceId: string,
    agentProfileId?: string,
    providerId?: string,
    modelName?: string,
  ) =>
    invoke<SessionSummary>("create_session", {
      workspaceId,
      agentProfileId: agentProfileId ?? null,
      providerId: providerId ?? null,
      modelName: modelName ?? null,
    }),
  getSessionDetail: (sessionId: string) => invoke<SessionDetail>("get_session_detail", { sessionId }),
  sendMessage: (sessionId: string, content: string, attachments: string[], images: ImageAttachment[] = []) =>
    invoke<void>("send_message", { sessionId, content, attachments, images }),
  /** Add a message to the running turn; `null` when no turn is running. */
  steerMessage: (sessionId: string, content: string) => invoke<string | null>("steer_message", { sessionId, content }),
  /** The draft rewritten by the session's model ("Enhance prompt"). */
  enhancePrompt: (sessionId: string, text: string) => invoke<string>("enhance_prompt", { sessionId, text }),
  cancelGeneration: (sessionId: string) => invoke<void>("cancel_generation", { sessionId }),
  decideApproval: (approvalId: string, approved: boolean, grant?: string) =>
    invoke<void>("decide_approval", { approvalId, approved, grant: grant ?? null }),
  setSessionModel: (sessionId: string, providerId?: string, modelName?: string) =>
    invoke<void>("set_session_model", {
      sessionId,
      providerId: providerId ?? null,
      modelName: modelName ?? null,
    }),
  setSessionAgent: (sessionId: string, agentProfileId?: string) =>
    invoke<void>("set_session_agent", {
      sessionId,
      agentProfileId: agentProfileId ?? null,
    }),
  setPolicyMode: (sessionId: string, mode: string) =>
    invoke<void>("set_policy_mode", { sessionId, mode }),
  deleteSession: (sessionId: string) => invoke<void>("delete_session", { sessionId }),
  renameSession: (sessionId: string, title: string) =>
    invoke<void>("rename_session", { sessionId, title }),
  sessionIsBusy: (sessionId: string) => invoke<boolean>("session_is_busy", { sessionId }),
  listCheckpoints: (sessionId: string) => invoke<CheckpointEntry[]>("list_checkpoints", { sessionId }),
  restoreCheckpoints: (sessionId: string) => invoke<number>("restore_checkpoints", { sessionId }),

  // Git
  gitStatus: () => invoke<FileStatus[]>("git_status"),
  gitDiff: (path?: string, staged?: boolean) =>
    invoke<string>("git_diff", { path: path ?? null, staged: staged ?? null }),
  gitDiffAll: (path?: string) => invoke<string>("git_diff_all", { path: path ?? null }),
  gitStage: (paths: string[]) => invoke<void>("git_stage", { paths }),
  gitUnstage: (paths: string[]) => invoke<void>("git_unstage", { paths }),
  gitDiscard: (path: string) => invoke<void>("git_discard", { path }),
  gitCommit: (message: string) => invoke<string>("git_commit", { message }),
  gitBranches: () => invoke<BranchInfo[]>("git_branches"),
  gitCheckoutBranch: (name: string) => invoke<void>("git_checkout_branch", { name }),
  gitCreateBranch: (name: string) => invoke<void>("git_create_branch", { name }),
  gitCurrentBranch: () => invoke<string | null>("git_current_branch"),
  gitLog: (limit?: number) => invoke<CommitInfo[]>("git_log", { limit: limit ?? null }),

  // Usage & settings
  getCredentialBackend: () => invoke<string>("get_credential_backend"),
  switchCredentialStorage: (target: string) =>
    invoke<number>("switch_credential_storage", { target }),
  getDataDir: () => invoke<string>("get_data_dir"),
  setZoom: (factor: number) => invoke<void>("set_zoom", { factor }),
  usageSummary: () => invoke<UsageSummary>("usage_summary"),
  getSessionUsage: (sessionId: string) => invoke<SessionUsage>("get_session_usage", { sessionId }),
  getSettings: () => invoke<AppSettings>("get_settings"),
  saveSettings: (settings: AppSettings) => invoke<void>("save_settings", { settings }),

  // Remote control
  remoteStatus: () => invoke<RemoteStatus>("remote_status"),
  /** Download page of the Android app, with a QR code to scan. */
  remoteMobileDownload: () =>
    invoke<{ url: string; apk_url: string; qr_svg: string }>("remote_mobile_download"),
  /** Open that page in the system browser (fixed URL). */
  remoteOpenMobileDownload: () => invoke<void>("remote_open_mobile_download"),
  remoteApplySettings: () => invoke<RemoteStatus>("remote_apply_settings"),
  remoteStartPairing: () => invoke<RemotePairingStarted>("remote_start_pairing"),
  remoteConfirmPairing: (pairingId: string, accept: boolean) =>
    invoke<RemoteStatus>("remote_confirm_pairing", { pairingId, accept }),
  remoteCancelPairing: () => invoke<void>("remote_cancel_pairing"),
  remoteRevokeDevice: (deviceId: string) => invoke<RemoteStatus>("remote_revoke_device", { deviceId }),
  remoteForgetDesktop: () => invoke<RemoteStatus>("remote_forget_desktop"),
  applyRetention: (days: number) => invoke<number>("apply_retention", { days }),
  clearHistory: () => invoke<number>("clear_history"),
  exportHistory: (path: string) => invoke<string>("export_history", { path }),

  // Browser preview (native child webview)
  browserOpen: (id: string, url: string, bounds: BrowserBounds) =>
    invoke<string>("browser_open", { id, url, bounds }),
  browserSetBounds: (id: string, bounds: BrowserBounds) => invoke<void>("browser_set_bounds", { id, bounds }),
  browserSetVisible: (id: string, visible: boolean) => invoke<void>("browser_set_visible", { id, visible }),
  browserNavigate: (id: string, url: string) => invoke<string>("browser_navigate", { id, url }),
  browserAction: (id: string, action: "back" | "forward" | "reload" | "stop") =>
    invoke<void>("browser_action", { id, action }),
  browserClose: (id: string) => invoke<void>("browser_close", { id }),

  // Terminals
  terminalSnapshot: (id: string) => invoke<TerminalSnapshot>("terminal_snapshot", { id }),
  /** `title` is the tab label, shown to paired phones. */
  terminalCreate: (id: string, cols?: number, rows?: number, command?: string, title?: string, bypass?: boolean) =>
    invoke<void>("terminal_create", {
      id,
      cols: cols ?? null,
      rows: rows ?? null,
      command: command ?? null,
      title: title ?? null,
      bypass: bypass ?? null,
    }),
  terminalWrite: (id: string, data: string) => invoke<void>("terminal_write", { id, data }),
  terminalResize: (id: string, cols: number, rows: number) =>
    invoke<void>("terminal_resize", { id, cols, rows }),
  terminalKill: (id: string) => invoke<void>("terminal_kill", { id }),
  terminalList: () => invoke<string[]>("terminal_list"),
  detectAgentClis: (commands: string[]) => invoke<boolean[]>("detect_agent_clis", { commands }),
  agentUsage: () => invoke<AgentUsageReport[]>("agent_usage"),
  listSkills: () => invoke<SkillInfo[]>("list_skills"),
  searchSkills: (query: string) => invoke<SkillSearchResult[]>("search_skills", { query }),
  installSkill: (source: string, skill: string) => invoke<string>("install_skill", { source, skill }),
  removeSkill: (name: string) => invoke<string>("remove_skill", { name }),
  mcpOverview: () => invoke<McpOverview>("mcp_overview"),
  /** Start/stop the open project's MCP servers to match the config. */
  mcpSync: () => invoke<McpServerStatus[]>("mcp_sync"),
  mcpRestart: (name: string) => invoke<McpServerStatus[]>("mcp_restart", { name }),
  mcpSaveGlobal: (servers: McpServerConfig[]) => invoke<void>("mcp_save_global", { servers }),
  mcpTrustProject: (trusted: boolean) => invoke<void>("mcp_trust_project", { trusted }),

  // External agents (experimental)
  cliAgentPresets: () =>
    invoke<{ id: string; name: string; command: string }[]>("cli_agent_presets"),
  runExternalAgent: (presetId: string, prompt: string) =>
    invoke<{ output: string; success: boolean; duration_ms: number }>("run_external_agent", {
      presetId,
      prompt,
    }),
};

// ---------------------------------------------------------------------------
// Listen helpers
// ---------------------------------------------------------------------------

export const events = {
  chatStatus: (cb: (e: ChatStatusEvent) => void): Promise<UnlistenFn> =>
    listen<ChatStatusEvent>("chat://status", (ev) => cb(ev.payload)),
  chatDelta: (cb: (e: ChatDeltaEvent) => void): Promise<UnlistenFn> =>
    listen<ChatDeltaEvent>("chat://delta", (ev) => cb(ev.payload)),
  chatMessage: (cb: (e: MessageEvent) => void): Promise<UnlistenFn> =>
    listen<MessageEvent>("chat://message", (ev) => cb(ev.payload)),
  chatTool: (cb: (e: ToolRunEvent) => void): Promise<UnlistenFn> =>
    listen<ToolRunEvent>("chat://tool", (ev) => cb(ev.payload)),
  chatApproval: (cb: (e: ApprovalEvent) => void): Promise<UnlistenFn> =>
    listen<ApprovalEvent>("chat://approval", (ev) => cb(ev.payload)),
  chatUsage: (cb: (e: UsageEvent) => void): Promise<UnlistenFn> =>
    listen<UsageEvent>("chat://usage", (ev) => cb(ev.payload)),
  browserState: (cb: (e: BrowserStateEvent) => void): Promise<UnlistenFn> =>
    listen<BrowserStateEvent>("browser://state", (ev) => cb(ev.payload)),
  chatError: (cb: (e: ChatErrorEvent) => void): Promise<UnlistenFn> =>
    listen<ChatErrorEvent>("chat://error", (ev) => cb(ev.payload)),
  terminalData: (cb: (e: TerminalDataEvent) => void): Promise<UnlistenFn> =>
    listen<TerminalDataEvent>("terminal://data", (ev) => cb(ev.payload)),
  terminalExit: (cb: (e: TerminalExitEvent) => void): Promise<UnlistenFn> =>
    listen<TerminalExitEvent>("terminal://exit", (ev) => cb(ev.payload)),
  fsChanged: (cb: (e: FsChangedEvent) => void): Promise<UnlistenFn> =>
    listen<FsChangedEvent>("fs://changed", (ev) => cb(ev.payload)),
  remoteStatus: (cb: (e: RemoteStatus) => void): Promise<UnlistenFn> =>
    listen<RemoteStatus>("remote://status", (ev) => cb(ev.payload)),
  remotePairing: (cb: (e: RemotePairingEvent) => void): Promise<UnlistenFn> =>
    listen<RemotePairingEvent>("remote://pairing", (ev) => cb(ev.payload)),
  /** A session was created from the phone. */
  remoteSessions: (cb: (e: { workspace_id: string }) => void): Promise<UnlistenFn> =>
    listen<{ workspace_id: string }>("remote://sessions", (ev) => cb(ev.payload)),
  remoteTerminal: (cb: (e: RemoteTerminalEvent) => void): Promise<UnlistenFn> =>
    listen<RemoteTerminalEvent>("remote://terminal", (ev) => cb(ev.payload)),
  terminalResized: (cb: (e: { terminal_id: string; cols: number; rows: number }) => void): Promise<UnlistenFn> =>
    listen<{ terminal_id: string; cols: number; rows: number }>("terminal://resized", (ev) => cb(ev.payload)),
};
