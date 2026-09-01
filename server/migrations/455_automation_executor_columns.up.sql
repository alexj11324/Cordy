-- Automation and quick-action targets are executors, matching issue roles.
-- No FK, no cascade.

ALTER TABLE automation RENAME COLUMN assignee_type TO executor_type;
ALTER TABLE automation RENAME COLUMN assignee_id TO executor_id;
ALTER TABLE quick_action RENAME COLUMN assignee_type TO executor_type;
ALTER TABLE quick_action RENAME COLUMN assignee_id TO executor_id;

ALTER TABLE automation
    RENAME CONSTRAINT automation_assignee_type_check TO automation_executor_type_check;
ALTER TABLE quick_action
    RENAME CONSTRAINT quick_action_assignee_type_check TO quick_action_executor_type_check;

ALTER INDEX IF EXISTS idx_automation_assignee RENAME TO idx_automation_executor;
ALTER INDEX IF EXISTS idx_automation_assignee_type_id RENAME TO idx_automation_executor_type_id;
