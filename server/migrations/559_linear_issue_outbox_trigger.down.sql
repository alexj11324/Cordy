DROP TRIGGER IF EXISTS trg_linear_issue_outbox ON issue;
DROP FUNCTION IF EXISTS enqueue_linear_issue_outbox();
ALTER TABLE linear_sync_outbox DROP CONSTRAINT IF EXISTS linear_sync_outbox_event_type_check;
ALTER TABLE linear_sync_outbox ADD CONSTRAINT linear_sync_outbox_event_type_check
    CHECK (event_type IN ('issue_created', 'issue_updated'));
