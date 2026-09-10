-- name: ListProjects :many
SELECT * FROM project
WHERE workspace_id = $1
  AND (sqlc.narg('status')::text IS NULL OR status = sqlc.narg('status'))
  AND (sqlc.narg('priority')::text IS NULL OR priority = sqlc.narg('priority'))
ORDER BY created_at DESC;

-- name: GetProjectInWorkspace :one
SELECT * FROM project
WHERE id = $1 AND workspace_id = $2;

-- name: LockProjectForChatSessionCreate :one
-- Conflicts with project deletion so a chat session cannot commit a soft
-- project reference after the delete transaction has swept existing sessions.
SELECT id FROM project
WHERE id = $1 AND workspace_id = $2
FOR KEY SHARE;

-- name: LockProjectForDelete :one
-- Serializes project deletion with chat-session creation. The handler locks,
-- clears every soft chat reference, and deletes the project in one transaction.
SELECT id FROM project
WHERE id = $1 AND workspace_id = $2
FOR UPDATE;

-- name: CreateProject :one
INSERT INTO project (
    workspace_id, title, summary, description, icon, status,
    lead_type, lead_id, priority, start_date, due_date,
    member_ids, label_ids, dependency_ids, milestones
) VALUES (
    $1, $2, sqlc.narg('summary'), $3, $4, $5, $6, $7, $8, $9, $10,
    COALESCE(sqlc.arg('member_ids')::jsonb, '[]'::jsonb),
    COALESCE(sqlc.arg('label_ids')::jsonb, '[]'::jsonb),
    COALESCE(sqlc.arg('dependency_ids')::jsonb, '[]'::jsonb),
    COALESCE(sqlc.arg('milestones')::jsonb, '[]'::jsonb)
) RETURNING *;

-- name: UpdateProject :one
UPDATE project SET
    title = COALESCE(sqlc.narg('title'), title),
    summary = sqlc.narg('summary'),
    description = sqlc.narg('description'),
    icon = sqlc.narg('icon'),
    status = COALESCE(sqlc.narg('status'), status),
    priority = COALESCE(sqlc.narg('priority'), priority),
    lead_type = sqlc.narg('lead_type'),
    lead_id = sqlc.narg('lead_id'),
    start_date = sqlc.narg('start_date'),
    due_date = sqlc.narg('due_date'),
    member_ids = COALESCE(sqlc.narg('member_ids')::jsonb, member_ids),
    label_ids = COALESCE(sqlc.narg('label_ids')::jsonb, label_ids),
    dependency_ids = COALESCE(sqlc.narg('dependency_ids')::jsonb, dependency_ids),
    milestones = COALESCE(sqlc.narg('milestones')::jsonb, milestones),
    updated_at = now()
WHERE id = $1
RETURNING *;

-- name: DeleteProject :exec
-- Defense-in-depth: workspace_id is a SQL-layer tenant guard. See DeleteIssue.
-- Keep project references application-owned instead of relying on the project's
-- historical ON DELETE SET NULL constraints. These updates are deliberately
-- scoped by both project and workspace and run in the same transaction as the
-- project-resource cleanup and project deletion.
WITH cleared_issue_projects AS (
    UPDATE issue
    SET project_id = NULL
    WHERE project_id = $1 AND workspace_id = $2
    RETURNING id
), cleared_automation_projects AS (
    UPDATE automation
    SET project_id = NULL
    WHERE project_id = $1 AND workspace_id = $2
    RETURNING id
), cleared_inbound_project_dependencies AS (
    UPDATE project AS dependent
    SET dependency_ids = dependent.dependency_ids - $1::text,
        updated_at = now()
    WHERE dependent.workspace_id = $2
      AND dependent.id <> $1
      AND dependent.dependency_ids ? $1::text
    RETURNING id
), deleted_resources AS (
    DELETE FROM project_resource
    WHERE project_id = $1 AND workspace_id = $2
    RETURNING id
)
DELETE FROM project AS p
WHERE p.id = $1
  AND p.workspace_id = $2
  AND (SELECT count(*) FROM cleared_issue_projects) >= 0
  AND (SELECT count(*) FROM cleared_automation_projects) >= 0
  AND (SELECT count(*) FROM cleared_inbound_project_dependencies) >= 0
  AND (SELECT count(*) FROM deleted_resources) >= 0;

-- name: CountIssuesByProject :one
SELECT count(*) FROM issue
WHERE project_id = $1;

-- name: GetProjectIssueStats :many
SELECT project_id,
       count(*)::bigint AS total_count,
       count(*) FILTER (WHERE status = ANY(sqlc.arg('terminal_status_keys')::text[]))::bigint AS done_count
FROM issue
WHERE workspace_id = sqlc.arg('workspace_id')::uuid
  AND project_id = ANY(sqlc.arg('project_ids')::uuid[])
GROUP BY project_id;

-- The metadata arrays are application-owned references. These lookup queries
-- keep validation workspace-scoped without introducing foreign keys.

-- name: ListProjectMetadataMemberUsers :many
SELECT user_id
FROM member
WHERE workspace_id = sqlc.arg('workspace_id')::uuid
  AND user_id = ANY(sqlc.arg('user_ids')::uuid[])
ORDER BY user_id;

-- name: ListProjectMetadataLabels :many
SELECT id
FROM issue_label
WHERE workspace_id = sqlc.arg('workspace_id')::uuid
  AND resource_type = 'project'
  AND id = ANY(sqlc.arg('label_ids')::uuid[])
ORDER BY id;

-- name: ListProjectMetadataDependencies :many
SELECT id
FROM project
WHERE workspace_id = sqlc.arg('workspace_id')::uuid
  AND id = ANY(sqlc.arg('project_ids')::uuid[])
ORDER BY id;

-- name: ClearProjectLabelReference :exec
-- Labels are catalog rows, while project.label_ids is an application-owned
-- reference list. Clear the deleted label in the same transaction as catalog
-- deletion so projects cannot retain an unusable ID.
UPDATE project
SET label_ids = label_ids - sqlc.arg('label_id')::text,
    updated_at = now()
WHERE workspace_id = sqlc.arg('workspace_id')::uuid
  AND label_ids ? sqlc.arg('label_id')::text;

-- name: ProjectMetadataDependencyCycle :one
-- dependency_ids point from a project to its prerequisites. Replace the source
-- row in the graph with the candidate list and walk transitively; reaching the
-- source again means the proposed write creates a cycle.
WITH RECURSIVE project_edges AS (
    SELECT p.id AS project_id, edge.dependency_id
    FROM project p
    CROSS JOIN LATERAL jsonb_array_elements_text(
        CASE
            WHEN p.id = sqlc.arg('source_id')::uuid
                THEN sqlc.arg('dependency_ids')::jsonb
            ELSE p.dependency_ids
        END
    ) AS edge(dependency_id)
    WHERE p.workspace_id = sqlc.arg('workspace_id')::uuid
), reachable(project_id, dependency_id) AS (
    SELECT project_id, dependency_id
    FROM project_edges
    UNION
    SELECT reachable.project_id, edge.dependency_id
    FROM reachable
    JOIN project_edges edge
      ON edge.project_id = reachable.dependency_id::uuid
)
SELECT EXISTS (
    SELECT 1
    FROM reachable
    WHERE project_id = sqlc.arg('source_id')::uuid
      AND dependency_id = sqlc.arg('source_id')::text
) AS creates_cycle;
