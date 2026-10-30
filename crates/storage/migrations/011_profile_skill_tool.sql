-- The read-only `skill` tool for the built-in Plan and Review profiles,
-- whose tool allowlists are stored. Appended once.
UPDATE agent_profiles
SET allowed_tools_json = json_insert(allowed_tools_json, '$[#]', 'skill')
WHERE built_in = 1 AND id IN ('plan', 'review')
  AND json_valid(allowed_tools_json)
  AND NOT EXISTS (SELECT 1 FROM json_each(agent_profiles.allowed_tools_json) WHERE value = 'skill');
