-- A graph apply commits its child issues before the in-process event bus can
-- fan out issue:created. Keep one durable publication row per node so an
-- idempotent apply can recover a process failure between those two steps.
CREATE TABLE dependency_graph_issue_created_outbox (
    plan_id UUID NOT NULL,
    node_id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    issue_id UUID NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'processing', 'published')),
    attempt INTEGER NOT NULL DEFAULT 0,
    lease_owner TEXT,
    lease_expires_at TIMESTAMPTZ,
    published_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
