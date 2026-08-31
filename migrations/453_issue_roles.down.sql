DROP TRIGGER IF EXISTS trg_issue_executor_generation ON issue;
DROP FUNCTION IF EXISTS bump_issue_executor_generation();

ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_owner_pair_check;
ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_executor_pair_check;
ALTER TABLE issue RENAME COLUMN executor_type TO assignee_type;
ALTER TABLE issue RENAME COLUMN executor_id TO assignee_id;
ALTER TABLE issue RENAME COLUMN executor_generation TO assignee_generation;

-- The old schema cannot represent owner and executor simultaneously. Preserve
-- the executor when present; otherwise restore the human owner as assignee.
UPDATE issue
SET assignee_type = COALESCE(assignee_type, owner_type),
    assignee_id = CASE
        WHEN assignee_type IS NULL THEN owner_id
        ELSE assignee_id
    END;

ALTER TABLE issue
    DROP COLUMN owner_type,
    DROP COLUMN owner_id,
    ADD CONSTRAINT issue_assignee_type_check
        CHECK (assignee_type IN ('member', 'agent', 'team'));

CREATE OR REPLACE FUNCTION bump_issue_assignee_generation()
RETURNS TRIGGER AS $$
BEGIN
    IF NEW.assignee_type IS DISTINCT FROM OLD.assignee_type
       OR NEW.assignee_id IS DISTINCT FROM OLD.assignee_id THEN
        NEW.assignee_generation := OLD.assignee_generation + 1;
    ELSIF NEW.assignee_generation IS DISTINCT FROM OLD.assignee_generation THEN
        NEW.assignee_generation := OLD.assignee_generation;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_issue_assignee_generation
    BEFORE UPDATE OF assignee_type, assignee_id, assignee_generation ON issue
    FOR EACH ROW
    EXECUTE FUNCTION bump_issue_assignee_generation();
