-- Arguments that continue an agent CLI's last conversation, for saved agent
-- lists (new installs get them from the serde default). Only fills entries
-- that have none.
UPDATE settings
SET value_json = json_set(value_json, '$.agent_clis', json((
    SELECT json_group_array(
        CASE
            WHEN json_extract(e.value, '$.resume_args') IS NULL AND p.args IS NOT NULL
                THEN json(json_set(e.value, '$.resume_args', p.args))
            ELSE json(e.value)
        END
        ORDER BY e.key
    )
    FROM json_each(settings.value_json, '$.agent_clis') AS e
    LEFT JOIN (
        SELECT 'claude' AS id, '--continue' AS args
        UNION ALL SELECT 'codex', 'resume --last'
        UNION ALL SELECT 'opencode', '--continue'
        UNION ALL SELECT 'antigravity', '--continue'
    ) AS p ON p.id = json_extract(e.value, '$.id')
)))
WHERE key = 'app' AND json_type(value_json, '$.agent_clis') = 'array';
