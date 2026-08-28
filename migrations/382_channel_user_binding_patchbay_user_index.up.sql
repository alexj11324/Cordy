CREATE INDEX CONCURRENTLY idx_channel_user_binding_patchbay_user
    ON channel_user_binding(patchbay_user_id, workspace_id);
