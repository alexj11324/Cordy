-- Remove only the guard; preserve every issue and catalog row.
DROP TRIGGER IF EXISTS trg_issue_active_executor ON issue;
DROP TRIGGER IF EXISTS trg_issue_status_executor_category ON issue_status;
DROP FUNCTION IF EXISTS enforce_issue_active_executor();
DROP FUNCTION IF EXISTS enforce_issue_status_executor_category();
