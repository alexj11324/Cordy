-- A Patchbay human may be linked to at most one active Linear identity in a
-- workspace. The application still validates active/unique email before it
-- writes; this index closes the race between two binding requests.
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_member_binding_member
    ON linear_member_binding (workspace_id, member_id)
    WHERE member_id IS NOT NULL;
