-- "Remove from list" hides a project instead of deleting it (deleting
-- cascades to its chat sessions). Opening the folder again shows it.
ALTER TABLE workspaces ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
