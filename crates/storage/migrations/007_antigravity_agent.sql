-- Offer the Antigravity CLI (`agy`) in the chat column's "+" menu for users
-- whose agent list was already saved (new installs get it from the serde
-- default). Appends once; the rest of the user's list is left as is.
UPDATE settings
SET value_json = json_insert(
    value_json,
    '$.agent_clis[#]',
    json('{"id":"antigravity","name":"Antigravity","command":"agy","enabled":true}')
)
WHERE key = 'app'
  AND json_type(value_json, '$.agent_clis') = 'array'
  AND NOT EXISTS (
    SELECT 1 FROM json_each(settings.value_json, '$.agent_clis')
    WHERE json_extract(json_each.value, '$.id') = 'antigravity'
  );
