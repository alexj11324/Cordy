CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_member_binding_linear
    ON linear_member_binding (workspace_id, connection_id, linear_user_id);
