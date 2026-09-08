-- name: LockTerminalReportTask :one
SELECT * FROM agent_task_queue WHERE id = $1 FOR UPDATE;

-- name: LockTerminalReportIssue :exec
-- Do not wait on an issue while holding its task: issue deletion acquires
-- those locks in the reverse order. Contention rolls back the report so the
-- durable sender can retry its unchanged identity after the writer commits.
SELECT issue.id
FROM issue JOIN agent_task_queue task ON task.issue_id = issue.id
WHERE task.id = $1
FOR UPDATE OF issue NOWAIT;

-- name: GetTerminalReportReceipt :one
SELECT * FROM terminal_report_receipt WHERE task_id = $1 AND claim_fence = $2;

-- name: CreateTerminalReportReceipt :exec
INSERT INTO terminal_report_receipt (report_id, task_id, claim_fence, payload_sha256, task_status)
VALUES ($1, $2, $3, $4, $5);
