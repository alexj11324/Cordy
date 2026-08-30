-- Candidate only: promote as the next contiguous migration after the
-- coordination, authorization, dependency-graph, and Work Product schema is
-- stable. This is a canonical-only cutover; no legacy API adapter is added.
BEGIN;

ALTER TABLE autopilot_trigger RENAME COLUMN autopilot_id TO automation_id;
ALTER TABLE autopilot_run RENAME COLUMN autopilot_id TO automation_id;
ALTER TABLE autopilot_rule_version RENAME COLUMN autopilot_id TO automation_id;
ALTER TABLE autopilot_subscriber RENAME COLUMN autopilot_id TO automation_id;
ALTER TABLE autopilot_collaborator RENAME COLUMN autopilot_id TO automation_id;
ALTER TABLE agent_task_queue RENAME COLUMN autopilot_run_id TO automation_run_id;
ALTER TABLE webhook_delivery RENAME COLUMN autopilot_id TO automation_id;
ALTER TABLE webhook_delivery RENAME COLUMN autopilot_run_id TO automation_run_id;

ALTER TABLE autopilot RENAME TO automation;
ALTER TABLE autopilot_trigger RENAME TO automation_trigger;
ALTER TABLE autopilot_run RENAME TO automation_run;
ALTER TABLE autopilot_rule_version RENAME TO automation_rule_version;
ALTER TABLE autopilot_subscriber RENAME TO automation_subscriber;
ALTER TABLE autopilot_collaborator RENAME TO automation_collaborator;
ALTER TABLE autopilot_quota_period RENAME TO automation_quota_period;
ALTER TABLE autopilot_quota_reservation RENAME TO automation_quota_reservation;

ALTER TABLE automation RENAME CONSTRAINT autopilot_pkey TO automation_pkey;
ALTER TABLE automation RENAME CONSTRAINT autopilot_workspace_id_fkey TO automation_workspace_id_fkey;
ALTER TABLE automation RENAME CONSTRAINT autopilot_project_id_fkey TO automation_project_id_fkey;
ALTER TABLE automation RENAME CONSTRAINT autopilot_assignee_type_check TO automation_assignee_type_check;
ALTER TABLE automation_trigger RENAME CONSTRAINT autopilot_trigger_pkey TO automation_trigger_pkey;
ALTER TABLE automation_trigger RENAME CONSTRAINT autopilot_trigger_autopilot_id_fkey TO automation_trigger_automation_id_fkey;
ALTER TABLE automation_run RENAME CONSTRAINT autopilot_run_pkey TO automation_run_pkey;
ALTER TABLE automation_run RENAME CONSTRAINT autopilot_run_autopilot_id_fkey TO automation_run_automation_id_fkey;
ALTER TABLE automation_run RENAME CONSTRAINT autopilot_run_trigger_id_fkey TO automation_run_trigger_id_fkey;
ALTER TABLE automation_run RENAME CONSTRAINT autopilot_run_issue_id_fkey TO automation_run_issue_id_fkey;
ALTER TABLE automation_run RENAME CONSTRAINT autopilot_run_task_id_fkey TO automation_run_task_id_fkey;
ALTER TABLE automation_run RENAME CONSTRAINT autopilot_run_status_check TO automation_run_status_check;
ALTER TABLE automation_rule_version RENAME CONSTRAINT autopilot_rule_version_pkey TO automation_rule_version_pkey;
ALTER TABLE automation_subscriber RENAME CONSTRAINT autopilot_subscriber_pkey TO automation_subscriber_pkey;
ALTER TABLE automation_collaborator RENAME CONSTRAINT autopilot_collaborator_pkey TO automation_collaborator_pkey;
ALTER TABLE automation_quota_period RENAME CONSTRAINT autopilot_quota_period_pkey TO automation_quota_period_pkey;
ALTER TABLE automation_quota_reservation RENAME CONSTRAINT autopilot_quota_reservation_pkey TO automation_quota_reservation_pkey;
ALTER TABLE agent_task_queue RENAME CONSTRAINT agent_task_queue_autopilot_run_id_fkey TO agent_task_queue_automation_run_id_fkey;
ALTER TABLE webhook_delivery RENAME CONSTRAINT webhook_delivery_autopilot_id_fkey TO webhook_delivery_automation_id_fkey;
ALTER TABLE webhook_delivery RENAME CONSTRAINT webhook_delivery_trigger_id_fkey TO webhook_delivery_automation_trigger_id_fkey;
ALTER TABLE webhook_delivery RENAME CONSTRAINT webhook_delivery_autopilot_run_id_fkey TO webhook_delivery_automation_run_id_fkey;

