-- name: CreateDeviceAuthorization :one
INSERT INTO device_authorization (
    client_name,
    device_code_hash,
    user_code_hash,
    expires_at,
    interval_seconds
)
VALUES ($1, $2, $3, $4, $5)
RETURNING *;

-- name: GetDeviceAuthorizationByDeviceCodeHashForUpdate :one
SELECT * FROM device_authorization
WHERE device_code_hash = $1
FOR UPDATE;

-- name: GetDeviceAuthorizationByDeviceCodeHash :one
SELECT * FROM device_authorization
WHERE device_code_hash = $1;

-- name: GetDeviceAuthorizationByUserCodeHash :one
SELECT * FROM device_authorization
WHERE user_code_hash = $1;

-- name: RecordDeviceAuthorizationPoll :one
UPDATE device_authorization
SET poll_count = poll_count + 1,
    last_polled_at = now()
WHERE id = $1
RETURNING *;

-- name: ApproveDeviceAuthorization :one
UPDATE device_authorization
SET status = 'approved',
    user_id = $2,
    approved_at = now()
WHERE id = $1
  AND status = 'pending'
  AND expires_at > now()
  AND poll_count < $3
RETURNING *;

-- name: DenyDeviceAuthorization :one
UPDATE device_authorization
SET status = 'denied',
    denied_at = now()
WHERE id = $1
  AND status = 'pending'
  AND expires_at > now()
  AND poll_count < $2
RETURNING *;

-- name: ConsumeDeviceAuthorization :one
UPDATE device_authorization
SET status = 'consumed',
    consumed_at = now()
WHERE id = $1
  AND status = 'approved'
  AND user_id = $2
  AND expires_at > now()
RETURNING *;

-- name: MarkDeviceAuthorizationExpired :one
UPDATE device_authorization
SET status = 'expired'
WHERE id = $1
  AND status IN ('pending', 'approved')
RETURNING *;
