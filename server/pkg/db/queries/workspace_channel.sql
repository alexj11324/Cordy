-- Workspace channels and channel message surface for W8.

-- name: GetWorkspaceChannelByID :one
SELECT * FROM workspace_channel WHERE id = $1 AND workspace_id = $2;

-- name: GetWorkspaceChannelBySlug :one
SELECT * FROM workspace_channel WHERE workspace_id = $1 AND slug = $2 LIMIT 1;

-- name: ListWorkspaceChannels :many
SELECT * FROM workspace_channel WHERE workspace_id = $1 ORDER BY created_at, id;

-- name: CreateWorkspaceChannel :one
INSERT INTO workspace_channel (workspace_id, slug, name, status, id)
VALUES ($1, $2, COALESCE($3, slug), COALESCE(sqlc.narg('status')::text, 'active'), COALESCE(sqlc.narg('id')::uuid, gen_random_uuid()))
RETURNING *;

-- name: GetWorkspaceChannelMessageByID :one
SELECT * FROM workspace_channel_message WHERE id = $1 AND workspace_id = $2;

-- name: ListWorkspaceChannelMessages :many
SELECT * FROM workspace_channel_message WHERE workspace_id = $1 AND channel_id = $2 ORDER BY created_at, id LIMIT $3 OFFSET $4;

-- name: CreateWorkspaceChannelMessage :one
INSERT INTO workspace_channel_message (workspace_id, channel_id, author_type, author_id, content, parent_id, quoted_message_id, id)
VALUES ($1, $2, $3, $4, $5, sqlc.narg('parent_id'), sqlc.narg('quoted_message_id'), COALESCE(sqlc.narg('id')::uuid, gen_random_uuid()))
RETURNING *;
