-- Split the historical assignee into a human owner and an execution target.
-- This is a deliberately breaking schema migration: runtime code after this
-- release no longer reads or writes the assignee columns.

ALTER TABLE issue
    ADD COLUMN owner_type TEXT,
    ADD COLUMN owner_id UUID;

UPDATE issue
SET owner_type = 'member',
    owner_id = assignee_id
WHERE assignee_type = 'member';

UPDATE issue
SET assignee_type = NULL,
    assignee_id = NULL
WHERE assignee_type = 'member';

DROP TRIGGER IF EXISTS trg_issue_assignee_generation ON issue;
DROP FUNCTION IF EXISTS bump_issue_assignee_generation();

ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_assignee_type_check;
ALTER TABLE issue RENAME COLUMN assignee_type TO executor_type;
ALTER TABLE issue RENAME COLUMN assignee_id TO executor_id;
ALTER TABLE issue RENAME COLUMN assignee_generation TO executor_generation;

ALTER TABLE issue
    ADD CONSTRAINT issue_owner_pair_check CHECK (
        (owner_type IS NULL AND owner_id IS NULL)
        OR (owner_type = 'member' AND owner_id IS NOT NULL)
    ),
    ADD CONSTRAINT issue_executor_pair_check CHECK (
        (executor_type IS NULL AND executor_id IS NULL)
        OR (executor_type IN ('agent', 'team') AND executor_id IS NOT NULL)
    );

CREATE OR REPLACE FUNCTION bump_issue_executor_generation()
RETURNS TRIGGER AS $$
BEGIN
    IF NEW.executor_type IS DISTINCT FROM OLD.executor_type
       OR NEW.executor_id IS DISTINCT FROM OLD.executor_id THEN
        NEW.executor_generation := OLD.executor_generation + 1;
    ELSIF NEW.executor_generation IS DISTINCT FROM OLD.executor_generation THEN
        NEW.executor_generation := OLD.executor_generation;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_issue_executor_generation
    BEFORE UPDATE OF executor_type, executor_id, executor_generation ON issue
    FOR EACH ROW
    EXECUTE FUNCTION bump_issue_executor_generation();
