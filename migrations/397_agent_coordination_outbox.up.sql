-- Durable internal handoffs. These rows are the source of truth for
-- coordinator work; the in-memory event bus is only a latency/UI hint.
--
-- IDs are attached as primary keys in migration 403 after their backing
-- indexes are built concurrently in migrations 401 and 402. Inline PRIMARY
-- KEY declarations build their indexes non-concurrently, which violates the
-- repository migration policy even for newly-created tables.
CREATE TABLE agent_coordination_outbox (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    event_key TEXT NOT NULL,
    workspace_id UUID NOT NULL,
    issue_id UUID NOT NULL,
    source_task_id UUID,
    event_type TEXT NOT NULL CHECK (event_type IN ('task_completed', 'review_returned')),
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'processing', 'completed')),
    attempt INT NOT NULL DEFAULT 0,
    lease_owner TEXT,
    lease_expires_at TIMESTAMPTZ,
    available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE agent_coordination_assignment (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    event_id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    issue_id UUID NOT NULL,
    source_task_id UUID,
    role TEXT NOT NULL CHECK (role IN ('reviewer', 'executor')),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'assigned', 'dispatched', 'blocked', 'completed')),
    owner_type TEXT CHECK (owner_type IS NULL OR owner_type IN ('agent', 'team', 'member')),
    owner_id UUID,
    dispatched_task_id UUID,
    decision JSONB NOT NULL DEFAULT '{}'::jsonb,
    attempt INT NOT NULL DEFAULT 0,
    assigned_at TIMESTAMPTZ,
    dispatched_at TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
