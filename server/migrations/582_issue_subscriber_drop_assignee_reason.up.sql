-- The current issue role model is owner(member) / executor(agent|team) /
-- reviewer. Older subscriber rows used the aggregate `assignee` reason;
-- classify those rows from the live issue role before removing that spelling
-- from the current constraint. Rows that no longer match either role remain
-- subscriptions, but become explicit manual subscriptions rather than being
-- silently discarded.
ALTER TABLE issue_subscriber DROP CONSTRAINT IF EXISTS issue_subscriber_reason_check;

UPDATE issue_subscriber AS subscriber
SET reason = CASE
    WHEN issue.owner_type = 'member'
         AND subscriber.user_type = 'member'
         AND subscriber.user_id = issue.owner_id
        THEN 'owner'
    WHEN issue.executor_type = 'agent'
         AND subscriber.user_type = 'agent'
         AND subscriber.user_id = issue.executor_id
        THEN 'executor'
    ELSE 'manual'
END
FROM issue
WHERE subscriber.issue_id = issue.id
  AND subscriber.reason = 'assignee';

ALTER TABLE issue_subscriber ADD CONSTRAINT issue_subscriber_reason_check
    CHECK (reason IN (
        'creator', 'commenter', 'mentioned', 'manual',
        'automation', 'delegated', 'executor', 'owner', 'reviewer'
    )) NOT VALID;

ALTER TABLE issue_subscriber VALIDATE CONSTRAINT issue_subscriber_reason_check;
