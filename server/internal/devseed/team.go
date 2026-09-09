package devseed

import (
	"context"
	"fmt"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"
)

type memberFixture struct{ key, name, legacyName, role, avatarURL string }

// Names and photo URLs match ReUI data-grid-base-1's original sample people.
var teamMembers = []memberFixture{
	{"lin", "Alex Johnson", "林悦 · 示例", "产品设计", "https://images.unsplash.com/photo-1535713875002-d1d0cf377fde?w=96&h=96&dpr=2&q=80"},
	{"chen", "Aron Thompson", "陈宇 · 示例", "前端开发", "https://images.unsplash.com/photo-1527980965255-d3b416303d12?w=96&h=96&dpr=2&q=80"},
	{"zhou", "David Kim", "周宁 · 示例", "后端开发", "https://images.unsplash.com/photo-1607990281513-2c110a25bd8c?w=96&h=96&dpr=2&q=80"},
	{"xu", "Emma Wilson", "许言 · 示例", "测试验收", "https://images.unsplash.com/photo-1485893086445-ed75865251e0?w=96&h=96&dpr=2&q=80"},
}

func ensureMembers(ctx context.Context, tx pgx.Tx, workspaceID pgtype.UUID) error {
	for _, member := range teamMembers {
		userID := pgUUID(fixtureID("user/" + member.key))
		if _, err := tx.Exec(ctx, `INSERT INTO "user" (id,name,email,avatar_url) VALUES ($1,$2,$3,$4) ON CONFLICT (id) DO NOTHING`, userID, member.name, "seed-"+member.key+"@dev-fixtures.invalid", member.avatarURL); err != nil {
			return fmt.Errorf("seed member user: %w", err)
		}
		// Upgrade only the exact original fixture profile; preserve user edits.
		if _, err := tx.Exec(ctx, `UPDATE "user" SET name=$2,avatar_url=$3
 WHERE id=$1 AND name=$4 AND COALESCE(avatar_url,'')='' AND email=$5`, userID, member.name, member.avatarURL, member.legacyName, "seed-"+member.key+"@dev-fixtures.invalid"); err != nil {
			return fmt.Errorf("upgrade seed member profile: %w", err)
		}
		if _, err := tx.Exec(ctx, `INSERT INTO member (id,workspace_id,user_id,role) VALUES ($1,$2,$3,'member') ON CONFLICT DO NOTHING`, pgUUID(fixtureID("member/"+member.key)), workspaceID, userID); err != nil {
			return fmt.Errorf("seed member: %w", err)
		}
	}
	return nil
}

