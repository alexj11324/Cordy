-- Workspace channels and channel message surface for W8.  These tables are
-- intentionally FK-free, so every workspace/entity boundary belongs in the
-- query as well as in the HTTP handler.

-- name: GetWorkspaceChannelByID :one
SELECT id, workspace_id, name, slug, description, created_by, archived_at, created_at, updated_at
FROM workspace_channel
WHERE id = $1 AND workspace_id = $2 AND archived_at IS NULL;

-- name: GetWorkspaceChannelBySlug :one
SELECT id, workspace_id, name, slug, description, created_by, archived_at, created_at, updated_at
FROM workspace_channel
WHERE workspace_id = $1 AND slug = $2 AND archived_at IS NULL
LIMIT 1;

-- name: ListWorkspaceChannels :many
SELECT id, workspace_id, name, slug, description, created_by, archived_at, created_at, updated_at
FROM workspace_channel
WHERE workspace_id = $1 AND archived_at IS NULL
ORDER BY created_at, id;

-- name: CreateWorkspaceChannel :one
INSERT INTO workspace_channel (workspace_id, slug, name, description, created_by, id)
VALUES (
    sqlc.arg('workspace_id'),
    sqlc.arg('slug'),
    sqlc.arg('name'),
    COALESCE(sqlc.narg('description')::text, ''),
    sqlc.arg('created_by'),
    COALESCE(sqlc.narg('id')::uuid, gen_random_uuid())
)
RETURNING id, workspace_id, name, slug, description, created_by, archived_at, created_at, updated_at;

-- name: GetWorkspaceChannelMessageByID :one
SELECT id, workspace_id, channel_id, author_type, author_id, content, parent_id, quoted_message_id, created_at, updated_at
FROM workspace_channel_message AS message
WHERE message.id = $1
  AND message.workspace_id = $2
  AND EXISTS (
      SELECT 1
      FROM workspace_channel AS channel
      WHERE channel.id = message.channel_id
        AND channel.workspace_id = message.workspace_id
        AND channel.archived_at IS NULL
  );

-- name: GetWorkspaceChannelMessageTaskSource :one
-- A channel mention task stores this message id in the existing
-- trigger_evidence_kind/trigger_evidence_ref_id pair. Resolve the complete
-- source under one workspace/channel/message/actor fence and hold the source
-- rows until the task and its input message commit. Channel archival does not
-- invalidate historical provenance: the message remains the durable source.
SELECT message.id, message.workspace_id, message.channel_id,
       message.author_type, message.author_id, message.content,
       message.parent_id, message.quoted_message_id,
       message.created_at, message.updated_at
FROM workspace_channel_message AS message
JOIN workspace_channel AS channel
  ON channel.id = message.channel_id
 AND channel.workspace_id = message.workspace_id
WHERE message.id = sqlc.arg('message_id')
  AND message.workspace_id = sqlc.arg('workspace_id')
  AND message.channel_id = sqlc.arg('channel_id')
  AND message.author_type = sqlc.arg('actor_type')
  AND message.author_id = sqlc.arg('actor_id')
FOR KEY SHARE OF message, channel;

-- name: ListWorkspaceChannelMessages :many
SELECT id, workspace_id, channel_id, author_type, author_id, content, parent_id, quoted_message_id, created_at, updated_at
FROM workspace_channel_message AS message
WHERE message.workspace_id = $1
  AND message.channel_id = $2
  AND EXISTS (
      SELECT 1
      FROM workspace_channel AS channel
      WHERE channel.id = message.channel_id
        AND channel.workspace_id = message.workspace_id
        AND channel.archived_at IS NULL
  )
  AND (
      sqlc.narg('before_created_at')::timestamptz IS NULL
      OR (message.created_at, message.id) < (
          sqlc.narg('before_created_at')::timestamptz,
          sqlc.narg('before_id')::uuid
      )
  )
ORDER BY message.created_at DESC, message.id DESC
LIMIT sqlc.arg('limit');

-- name: CreateWorkspaceChannelMessage :one
WITH channel AS MATERIALIZED (
    SELECT id
    FROM workspace_channel
    WHERE id = sqlc.arg('channel_id')
      AND workspace_id = sqlc.arg('workspace_id')
      AND archived_at IS NULL
), valid_author AS MATERIALIZED (
    SELECT 1
    WHERE (
        (sqlc.arg('author_type') = 'member' AND EXISTS (
            SELECT 1
            FROM member
            WHERE workspace_id = sqlc.arg('workspace_id')
              AND user_id = sqlc.arg('author_id')
        ))
        OR (sqlc.arg('author_type') = 'agent' AND EXISTS (
            SELECT 1
            FROM agent
            WHERE workspace_id = sqlc.arg('workspace_id')
              AND id = sqlc.arg('author_id')
        ))
    )
), valid_parent AS MATERIALIZED (
    SELECT 1
    WHERE sqlc.narg('parent_id')::uuid IS NULL
       OR EXISTS (
           SELECT 1
           FROM workspace_channel_message
           WHERE id = sqlc.narg('parent_id')::uuid
             AND workspace_id = sqlc.arg('workspace_id')
             AND channel_id = sqlc.arg('channel_id')
       )
), valid_quote AS MATERIALIZED (
    SELECT 1
    WHERE sqlc.narg('quoted_message_id')::uuid IS NULL
       OR EXISTS (
           SELECT 1
           FROM workspace_channel_message
           WHERE id = sqlc.narg('quoted_message_id')::uuid
             AND workspace_id = sqlc.arg('workspace_id')
             AND channel_id = sqlc.arg('channel_id')
       )
)
INSERT INTO workspace_channel_message (
    workspace_id, channel_id, author_type, author_id, content,
    parent_id, quoted_message_id, id
)
SELECT
    sqlc.arg('workspace_id'),
    sqlc.arg('channel_id'),
    sqlc.arg('author_type'),
    sqlc.arg('author_id'),
    sqlc.arg('content'),
    sqlc.narg('parent_id'),
    sqlc.narg('quoted_message_id'),
    COALESCE(sqlc.narg('id')::uuid, gen_random_uuid())
FROM channel, valid_author, valid_parent, valid_quote
RETURNING id, workspace_id, channel_id, author_type, author_id, content, parent_id, quoted_message_id, created_at, updated_at;
