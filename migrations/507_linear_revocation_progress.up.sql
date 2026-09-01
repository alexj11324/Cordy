-- Durable per-task progress for Linear revocation cleanup. The relation is
-- intentionally foreign-key free: task/session teardown owns its cleanup in
-- application transactions, while progress survives a process crash between
-- post-commit side effects and the connection-level completion marker.
CREATE TABLE linear_revocation_cancellation_progress (
    connection_id UUID NOT NULL,
    task_id UUID NOT NULL,
    completed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
