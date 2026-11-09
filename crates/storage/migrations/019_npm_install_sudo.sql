-- v0.1.16 saved plain `npm install -g <pkg>` installers, which fail with
-- EACCES when npm's global folder belongs to root; the current default
-- falls back to sudo there. @NPM_INSTALL_UPGRADES@ (old -> new) is filled
-- in by the migration runner. Only values still equal to the old default
-- are changed.
UPDATE settings
SET value_json = json_set(value_json, '$.agent_clis', json((
    SELECT json_group_array(
        CASE
            WHEN u.new IS NOT NULL THEN json(json_set(e.value, '$.install_command', u.new))
            ELSE json(e.value)
        END
        ORDER BY e.key
    )
    FROM json_each(settings.value_json, '$.agent_clis') AS e
    LEFT JOIN (@NPM_INSTALL_UPGRADES@) AS u ON u.old = json_extract(e.value, '$.install_command')
)))
WHERE key = 'app' AND json_type(value_json, '$.agent_clis') = 'array';
