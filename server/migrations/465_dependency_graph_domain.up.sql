-- Dependency graphs are a first-class execution plan. The tables deliberately
-- do not use foreign keys: this repository owns relationship cleanup in the
-- application layer so workspace deletion and rolling migrations stay
-- explicit and auditable.

CREATE TABLE dependency_graph_plan (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL,
    parent_issue_id UUID NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    goal TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'superseded', 'cancelled')),
    created_by_type TEXT NOT NULL
        CHECK (created_by_type IN ('member', 'agent', 'system')),
    created_by_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE dependency_graph_node (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    plan_id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    temp_id TEXT NOT NULL,
    issue_id UUID NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    acceptance_criteria JSONB NOT NULL DEFAULT '[]',
    context JSONB NOT NULL DEFAULT '{}',
    outputs JSONB NOT NULL DEFAULT '[]',
    assignee_type TEXT,
    assignee_id UUID,
    candidate_assignees JSONB NOT NULL DEFAULT '[]',
    wave INTEGER NOT NULL CHECK (wave >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE dependency_graph_edge (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    plan_id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    from_issue_id UUID NOT NULL,
    to_issue_id UUID NOT NULL,
    type TEXT NOT NULL CHECK (type = 'hard'),
    reason TEXT NOT NULL,
    consumed_output TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
