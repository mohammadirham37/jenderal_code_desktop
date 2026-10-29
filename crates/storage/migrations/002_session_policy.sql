-- Per-session permission policy override (persisted so a "full access"
-- choice survives app restarts). NULL = derive from the agent profile.
ALTER TABLE sessions ADD COLUMN policy_mode TEXT;
