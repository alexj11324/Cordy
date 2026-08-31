CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_member_binding_user
    ON linear_member_binding (workspace_id, linear_user_id);
