-- name: ListWorkspaceAutomationRuns :many
SELECT sqlc.embed(r), a.title AS automation_title, a.executor_id
FROM automation_run r
JOIN automation a ON a.id = r.automation_id
WHERE a.workspace_id = sqlc.arg(workspace_id)
  AND (NOT sqlc.arg(mine)::boolean OR (a.created_by_type = 'member' AND a.created_by_id = sqlc.arg(user_id)))
  AND (a.title ILIKE '%' || sqlc.arg(search)::text || '%' AND (cardinality(sqlc.arg(statuses)::text[]) = 0 OR r.status = ANY(sqlc.arg(statuses)::text[])))
ORDER BY r.triggered_at DESC, r.id DESC
LIMIT sqlc.arg(page_limit) OFFSET sqlc.arg(page_offset);

-- name: WorkspaceAutomationRunSummary :one
SELECT count(*) FILTER (WHERE (a.title ILIKE '%' || sqlc.arg(search)::text || '%' AND (cardinality(sqlc.arg(statuses)::text[]) = 0 OR r.status = ANY(sqlc.arg(statuses)::text[]))))::bigint AS total,
  count(*) FILTER (WHERE r.status = 'completed' AND r.triggered_at >= now() - interval '24 hours')::bigint AS successful_24h,
  count(*) FILTER (WHERE r.status = 'failed' AND r.triggered_at >= now() - interval '24 hours')::bigint AS failed_24h,
  count(*) FILTER (WHERE r.status = 'completed' AND r.triggered_at >= now() - interval '7 days')::bigint AS successful_7d,
  count(*) FILTER (WHERE r.status = 'failed' AND r.triggered_at >= now() - interval '7 days')::bigint AS failed_7d
FROM automation_run r
JOIN automation a ON a.id = r.automation_id
WHERE a.workspace_id = sqlc.arg(workspace_id)
  AND (NOT sqlc.arg(mine)::boolean OR (a.created_by_type = 'member' AND a.created_by_id = sqlc.arg(user_id)));
