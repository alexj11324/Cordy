CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS workspace_channel_message_id_uidx
    ON workspace_channel_message (id);
