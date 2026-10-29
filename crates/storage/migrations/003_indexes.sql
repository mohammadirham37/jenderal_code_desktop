-- Extra lookup indexes. tool_runs(session_id) and approvals(session_id) are
-- already covered by composite indexes from 001; IF NOT EXISTS keeps this
-- migration safe to re-run.
CREATE INDEX IF NOT EXISTS idx_tool_runs_session ON tool_runs(session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_tool_runs_message ON tool_runs(message_id);
CREATE INDEX IF NOT EXISTS idx_approvals_session ON approvals(session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_approvals_tool_run ON approvals(tool_run_id);
