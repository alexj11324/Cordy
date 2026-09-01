ALTER INDEX IF EXISTS idx_automation_executor RENAME TO idx_automation_assignee;
ALTER INDEX IF EXISTS idx_automation_executor_type_id RENAME TO idx_automation_assignee_type_id;

ALTER TABLE automation
    RENAME CONSTRAINT automation_executor_type_check TO automation_assignee_type_check;
ALTER TABLE quick_action
    RENAME CONSTRAINT quick_action_executor_type_check TO quick_action_assignee_type_check;

ALTER TABLE automation RENAME COLUMN executor_type TO assignee_type;
ALTER TABLE automation RENAME COLUMN executor_id TO assignee_id;
ALTER TABLE quick_action RENAME COLUMN executor_type TO assignee_type;
ALTER TABLE quick_action RENAME COLUMN executor_id TO assignee_id;
