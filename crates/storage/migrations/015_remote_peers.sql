-- Phones paired with this desktop for remote control (docs/REMOTE_PROTOCOL.md).
-- A row exists only after the user confirmed the 6-digit code on the desktop;
-- messages from any other device are ignored.
CREATE TABLE IF NOT EXISTS remote_peers (
    device_id   TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    public_key  TEXT NOT NULL,
    user_email  TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL,
    last_seen   INTEGER
);
