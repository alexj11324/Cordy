-- Split the historical issue assignee into owner (member) / executor
-- (agent|team) / reviewer. No FK, no cascade.

ALTER TABLE issue
    ADD COLUMN owner_type TEXT,
    ADD COLUMN owner_id UUID,
    ADD COLUMN reviewer_type TEXT,
    ADD COLUMN reviewer_id UUID;

UPDATE issue
SET owner_type = 'member',
    owner_id = assignee_id
WHERE assignee_type = 'member';

-- A historical member assignee was not an executable target. Move active
-- work back to Todo before clearing that target.
UPDATE issue
SET status = 'todo'
WHERE assignee_type = 'member'
  AND issue_effective_status(workspace_id, status) IN ('in_progress', 'in_review', 'blocked');

UPDATE issue
SET assignee_type = NULL,
    assignee_id = NULL
WHERE assignee_type = 'member';

ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_assignee_type_check;
ALTER TABLE issue RENAME COLUMN assignee_type TO executor_type;
ALTER TABLE issue RENAME COLUMN assignee_id TO executor_id;

ALTER TABLE issue
    ADD CONSTRAINT issue_owner_pair_check CHECK (
        (owner_type IS NULL AND owner_id IS NULL)
        OR (owner_type = 'member' AND owner_id IS NOT NULL)
    ),
    ADD CONSTRAINT issue_executor_pair_check CHECK (
        (executor_type IS NULL AND executor_id IS NULL)
        OR (executor_type IN ('agent', 'team') AND executor_id IS NOT NULL)
    ),
    ADD CONSTRAINT issue_reviewer_pair_check CHECK (
        (reviewer_type IS NULL AND reviewer_id IS NULL)
        OR (reviewer_type IN ('member', 'agent', 'team') AND reviewer_id IS NOT NULL)
    );
