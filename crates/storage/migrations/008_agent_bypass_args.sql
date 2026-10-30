-- Bypass-permission flags for agent CLIs in saved agent lists (new installs
-- get them from the serde default). Only fills entries that have none.
UPDATE settings
SET value_json = json_set(value_json, '$.agent_clis', json((
    SELECT json_group_array(
        CASE
            WHEN json_extract(e.value, '$.bypass_args') IS NULL AND p.args IS NOT NULL
                THEN json(json_set(e.value, '$.bypass_args', p.args))
            ELSE json(e.value)
        END
        ORDER BY e.key
    )
    FROM json_each(settings.value_json, '$.agent_clis') AS e
    LEFT JOIN (
        SELECT 'claude' AS id, '--dangerously-skip-permissions' AS args
        UNION ALL SELECT 'codex', '--dangerously-bypass-approvals-and-sandbox'
        UNION ALL SELECT 'opencode', '--auto'
        UNION ALL SELECT 'antigravity', '--dangerously-skip-permissions'
    ) AS p ON p.id = json_extract(e.value, '$.id')
)))
WHERE key = 'app' AND json_type(value_json, '$.agent_clis') = 'array';
