CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_channel_receive_state_installation_type
    ON channel_receive_state (installation_id, channel_type);
