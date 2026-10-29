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

export interface MessageRecord {
  id: string;
  session_id: string;
  role: Role;
  content: string;
  tool_calls: ToolCall[];
  tool_call_id?: string | null;
  tool_name?: string | null;
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
  sendMessage: (sessionId: string, content: string, attachments: string[]) =>
    invoke<void>("send_message", { sessionId, content, attachments }),
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
  terminalCreate: (id: string, cols?: number, rows?: number) =>
    invoke<void>("terminal_create", { id, cols: cols ?? null, rows: rows ?? null }),
  terminalWrite: (id: string, data: string) => invoke<void>("terminal_write", { id, data }),
  terminalResize: (id: string, cols: number, rows: number) =>
    invoke<void>("terminal_resize", { id, cols, rows }),
  terminalKill: (id: string) => invoke<void>("terminal_kill", { id }),
  terminalList: () => invoke<string[]>("terminal_list"),

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
};
