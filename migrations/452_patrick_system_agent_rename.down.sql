-- Roll back only when no canonical identity has been created alongside the
-- legacy one. This preserves the one-orchestrator invariant during rollback.
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
            'cannot roll back system agent rename: workspace contains duplicate Mika/Patrick identities';
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
WHERE settings ->> 'orchestrator_system_key' = 'patrick';

COMMIT;
