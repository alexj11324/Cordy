-- Workspace-wide defaults for execution and review admission. Relationships
-- are application-owned; no foreign keys or cascades are introduced.
CREATE TABLE workspace_issue_category_policy (
    workspace_id UUID NOT NULL,
    category TEXT NOT NULL CHECK (category IN ('in_progress', 'in_review')),
    default_execution_agent_id UUID,
    default_reviewer_agent_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
