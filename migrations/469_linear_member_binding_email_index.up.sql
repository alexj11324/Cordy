CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_member_binding_email
    ON linear_member_binding (workspace_id, normalized_email);
