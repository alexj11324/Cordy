UPDATE issue_subscriber
SET reason = 'assignee'
WHERE reason IN ('executor', 'owner');

ALTER TABLE issue_subscriber DROP CONSTRAINT IF EXISTS issue_subscriber_reason_check;

ALTER TABLE issue_subscriber ADD CONSTRAINT issue_subscriber_reason_check
    CHECK (reason IN (
        'creator', 'assignee', 'commenter', 'mentioned', 'manual',
        'automation', 'delegated'
    )) NOT VALID;

ALTER TABLE issue_subscriber VALIDATE CONSTRAINT issue_subscriber_reason_check;
