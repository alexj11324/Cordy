-- A hub installation has no legacy Agent target. Refuse the rollback before
-- changing the constraint instead of deleting the installation and its
-- channel history implicitly. Operators must explicitly revoke or migrate
-- every hub installation before retrying this rollback.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM channel_installation
        WHERE agent_id IS NULL
    ) THEN
        RAISE EXCEPTION
            'cannot restore channel_installation.agent_id NOT NULL while workspace hub installations exist';
    END IF;
END
$$;

ALTER TABLE channel_installation
ALTER COLUMN agent_id SET NOT NULL;
