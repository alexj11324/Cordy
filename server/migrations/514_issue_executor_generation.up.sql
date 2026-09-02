-- Monotonic executor-specific fence for implementation task completion.
-- Unlike issue.revision, this value changes only when executor identity changes,
-- so an A -> B -> A reassignment cannot revive A's earlier task.
ALTER TABLE issue
    ADD COLUMN IF NOT EXISTS executor_generation BIGINT NOT NULL DEFAULT 0;

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

DROP TRIGGER IF EXISTS trg_issue_executor_generation ON issue;
CREATE TRIGGER trg_issue_executor_generation
    BEFORE UPDATE OF executor_type, executor_id, executor_generation ON issue
    FOR EACH ROW
    EXECUTE FUNCTION bump_issue_executor_generation();
