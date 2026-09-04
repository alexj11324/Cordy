DROP TRIGGER IF EXISTS trg_issue_executor_generation ON issue;
DROP FUNCTION IF EXISTS bump_issue_executor_generation();
ALTER TABLE issue DROP COLUMN IF EXISTS executor_generation;
