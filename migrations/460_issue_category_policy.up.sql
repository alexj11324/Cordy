-- Workspace-wide defaults for dependency admission and review handoff. The
-- relationship to agent is intentionally application-owned (no foreign key),
-- matching the repository's explicit cleanup and workspace-boundary policy.
CREATE TABLE workspace_issue_category_policy (
    workspace_id UUID NOT NULL,
    category TEXT NOT NULL CHECK (category IN ('in_progress', 'in_review')),
    default_execution_agent_id UUID,
    default_reviewer_agent_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
