-- Saved npm installers from earlier releases (v0.1.16's plain `npm install
-- -g`, which fails with EACCES when npm's global folder belongs to root;
-- v0.1.17's sudo fallback, which fails when npm is missing) are replaced by
-- the current default, which also gets Node.js first.
-- @NPM_INSTALL_UPGRADES@ (old -> new) is filled in by the migration runner.
-- Only values still equal to an old default are changed.
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
