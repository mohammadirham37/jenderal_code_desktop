-- New read-only tools (todo_write, web_fetch) for the built-in Plan and
-- Review profiles, whose tool allowlists are stored. Appends each tool once.
UPDATE agent_profiles
SET allowed_tools_json = json_insert(allowed_tools_json, '$[#]', 'todo_write')
WHERE built_in = 1 AND id IN ('plan', 'review')
  AND json_valid(allowed_tools_json)
  AND NOT EXISTS (SELECT 1 FROM json_each(agent_profiles.allowed_tools_json) WHERE value = 'todo_write');

UPDATE agent_profiles
SET allowed_tools_json = json_insert(allowed_tools_json, '$[#]', 'web_fetch')
WHERE built_in = 1 AND id IN ('plan', 'review')
  AND json_valid(allowed_tools_json)
  AND NOT EXISTS (SELECT 1 FROM json_each(agent_profiles.allowed_tools_json) WHERE value = 'web_fetch');
