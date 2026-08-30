-- Candidate only: reverse automation_rename.up.sql during a coordinated
-- rollback with the canonical binary stopped.
BEGIN;

ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_origin_type_check;
UPDATE issue SET origin_type = 'autopilot' WHERE origin_type = 'automation';
ALTER TABLE issue ADD CONSTRAINT issue_origin_type_check
    CHECK (origin_type IN ('autopilot', 'quick_create', 'lark_chat', 'slack_chat', 'agent_create', 'dingtalk_chat', 'wecom_chat', 'telegram_chat'))
    NOT VALID;
ALTER TABLE issue VALIDATE CONSTRAINT issue_origin_type_check;

ALTER TABLE issue_subscriber DROP CONSTRAINT IF EXISTS issue_subscriber_reason_check;
UPDATE issue_subscriber SET reason = 'autopilot' WHERE reason = 'automation';
ALTER TABLE issue_subscriber ADD CONSTRAINT issue_subscriber_reason_check
    CHECK (reason IN ('creator', 'assignee', 'commenter', 'mentioned', 'manual', 'autopilot', 'delegated'))
    NOT VALID;
ALTER TABLE issue_subscriber VALIDATE CONSTRAINT issue_subscriber_reason_check;

ALTER TABLE automation RENAME CONSTRAINT automation_pkey TO autopilot_pkey;
ALTER TABLE automation RENAME CONSTRAINT automation_workspace_id_fkey TO autopilot_workspace_id_fkey;
ALTER TABLE automation RENAME CONSTRAINT automation_project_id_fkey TO autopilot_project_id_fkey;
ALTER TABLE automation RENAME CONSTRAINT automation_assignee_type_check TO autopilot_assignee_type_check;
ALTER TABLE automation_trigger RENAME CONSTRAINT automation_trigger_pkey TO autopilot_trigger_pkey;
ALTER TABLE automation_trigger RENAME CONSTRAINT automation_trigger_automation_id_fkey TO autopilot_trigger_autopilot_id_fkey;
ALTER TABLE automation_run RENAME CONSTRAINT automation_run_pkey TO autopilot_run_pkey;
ALTER TABLE automation_run RENAME CONSTRAINT automation_run_automation_id_fkey TO autopilot_run_autopilot_id_fkey;
ALTER TABLE automation_run RENAME CONSTRAINT automation_run_trigger_id_fkey TO autopilot_run_trigger_id_fkey;
ALTER TABLE automation_run RENAME CONSTRAINT automation_run_issue_id_fkey TO autopilot_run_issue_id_fkey;
ALTER TABLE automation_run RENAME CONSTRAINT automation_run_task_id_fkey TO autopilot_run_task_id_fkey;
ALTER TABLE automation_run RENAME CONSTRAINT automation_run_status_check TO autopilot_run_status_check;
ALTER TABLE automation_rule_version RENAME CONSTRAINT automation_rule_version_pkey TO autopilot_rule_version_pkey;
ALTER TABLE automation_subscriber RENAME CONSTRAINT automation_subscriber_pkey TO autopilot_subscriber_pkey;
ALTER TABLE automation_collaborator RENAME CONSTRAINT automation_collaborator_pkey TO autopilot_collaborator_pkey;
ALTER TABLE automation_quota_period RENAME CONSTRAINT automation_quota_period_pkey TO autopilot_quota_period_pkey;
ALTER TABLE automation_quota_reservation RENAME CONSTRAINT automation_quota_reservation_pkey TO autopilot_quota_reservation_pkey;
ALTER TABLE agent_task_queue RENAME CONSTRAINT agent_task_queue_automation_run_id_fkey TO agent_task_queue_autopilot_run_id_fkey;
ALTER TABLE webhook_delivery RENAME CONSTRAINT webhook_delivery_automation_id_fkey TO webhook_delivery_autopilot_id_fkey;
ALTER TABLE webhook_delivery RENAME CONSTRAINT webhook_delivery_automation_trigger_id_fkey TO webhook_delivery_trigger_id_fkey;
ALTER TABLE webhook_delivery RENAME CONSTRAINT webhook_delivery_automation_run_id_fkey TO webhook_delivery_autopilot_run_id_fkey;

