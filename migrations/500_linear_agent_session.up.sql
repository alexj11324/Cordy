-- Durable correlation between a Linear Agent Session and the Patchbay task
-- that owns its execution. Relationships are enforced by application
-- transactions; this table intentionally has no database foreign keys.
CREATE TABLE linear_agent_session (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    connection_id UUID NOT NULL,
    linear_session_id TEXT NOT NULL,
    linear_issue_id TEXT NOT NULL,
    patchbay_issue_id UUID,
    agent_id UUID,
    task_id UUID,
    action TEXT NOT NULL,
    status TEXT NOT NULL,
    prompt_context TEXT,
    last_event_id TEXT NOT NULL,
    last_event_at_ms BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
