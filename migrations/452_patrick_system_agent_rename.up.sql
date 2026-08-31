DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM agent
        WHERE system_key IN ('mika', 'patrick')
        GROUP BY workspace_id
        HAVING COUNT(*) > 1
    ) THEN
        RAISE EXCEPTION
            'cannot rename the built-in agent to patrick: a workspace contains multiple orchestrator identities';
    END IF;
    IF EXISTS (
        SELECT 1
        FROM agent legacy
        JOIN agent canonical
          ON canonical.workspace_id = legacy.workspace_id
         AND canonical.system_key = 'patrick'
         AND canonical.id <> legacy.id
        WHERE legacy.system_key = 'mika'
    ) THEN
        RAISE EXCEPTION
            'cannot rename the built-in agent to patrick: a workspace contains both system identities';
    END IF;
END
$$;

UPDATE agent
SET system_key = 'patrick',
    name = CASE WHEN name = 'Mika' THEN 'Patrick' ELSE name END,
    updated_at = now()
WHERE system_key = 'mika';

UPDATE workspace
SET settings = jsonb_set(
        settings,
        '{orchestrator_system_key}',
        '"patrick"'::jsonb,
        false
    ),
    updated_at = now()
WHERE settings->>'orchestrator_system_key' = 'mika';
