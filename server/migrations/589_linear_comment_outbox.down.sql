DROP TRIGGER trg_linear_comment_outbox ON comment;
DROP FUNCTION enqueue_linear_comment_outbox();
DELETE FROM linear_sync_outbox WHERE event_type LIKE 'comment_%';
ALTER TABLE linear_sync_outbox DROP CONSTRAINT linear_sync_outbox_event_type_check;
ALTER TABLE linear_sync_outbox ADD CONSTRAINT linear_sync_outbox_event_type_check
    CHECK (event_type IN ('issue_created','issue_updated','issue_deleted'));