ALTER INDEX IF EXISTS autopilot_pkey RENAME TO automation_pkey;
ALTER INDEX IF EXISTS idx_autopilot_workspace RENAME TO idx_automation_workspace;
ALTER INDEX IF EXISTS idx_autopilot_assignee RENAME TO idx_automation_assignee;
ALTER INDEX IF EXISTS autopilot_trigger_pkey RENAME TO automation_trigger_pkey;
ALTER INDEX IF EXISTS idx_autopilot_trigger_autopilot RENAME TO idx_automation_trigger_automation;
ALTER INDEX IF EXISTS idx_autopilot_trigger_next_run RENAME TO idx_automation_trigger_next_run;
ALTER INDEX IF EXISTS autopilot_run_pkey RENAME TO automation_run_pkey;
ALTER INDEX IF EXISTS idx_autopilot_run_autopilot RENAME TO idx_automation_run_automation;
ALTER INDEX IF EXISTS idx_autopilot_run_status RENAME TO idx_automation_run_status;
ALTER INDEX IF EXISTS idx_autopilot_run_issue RENAME TO idx_automation_run_issue;
ALTER INDEX IF EXISTS idx_autopilot_trigger_webhook_token RENAME TO idx_automation_trigger_webhook_token;
ALTER INDEX IF EXISTS idx_autopilot_assignee_type_id RENAME TO idx_automation_assignee_type_id;
ALTER INDEX IF EXISTS idx_autopilot_run_team_id RENAME TO idx_automation_run_team_id;
ALTER INDEX IF EXISTS idx_autopilot_project RENAME TO idx_automation_project;
ALTER INDEX IF EXISTS idx_autopilot_subscriber_user RENAME TO idx_automation_subscriber_user;
ALTER INDEX IF EXISTS autopilot_rule_version_pkey RENAME TO automation_rule_version_pkey;
ALTER INDEX IF EXISTS idx_autopilot_rule_version_active RENAME TO idx_automation_rule_version_active;
ALTER INDEX IF EXISTS uq_autopilot_run_trigger_planned RENAME TO uq_automation_run_trigger_planned;
ALTER INDEX IF EXISTS uq_autopilot_run_webhook_delivery RENAME TO uq_automation_run_webhook_delivery;
ALTER INDEX IF EXISTS idx_autopilot_run_task_id RENAME TO idx_automation_run_task_id;
ALTER INDEX IF EXISTS autopilot_subscriber_pkey RENAME TO automation_subscriber_pkey;
ALTER INDEX IF EXISTS autopilot_collaborator_pkey RENAME TO automation_collaborator_pkey;
ALTER INDEX IF EXISTS idx_webhook_delivery_autopilot RENAME TO idx_webhook_delivery_automation;
ALTER INDEX IF EXISTS idx_webhook_delivery_run RENAME TO idx_webhook_delivery_automation_run;
ALTER INDEX IF EXISTS autopilot_quota_period_pkey RENAME TO automation_quota_period_pkey;
ALTER INDEX IF EXISTS autopilot_quota_reservation_pkey RENAME TO automation_quota_reservation_pkey;
ALTER INDEX IF EXISTS uq_autopilot_quota_reservation_key RENAME TO uq_automation_quota_reservation_key;
ALTER INDEX IF EXISTS uq_autopilot_run_quota_reservation RENAME TO uq_automation_run_quota_reservation;
ALTER INDEX IF EXISTS idx_autopilot_quota_reservation_state RENAME TO idx_automation_quota_reservation_state;

ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_origin_type_check;
UPDATE issue SET origin_type = 'automation' WHERE origin_type = 'autopilot';
ALTER TABLE issue ADD CONSTRAINT issue_origin_type_check
    CHECK (origin_type IN ('automation', 'quick_create', 'lark_chat', 'slack_chat', 'agent_create', 'dingtalk_chat', 'wecom_chat', 'telegram_chat'))
    NOT VALID;
ALTER TABLE issue VALIDATE CONSTRAINT issue_origin_type_check;

ALTER TABLE issue_subscriber DROP CONSTRAINT IF EXISTS issue_subscriber_reason_check;
UPDATE issue_subscriber SET reason = 'automation' WHERE reason = 'autopilot';
ALTER TABLE issue_subscriber ADD CONSTRAINT issue_subscriber_reason_check
    CHECK (reason IN ('creator', 'assignee', 'commenter', 'mentioned', 'manual', 'automation', 'delegated'))
    NOT VALID;
ALTER TABLE issue_subscriber VALIDATE CONSTRAINT issue_subscriber_reason_check;

COMMIT;
