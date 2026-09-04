-- Re-enable the prior current-schema reason set when rolling back 582. The
-- explicit role reasons remain intact; this only restores the value accepted
-- by the preceding migration for a subsequent historical rollback.
ALTER TABLE issue_subscriber DROP CONSTRAINT IF EXISTS issue_subscriber_reason_check;

ALTER TABLE issue_subscriber ADD CONSTRAINT issue_subscriber_reason_check
    CHECK (reason IN (
        'creator', 'assignee', 'commenter', 'mentioned', 'manual',
        'automation', 'delegated', 'executor', 'owner', 'reviewer'
    )) NOT VALID;

ALTER TABLE issue_subscriber VALIDATE CONSTRAINT issue_subscriber_reason_check;
