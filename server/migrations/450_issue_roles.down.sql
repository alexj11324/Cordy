ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_owner_pair_check;
ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_executor_pair_check;
ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_reviewer_pair_check;

ALTER TABLE issue RENAME COLUMN executor_type TO assignee_type;
ALTER TABLE issue RENAME COLUMN executor_id TO assignee_id;

UPDATE issue
SET assignee_type = 'member',
    assignee_id = owner_id
WHERE owner_type = 'member' AND assignee_type IS NULL;

ALTER TABLE issue ADD CONSTRAINT issue_assignee_type_check
    CHECK (assignee_type IN ('member', 'agent', 'team'));

ALTER TABLE issue
    DROP COLUMN IF EXISTS owner_type,
    DROP COLUMN IF EXISTS owner_id,
    DROP COLUMN IF EXISTS reviewer_type,
    DROP COLUMN IF EXISTS reviewer_id;
