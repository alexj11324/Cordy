-- Persistent reviewer on an issue, independent of the current assignee.
-- Entering review requires a reviewer; once set, the reviewer can be
-- replaced but not cleared. No FK: the application layer owns the
-- member/agent/team relationship, matching assignee_type/assignee_id.
ALTER TABLE issue
    ADD COLUMN IF NOT EXISTS reviewer_type TEXT,
    ADD COLUMN IF NOT EXISTS reviewer_id UUID;
