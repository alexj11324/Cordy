CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_member_binding_user
    ON linear_member_binding (workspace_id, connection_id, patchbay_user_id);
