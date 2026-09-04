CREATE INDEX CONCURRENTLY IF NOT EXISTS authorization_audit_event_lookup_idx ON authorization_audit_event(workspace_id, created_at DESC, id);
