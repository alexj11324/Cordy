-- Monotonic owner-specific fence for implementation task completion.
-- Unlike issue.revision, this value changes only when assignee identity changes,
-- so an A -> B -> A reassignment cannot revive A's earlier task.
ALTER TABLE issue
    ADD COLUMN IF NOT EXISTS assignee_generation BIGINT NOT NULL DEFAULT 0;

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

DROP TRIGGER IF EXISTS trg_issue_assignee_generation ON issue;
CREATE TRIGGER trg_issue_assignee_generation
    BEFORE UPDATE OF assignee_type, assignee_id, assignee_generation ON issue
    FOR EACH ROW
    EXECUTE FUNCTION bump_issue_assignee_generation();
