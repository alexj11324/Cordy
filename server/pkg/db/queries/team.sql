-- name: CreateTeam :one
INSERT INTO team (workspace_id, name, description, leader_id, creator_id, avatar_url)
VALUES ($1, $2, $3, $4, $5, $6)
RETURNING *;

-- name: GetTeam :one
SELECT * FROM team WHERE id = $1;

-- name: GetTeamInWorkspace :one
SELECT * FROM team WHERE id = $1 AND workspace_id = $2;

-- name: LockTeamForAutopilotAssignment :one
-- Stabilizes the team-to-leader resolution while an active Autopilot is
-- created, retargeted, or resumed. FOR SHARE conflicts with an ordinary
-- leader_id update, so the caller subsequently locks the same leader Agent
-- whose row Runtime teardown serializes against.
SELECT * FROM team
WHERE id = $1 AND workspace_id = $2
FOR SHARE;

-- name: LockTeamForUpdate :one
-- Team leader changes take the exclusive side of the same lock used by
-- Autopilot assignment. The handler then locks the proposed leader Agent and
-- pauses active team Autopilots when that Agent is unbound.
SELECT * FROM team
WHERE id = $1 AND workspace_id = $2
FOR UPDATE;

-- name: ListTeams :many
SELECT * FROM team WHERE workspace_id = $1 AND archived_at IS NULL ORDER BY created_at ASC;

-- name: ListTeamMemberPreviewRows :many
-- Static team membership summary for list/hover previews. This deliberately
-- excludes derived runtime/task status; the team detail members-status
-- endpoint owns live state.
SELECT
    sm.team_id,
    sm.member_type,
    sm.member_id,
    sm.role
FROM team_member sm
JOIN team s ON s.id = sm.team_id
WHERE s.workspace_id = $1 AND s.archived_at IS NULL
ORDER BY
    sm.team_id ASC,
    (sm.member_type = 'agent' AND sm.member_id = s.leader_id) DESC,
    sm.created_at ASC;

-- name: ListTeamMemberPreviewRowsByTeam :many
SELECT
    sm.team_id,
    sm.member_type,
    sm.member_id,
    sm.role
FROM team_member sm
JOIN team s ON s.id = sm.team_id
WHERE sm.team_id = $1
ORDER BY
    (sm.member_type = 'agent' AND sm.member_id = s.leader_id) DESC,
    sm.created_at ASC;

-- name: ListAllTeams :many
SELECT * FROM team WHERE workspace_id = $1 ORDER BY created_at ASC;

-- name: UpdateTeam :one
UPDATE team SET
    name = COALESCE(sqlc.narg('name'), name),
    description = COALESCE(sqlc.narg('description'), description),
    leader_id = COALESCE(sqlc.narg('leader_id'), leader_id),
    avatar_url = COALESCE(sqlc.narg('avatar_url'), avatar_url),
    instructions = COALESCE(sqlc.narg('instructions'), instructions),
    updated_at = now()
WHERE id = $1
RETURNING *;

-- name: ArchiveTeam :one
UPDATE team SET archived_at = now(), archived_by = $2, updated_at = now()
WHERE id = $1
RETURNING *;

-- name: AddTeamMember :one
INSERT INTO team_member (team_id, member_type, member_id, role)
VALUES ($1, $2, $3, $4)
RETURNING *;

-- name: RemoveTeamMember :execrows
DELETE FROM team_member
WHERE team_id = $1 AND member_type = $2 AND member_id = $3;

-- name: ListTeamMembers :many
SELECT * FROM team_member WHERE team_id = $1 ORDER BY created_at ASC;

-- name: UpdateTeamMemberRole :one
UPDATE team_member SET role = $4
WHERE team_id = $1 AND member_type = $2 AND member_id = $3
RETURNING *;

-- name: IsTeamMember :one
SELECT EXISTS(
    SELECT 1 FROM team_member
    WHERE team_id = $1 AND member_type = $2 AND member_id = $3
) AS is_member;

-- name: CountTeamMembers :one
SELECT count(*) FROM team_member WHERE team_id = $1;

-- name: GetTeamByAssignee :one
-- Look up the team when an issue is assigned to a team.
SELECT s.* FROM team s WHERE s.id = $1 AND s.workspace_id = $2;

-- name: ListTeamsByMember :many
-- Find all teams a given entity belongs to in a workspace.
SELECT s.* FROM team s
JOIN team_member sm ON sm.team_id = s.id
WHERE s.workspace_id = $1 AND sm.member_type = $2 AND sm.member_id = $3
ORDER BY s.created_at ASC;

-- name: TransferTeamAssignees :exec
-- Transfer all issues assigned to a team to the team's leader agent.
UPDATE issue SET assignee_type = 'agent', assignee_id = $2, revision = revision + 1, updated_at = now()
WHERE assignee_type = 'team' AND assignee_id = $1;

-- name: TransferTeamAutopilotsToLeader :exec
-- Mirrors TransferTeamAssignees for autopilot rows: when a team is archived,
-- any autopilot still pointing at the team would otherwise dangle and the
-- admission gate would skip every subsequent dispatch with "assignee team
-- cannot be resolved". Rewrite the assignee in place to the leader agent so
-- the autopilot keeps firing under the same leader-only execution semantics
-- it had a moment before the archive (Path A from PB-2429).
UPDATE autopilot
SET assignee_type = 'agent',
    assignee_id = $2,
    updated_at = now()
WHERE assignee_type = 'team' AND assignee_id = $1;

-- name: ListTeamMemberStatusRows :many
-- Per-row join used to build the team-members status view. One row per
-- (team_member × in_flight_task); members with no in-flight task return a
-- single row with NULL task_* columns. Human members and agent members
-- with no agent row also return one row with NULL agent_/runtime_ columns.
-- waiting_local_directory stays in the row set so its issue remains visible,
-- but the handler only treats dispatched/running rows as working because the
-- team status vocabulary has no queued bucket.
SELECT
    sm.id              AS team_member_id,
    sm.member_type     AS member_type,
    sm.member_id       AS member_id,
    a.archived_at      AS agent_archived_at,
    ar.status          AS runtime_status,
    ar.last_seen_at    AS runtime_last_seen_at,
    atq.id             AS task_id,
    atq.status         AS task_status,
    atq.issue_id       AS task_issue_id,
    atq.dispatched_at  AS task_dispatched_at,
    i.number           AS issue_number,
    i.title            AS issue_title,
    i.status           AS issue_status
FROM team_member sm
LEFT JOIN agent a
       ON sm.member_type = 'agent' AND a.id = sm.member_id
LEFT JOIN agent_runtime ar
       ON ar.id = a.runtime_id
LEFT JOIN agent_task_queue atq
       ON sm.member_type = 'agent'
      AND atq.agent_id = sm.member_id
      AND atq.status IN ('dispatched', 'running', 'waiting_local_directory')
LEFT JOIN issue i
       ON i.id = atq.issue_id
WHERE sm.team_id = $1
ORDER BY sm.created_at ASC, atq.dispatched_at DESC NULLS LAST;
