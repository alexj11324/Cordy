-- name: ListAutomationMemories :many
SELECT name, revision, updated_at FROM automation_memory
WHERE automation_id = $1 AND NOT deleted ORDER BY name;

-- name: GetAutomationMemory :one
-- Includes tombstones so delete/recreate never reuses an editor revision.
SELECT * FROM automation_memory WHERE automation_id = $1 AND name = $2;

-- name: PutAutomationMemory :one
-- Caller holds the workspace share lock then the automation update lock.
INSERT INTO automation_memory (automation_id, name, content)
VALUES ($1, $2, $3)
ON CONFLICT (automation_id, name) DO UPDATE
SET content = EXCLUDED.content, revision = automation_memory.revision + 1,
    deleted = false, updated_at = now()
RETURNING *;

-- name: DeleteAutomationMemory :exec
UPDATE automation_memory SET content = '', deleted = true,
    revision = revision + 1, updated_at = now()
WHERE automation_id = $1 AND name = $2;

-- name: LockAutomationMemoryWorkspace :one
-- Workspace teardown holds FOR UPDATE before cleaning children.
SELECT id FROM workspace WHERE id = $1 FOR KEY SHARE;
