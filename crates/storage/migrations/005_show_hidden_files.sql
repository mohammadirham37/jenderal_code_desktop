-- Hidden files are now shown in the explorer by default. Flip the old
-- stored default once; users can turn it off again from the explorer.
UPDATE settings
SET value_json = json_set(value_json, '$.show_hidden_files', json('true'))
WHERE key = 'app' AND json_extract(value_json, '$.show_hidden_files') = 0;
