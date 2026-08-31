-- Rename the built-in system agent identity without rewriting immutable history.
-- The guard makes a partial deployment fail closed if a workspace already has
-- both the legacy and canonical identity, rather than silently choosing one.
BEGIN;

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
            'cannot rename system agent: workspace contains duplicate Mika/Patrick identities';
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
WHERE settings ->> 'orchestrator_system_key' = 'mika';

COMMIT;