// SeedTeam adds collaborators and assigns fixture work to real, online preset
// agents. It never inserts execution queue rows or pretends a model is running.
// Call after SeedRuntimeAgents. Human fixtures have no credentials or invitations.
func SeedTeam(ctx context.Context, pool *pgxpool.Pool, developerEmail string) error {
	if developerEmail == "" {
		developerEmail = DefaultDeveloperEmail
	}
	tx, err := pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer tx.Rollback(ctx) //nolint:errcheck
	workspaceID := pgUUID(fixtureID("workspace"))
	var developerID pgtype.UUID
	if err := tx.QueryRow(ctx, `SELECT u.id FROM "user" u JOIN member m ON m.user_id=u.id WHERE u.email=$1 AND m.workspace_id=$2`, developerEmail, workspaceID).Scan(&developerID); err != nil {
		return err
	}
	rows, err := tx.Query(ctx, `SELECT a.id, r.id::text FROM agent a JOIN agent_runtime r ON r.id=a.runtime_id WHERE a.workspace_id=$1 AND a.archived_at IS NULL AND r.status='online' ORDER BY a.id`, workspaceID)
	if err != nil {
		return err
	}
	var agents []pgtype.UUID
	for rows.Next() {
		var id pgtype.UUID
		var runtimeID string
		if err := rows.Scan(&id, &runtimeID); err != nil {
			rows.Close()
			return err
		}
		if id == pgUUID(fixtureID("agent/runtime/"+runtimeID)) {
			agents = append(agents, id)
		}
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return err
	}
	if len(agents) == 0 {
		return tx.Commit(ctx)
	}
	teamID := pgUUID(fixtureID("team/product"))
	if _, err := tx.Exec(ctx, `INSERT INTO team (id,workspace_id,name,description,leader_id,creator_id) VALUES ($1,$2,'产品研发','用于查看多人协作和任务分工的开发样例；任务状态为示例，Agent 在线状态来自真实设备。',$3,$4) ON CONFLICT (id) DO NOTHING`, teamID, workspaceID, agents[0], developerID); err != nil {
		return err
	}
	if _, err := tx.Exec(ctx, `UPDATE team SET name='产品研发' WHERE id=$1 AND workspace_id=$2 AND name='产品研发 · 示例' AND creator_id=$3 AND description='用于查看多人协作和任务分工的开发样例；任务状态为示例，Agent 在线状态来自真实设备。'`, teamID, workspaceID, developerID); err != nil {
		return err
	}
	for _, member := range teamMembers {
		if _, err := tx.Exec(ctx, `INSERT INTO team_member (id,team_id,member_type,member_id,role) VALUES ($1,$2,'member',$3,$4) ON CONFLICT DO NOTHING`, pgUUID(fixtureID("team/member/"+member.key)), teamID, pgUUID(fixtureID("user/"+member.key)), member.role); err != nil {
			return err
		}
	}
	for i, agent := range agents {
		role := "开发协作"
		if i == 0 {
			role = "协调与实现"
		}
		if _, err := tx.Exec(ctx, `INSERT INTO team_member (team_id,member_type,member_id,role) VALUES ($1,'agent',$2,$3) ON CONFLICT DO NOTHING`, teamID, agent, role); err != nil {
			return err
		}
	}
	for i, issue := range issueFixtures() {
		owner := pgUUID(fixtureID("user/" + teamMembers[i%len(teamMembers)].key))
		executor := agents[i%len(agents)]
		reviewer := pgUUID(fixtureID("user/xu"))
		// App edits advance revision; exact seed content protects imported edits.
		// Executor, reviewer, and status transition are committed in one write.
		if _, err := tx.Exec(ctx, `UPDATE issue SET owner_id=$3,executor_type='agent',executor_id=$4,
 reviewer_type=CASE WHEN $5='in_review' THEN 'member' ELSE reviewer_type END,
 reviewer_id=CASE WHEN $5='in_review' THEN $9 ELSE reviewer_id END,
 status=$5,metadata=metadata-'demo_awaiting_agent'
 WHERE id=$1 AND workspace_id=$2 AND metadata->>'demo_seed'=$6 AND revision=1
 AND executor_id IS NULL AND owner_id=$7 AND title=$8 AND description=$10
 AND (status=$5 OR (status='todo' AND metadata->>'demo_awaiting_agent'='true'))`,
			pgUUID(fixtureID("issue/"+issue.key)), workspaceID, owner, executor, issue.status, FixtureSet, developerID, issue.title, reviewer, issue.description); err != nil {
			return err
		}
		// Repair the earlier fixture-only review rows; preserve real user handoffs.
		if issue.status == "in_review" {
			if _, err := tx.Exec(ctx, `UPDATE issue SET reviewer_type='member',reviewer_id=$3
 WHERE id=$1 AND workspace_id=$2 AND metadata->>'demo_seed'=$4 AND revision=1
 AND title=$5 AND description=$6 AND status='in_review' AND reviewer_id IS NULL
 AND executor_type='agent' AND executor_id=$7 AND owner_id=$8`,
				pgUUID(fixtureID("issue/"+issue.key)), workspaceID, reviewer, FixtureSet, issue.title, issue.description, executor, owner); err != nil {
				return err
			}
		}
	}

	return tx.Commit(ctx)
}

func initialFixtureStatus(status string) string {
	if status == "in_progress" || status == "in_review" || status == "blocked" {
		return "todo"
	}
	return status
}
