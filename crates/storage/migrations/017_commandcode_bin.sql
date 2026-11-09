-- Command Code's preset used the `commandcode` program name, which only
-- exists in command-code >= 0.11; `cmd` (`cmdc` on Windows, filled in for
-- @COMMAND_CODE_BIN@ by the migration runner) is in every release. Only an
-- entry still holding 016's value is changed.
UPDATE settings
SET value_json = json_set(value_json, '$.agent_clis', json((
    SELECT json_group_array(
        CASE
            WHEN json_extract(e.value, '$.id') = 'commandcode'
                 AND json_extract(e.value, '$.command') = 'commandcode'
                THEN json(json_set(e.value, '$.command', '@COMMAND_CODE_BIN@'))
            ELSE json(e.value)
        END
        ORDER BY e.key
    )
    FROM json_each(settings.value_json, '$.agent_clis') AS e
)))
WHERE key = 'app' AND json_type(value_json, '$.agent_clis') = 'array';
