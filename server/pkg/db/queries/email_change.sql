-- name: CreateEmailChangeVerificationCode :exec
INSERT INTO verification_code (email, code, expires_at, purpose, requester_user_id)
VALUES ($1, $2, $3, 'email_change', $4);

-- name: GetLatestEmailChangeRequest :one
SELECT created_at FROM verification_code
WHERE purpose = 'email_change' AND requester_user_id = $1
ORDER BY created_at DESC LIMIT 1;

-- name: GetEmailChangeVerificationCode :one
SELECT * FROM verification_code
WHERE purpose = 'email_change' AND requester_user_id = $1 AND email = $2
  AND used = FALSE AND expires_at > now() AND attempts < 5
ORDER BY created_at DESC LIMIT 1
FOR UPDATE;

-- name: UpdateUserEmail :one
UPDATE "user" SET email = $2, updated_at = now() WHERE id = $1 RETURNING *;
