DROP TRIGGER IF EXISTS issue_review_submission_gate ON issue;
DROP FUNCTION IF EXISTS enforce_issue_review_submission();
ALTER TABLE issue DROP COLUMN IF EXISTS review_submission;
