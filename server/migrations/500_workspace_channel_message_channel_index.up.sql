CREATE INDEX CONCURRENTLY IF NOT EXISTS workspace_channel_message_channel_created_idx
    ON workspace_channel_message (channel_id, created_at, id);
