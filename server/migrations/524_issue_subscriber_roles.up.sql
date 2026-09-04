ALTER TABLE issue_subscriber DROP CONSTRAINT IF EXISTS issue_subscriber_reason_check;

ALTER TABLE issue_subscriber ADD CONSTRAINT issue_subscriber_reason_check
    CHECK (reason IN (
        'creator', 'assignee', 'commenter', 'mentioned', 'manual',
        'automation', 'delegated', 'executor', 'owner'
    )) NOT VALID;

ALTER TABLE issue_subscriber VALIDATE CONSTRAINT issue_subscriber_reason_check;
