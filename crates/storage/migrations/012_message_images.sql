-- Images pasted into user messages (JSON array of {mime, data}).
ALTER TABLE messages ADD COLUMN images_json TEXT;
