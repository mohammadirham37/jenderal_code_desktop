-- Model reasoning ("thinking") streamed separately from the answer text.
ALTER TABLE messages ADD COLUMN reasoning TEXT;
