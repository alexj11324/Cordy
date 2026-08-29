CREATE UNIQUE INDEX CONCURRENTLY idx_channel_installation_hub_workspace_type
ON channel_installation (workspace_id, channel_type)
WHERE agent_id IS NULL;