ALTER INDEX IF EXISTS automation_pkey RENAME TO autopilot_pkey;
ALTER INDEX IF EXISTS idx_automation_workspace RENAME TO idx_autopilot_workspace;
ALTER INDEX IF EXISTS idx_automation_assignee RENAME TO idx_autopilot_assignee;
ALTER INDEX IF EXISTS automation_trigger_pkey RENAME TO autopilot_trigger_pkey;
ALTER INDEX IF EXISTS idx_automation_trigger_automation RENAME TO idx_autopilot_trigger_autopilot;
ALTER INDEX IF EXISTS idx_automation_trigger_next_run RENAME TO idx_autopilot_trigger_next_run;
ALTER INDEX IF EXISTS automation_run_pkey RENAME TO autopilot_run_pkey;
ALTER INDEX IF EXISTS idx_automation_run_automation RENAME TO idx_autopilot_run_autopilot;
ALTER INDEX IF EXISTS idx_automation_run_status RENAME TO idx_autopilot_run_status;
ALTER INDEX IF EXISTS idx_automation_run_issue RENAME TO idx_autopilot_run_issue;
ALTER INDEX IF EXISTS idx_automation_trigger_webhook_token RENAME TO idx_autopilot_trigger_webhook_token;
ALTER INDEX IF EXISTS idx_automation_assignee_type_id RENAME TO idx_autopilot_assignee_type_id;
ALTER INDEX IF EXISTS idx_automation_run_team_id RENAME TO idx_autopilot_run_team_id;
ALTER INDEX IF EXISTS idx_automation_project RENAME TO idx_autopilot_project;
ALTER INDEX IF EXISTS idx_automation_subscriber_user RENAME TO idx_autopilot_subscriber_user;
ALTER INDEX IF EXISTS automation_rule_version_pkey RENAME TO autopilot_rule_version_pkey;
ALTER INDEX IF EXISTS idx_automation_rule_version_active RENAME TO idx_autopilot_rule_version_active;
ALTER INDEX IF EXISTS uq_automation_run_trigger_planned RENAME TO uq_autopilot_run_trigger_planned;
ALTER INDEX IF EXISTS uq_automation_run_webhook_delivery RENAME TO uq_autopilot_run_webhook_delivery;
ALTER INDEX IF EXISTS idx_automation_run_task_id RENAME TO idx_autopilot_run_task_id;
ALTER INDEX IF EXISTS automation_subscriber_pkey RENAME TO autopilot_subscriber_pkey;
ALTER INDEX IF EXISTS automation_collaborator_pkey RENAME TO autopilot_collaborator_pkey;
ALTER INDEX IF EXISTS idx_webhook_delivery_automation RENAME TO idx_webhook_delivery_autopilot;
ALTER INDEX IF EXISTS idx_webhook_delivery_automation_run RENAME TO idx_webhook_delivery_run;
ALTER INDEX IF EXISTS automation_quota_period_pkey RENAME TO autopilot_quota_period_pkey;
ALTER INDEX IF EXISTS automation_quota_reservation_pkey RENAME TO autopilot_quota_reservation_pkey;
ALTER INDEX IF EXISTS uq_automation_quota_reservation_key RENAME TO uq_autopilot_quota_reservation_key;
ALTER INDEX IF EXISTS uq_automation_run_quota_reservation RENAME TO uq_autopilot_run_quota_reservation;
ALTER INDEX IF EXISTS idx_automation_quota_reservation_state RENAME TO idx_autopilot_quota_reservation_state;

ALTER TABLE automation_trigger RENAME COLUMN automation_id TO autopilot_id;
ALTER TABLE automation_run RENAME COLUMN automation_id TO autopilot_id;
ALTER TABLE automation_rule_version RENAME COLUMN automation_id TO autopilot_id;
ALTER TABLE automation_subscriber RENAME COLUMN automation_id TO autopilot_id;
ALTER TABLE automation_collaborator RENAME COLUMN automation_id TO autopilot_id;
ALTER TABLE agent_task_queue RENAME COLUMN automation_run_id TO autopilot_run_id;
ALTER TABLE webhook_delivery RENAME COLUMN automation_id TO autopilot_id;
ALTER TABLE webhook_delivery RENAME COLUMN automation_run_id TO autopilot_run_id;

ALTER TABLE automation RENAME TO autopilot;
ALTER TABLE automation_trigger RENAME TO autopilot_trigger;
ALTER TABLE automation_run RENAME TO autopilot_run;
ALTER TABLE automation_rule_version RENAME TO autopilot_rule_version;
ALTER TABLE automation_subscriber RENAME TO autopilot_subscriber;
ALTER TABLE automation_collaborator RENAME TO autopilot_collaborator;
ALTER TABLE automation_quota_period RENAME TO autopilot_quota_period;
ALTER TABLE automation_quota_reservation RENAME TO autopilot_quota_reservation;

COMMIT;
