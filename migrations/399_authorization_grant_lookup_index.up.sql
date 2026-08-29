CREATE INDEX CONCURRENTLY IF NOT EXISTS authorization_grant_lookup_idx ON authorization_grant(workspace_id, principal_type, principal_id, action, resource_type) WHERE revoked_at IS NULL;
