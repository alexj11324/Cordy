CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_revocation_progress_task
    ON linear_revocation_cancellation_progress (connection_id, task_id);
