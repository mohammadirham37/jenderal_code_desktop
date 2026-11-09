-- The official installer of each preset agent CLI, offered in the "+" menu
-- when the program is not found, for saved agent lists (new installs get
-- them from the serde default). @AGENT_INSTALL_COMMANDS@ is filled in by the
-- migration runner with this platform's commands. Only fills entries that
-- have none.
UPDATE settings
SET value_json = json_set(value_json, '$.agent_clis', json((
    SELECT json_group_array(
        CASE
            WHEN json_extract(e.value, '$.install_command') IS NULL AND p.cmd IS NOT NULL
                THEN json(json_set(e.value, '$.install_command', p.cmd))
            ELSE json(e.value)
        END
        ORDER BY e.key
    )
    FROM json_each(settings.value_json, '$.agent_clis') AS e
    LEFT JOIN (@AGENT_INSTALL_COMMANDS@) AS p ON p.id = json_extract(e.value, '$.id')
)))
WHERE key = 'app' AND json_type(value_json, '$.agent_clis') = 'array';
