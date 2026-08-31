DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM agent canonical
        JOIN agent legacy
          ON legacy.workspace_id = canonical.workspace_id
         AND legacy.system_key = 'mika'
         AND legacy.id <> canonical.id
        WHERE canonical.system_key = 'patrick'
    ) THEN
        RAISE EXCEPTION
            'cannot restore the built-in agent identity: a workspace contains both system identities';
    END IF;
END
$$;

UPDATE agent
SET system_key = 'mika',
    name = CASE WHEN name = 'Patrick' THEN 'Mika' ELSE name END,
    updated_at = now()
WHERE system_key = 'patrick';

UPDATE workspace
SET settings = jsonb_set(
        settings,
        '{orchestrator_system_key}',
        '"mika"'::jsonb,
        false
    ),
    updated_at = now()
WHERE settings->>'orchestrator_system_key' = 'patrick';
