CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS workspace_channel_workspace_slug_uidx
    ON workspace_channel (workspace_id, slug);
