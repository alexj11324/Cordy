CREATE INDEX CONCURRENTLY authorization_audit_event_lookup_idx ON authorization_audit_event(workspace_id, created_at DESC, id);
