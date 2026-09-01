DROP TRIGGER IF EXISTS trg_issue_assignee_generation ON issue;
DROP FUNCTION IF EXISTS bump_issue_assignee_generation();
ALTER TABLE issue DROP COLUMN IF EXISTS assignee_generation;
