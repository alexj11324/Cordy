CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_one_pending_task_per_issue_agent_v3
    ON agent_task_queue (issue_id, agent_id)
    WHERE (
        status IN ('queued', 'dispatched')
        AND COALESCE(context->>'side_chat_parent_task_id', '') = ''
    )
       OR (status = 'deferred' AND context->>'channel_issue_media_pending' = 'true');
