-- Keep published migration history immutable and move the remaining persisted
-- product identifiers forward at one explicit compatibility boundary.

ALTER TABLE lark_user_binding
    RENAME COLUMN cordy_user_id TO patchbay_user_id;

ALTER TABLE channel_user_binding
    RENAME COLUMN cordy_user_id TO patchbay_user_id;

UPDATE agent_runtime
SET provider = 'patchbay_agent'
WHERE provider = 'cordy_agent';

DROP TRIGGER IF EXISTS trg_atq_dirty_hourly ON agent_task_queue;
CREATE TRIGGER trg_atq_dirty_hourly
BEFORE UPDATE OF runtime_id, issue_id OR DELETE ON agent_task_queue
FOR EACH ROW
WHEN (current_setting('patchbay.workspace_teardown', true) IS DISTINCT FROM 'on')
EXECUTE FUNCTION enqueue_task_usage_hourly_dirty_for_atq();

DROP TRIGGER IF EXISTS trg_issue_delete_dirty_hourly ON issue;
CREATE TRIGGER trg_issue_delete_dirty_hourly
BEFORE DELETE ON issue
FOR EACH ROW
WHEN (current_setting('patchbay.workspace_teardown', true) IS DISTINCT FROM 'on')
EXECUTE FUNCTION enqueue_task_usage_hourly_dirty_for_issue_delete();

DROP TRIGGER IF EXISTS trg_tu_dirty_hourly ON task_usage;
CREATE TRIGGER trg_tu_dirty_hourly
BEFORE DELETE ON task_usage
FOR EACH ROW
WHEN (current_setting('patchbay.workspace_teardown', true) IS DISTINCT FROM 'on')
EXECUTE FUNCTION enqueue_task_usage_hourly_dirty_for_tu();
