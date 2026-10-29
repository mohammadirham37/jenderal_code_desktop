-- The default agent step limit went from 30 to 100. Raise it for users who
-- still have the old default stored (custom values are left alone).
UPDATE settings
SET value_json = json_set(value_json, '$.max_iterations', 100)
WHERE key = 'app' AND json_extract(value_json, '$.max_iterations') = 30;
