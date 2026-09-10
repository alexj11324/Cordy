-- name: ListMembers :many
SELECT * FROM member
WHERE workspace_id = $1
ORDER BY created_at ASC;

-- name: GetMember :one
SELECT * FROM member
WHERE id = $1;

-- name: GetMemberByUserAndWorkspace :one
SELECT * FROM member
WHERE user_id = $1 AND workspace_id = $2;

-- name: CreateMember :one
INSERT INTO member (workspace_id, user_id, role)
VALUES ($1, $2, $3)
RETURNING *;

-- name: UpdateMemberRole :one
UPDATE member SET role = $2
WHERE id = $1
RETURNING *;

-- name: DeleteMember :exec
DELETE FROM member WHERE id = $1;

-- name: ClearProjectMemberReference :exec
-- project.member_ids is an application-owned workspace relationship. Clear a
-- departing user's IDs in the same transaction as the member-row deletion so
-- a re-invitation cannot inherit stale project membership metadata.
UPDATE project
SET member_ids = member_ids - sqlc.arg('user_id')::text,
    updated_at = now()
WHERE workspace_id = sqlc.arg('workspace_id')::uuid
  AND member_ids ? sqlc.arg('user_id')::text;

-- name: ListMembersWithUser :many
SELECT m.id, m.workspace_id, m.user_id, m.role, m.created_at,
       u.name as user_name, u.email as user_email, u.avatar_url as user_avatar_url
FROM member m
JOIN "user" u ON u.id = m.user_id
WHERE m.workspace_id = $1
ORDER BY m.created_at ASC;
