-- JenderalCode Desktop — initial schema (PRD §11).
-- All timestamps are i64 milliseconds since the Unix epoch (UTC).
-- API keys are NEVER stored here; providers.credential_ref references the
-- OS credential store.

CREATE TABLE workspaces (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  root_path TEXT NOT NULL UNIQUE,
  last_opened_at INTEGER NOT NULL
);

CREATE TABLE providers (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  base_url TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  extra_json TEXT NOT NULL DEFAULT '{}',
  credential_ref TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE models (
  id TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
  model_name TEXT NOT NULL,
  display_name TEXT NOT NULL,
  capabilities_json TEXT NOT NULL DEFAULT '{}',
  input_price_per_mtok REAL,
  output_price_per_mtok REAL,
  custom INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL,
  UNIQUE(provider_id, model_name)
);

CREATE TABLE agent_profiles (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  mode TEXT NOT NULL,
  system_prompt TEXT NOT NULL,
  allowed_tools_json TEXT,
  policy_mode TEXT NOT NULL DEFAULT 'safe',
  provider_id TEXT,
  model_id TEXT,
  built_in INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  agent_profile_id TEXT,
  provider_id TEXT,
  model_name TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_sessions_workspace ON sessions(workspace_id, updated_at DESC);

CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  content TEXT NOT NULL,
  tool_calls_json TEXT,
  tool_call_id TEXT,
  tool_name TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_messages_session ON messages(session_id, created_at);

CREATE TABLE tool_runs (
  id TEXT PRIMARY KEY,
  message_id TEXT,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  tool_name TEXT NOT NULL,
  status TEXT NOT NULL,
  args_json TEXT NOT NULL DEFAULT '{}',
  result_summary TEXT,
  error TEXT,
  approval_id TEXT,
  created_at INTEGER NOT NULL,
  finished_at INTEGER
);
CREATE INDEX idx_tool_runs_session ON tool_runs(session_id, created_at);

CREATE TABLE approvals (
  id TEXT PRIMARY KEY,
  tool_run_id TEXT NOT NULL,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  description TEXT NOT NULL,
  preview TEXT,
  status TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  decided_at INTEGER
);
CREATE INDEX idx_approvals_session ON approvals(session_id, created_at);

CREATE TABLE usage_records (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  provider_id TEXT NOT NULL,
  model_name TEXT NOT NULL,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cost_estimate REAL,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_usage_session ON usage_records(session_id);
CREATE INDEX idx_usage_created ON usage_records(created_at);

CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value_json TEXT NOT NULL
);
