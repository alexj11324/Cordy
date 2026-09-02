-- Guest identity surface for W9.

-- name: GetGuestSessionByID :one
SELECT id, user_id, token_hash, status, created_at, claimed_at, claimed_by FROM guest_session WHERE id = $1 LIMIT 1;

-- name: GetGuestSessionByTokenHash :one
SELECT id, user_id, token_hash, status, created_at, claimed_at, claimed_by FROM guest_session WHERE token_hash = $1 LIMIT 1;

-- name: CreateGuestSession :one
INSERT INTO guest_session (user_id, token_hash, status, id)
VALUES ($1, $2, COALESCE(sqlc.narg('status')::text, 'active'), COALESCE(sqlc.narg('id')::uuid, gen_random_uuid()))
RETURNING id, user_id, token_hash, status, created_at, claimed_at, claimed_by;

-- name: ClaimGuestSession :one
UPDATE guest_session SET status = 'claimed', claimed_at = now(), claimed_by = $2
WHERE id = $1 AND status = 'active'
RETURNING id, user_id, token_hash, status, created_at, claimed_at, claimed_by;

-- name: RevokeGuestSession :one
UPDATE guest_session SET status = 'revoked' WHERE id = $1 AND status = 'active' RETURNING id, user_id, token_hash, status, created_at, claimed_at, claimed_by;
