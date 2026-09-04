package handler

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// TestCreateComment_TeamMentionStampsTeamIDOnLeaderTask locks the enqueue
// side of the MUL-3730 fix: when a comment @mentions a team, the leader task
// it enqueues must carry team_id on the task row, so the daemon claim handler
// can locate the team and inject the briefing (keyed off is_leader_task +
// team_id, not issue assignee). The issue here is NOT assigned to the team —
// exactly the comment-mention path that the old issue-executor gate missed.
func TestCreateComment_TeamMentionStampsTeamIDOnLeaderTask(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()

	var leaderID string
	if err := testPool.QueryRow(ctx, `
		SELECT id FROM agent WHERE workspace_id = $1 ORDER BY created_at ASC LIMIT 1
	`, testWorkspaceID).Scan(&leaderID); err != nil {
		t.Fatalf("load leader agent: %v", err)
	}

	var teamID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO team (workspace_id, name, description, leader_id, creator_id)
		VALUES ($1, 'Team ID Stamp Team', '', $2, $3)
		RETURNING id
	`, testWorkspaceID, leaderID, testUserID).Scan(&teamID); err != nil {
		t.Fatalf("create team: %v", err)
	}
	t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM team WHERE id = $1`, teamID) })

	// Issue assigned to nobody (definitely not the team) — the leader task is
	// produced purely by the @team comment mention.
	var issueID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO issue (workspace_id, creator_type, creator_id, title)
		VALUES ($1, 'member', $2, 'team_id stamp test')
		RETURNING id
	`, testWorkspaceID, testUserID).Scan(&issueID); err != nil {
		t.Fatalf("create issue: %v", err)
	}
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM agent_task_queue WHERE issue_id = $1`, issueID)
		testPool.Exec(context.Background(), `DELETE FROM comment WHERE issue_id = $1`, issueID)
		testPool.Exec(context.Background(), `DELETE FROM issue WHERE id = $1`, issueID)
	})

	w := httptest.NewRecorder()
	r := newRequest("POST", "/api/issues/"+issueID+"/comments", map[string]any{
		"content": "[@Team](mention://team/" + teamID + ") please handle this",
	})
	r = withURLParam(r, "id", issueID)
	testHandler.CreateComment(w, r)
	if w.Code != http.StatusCreated {
		t.Fatalf("CreateComment: expected 201, got %d: %s", w.Code, w.Body.String())
	}

	// The leader task must be queued AND carry team_id = teamID, with
	// is_leader_task = true.
	var gotTeamID string
	var isLeader bool
	if err := testPool.QueryRow(ctx, `
		SELECT team_id::text, is_leader_task
		FROM agent_task_queue
		WHERE issue_id = $1 AND agent_id = $2 AND status = 'queued'
	`, issueID, leaderID).Scan(&gotTeamID, &isLeader); err != nil {
		t.Fatalf("load leader task: %v", err)
	}
	if gotTeamID != teamID {
		t.Fatalf("leader task team_id = %q, want %q", gotTeamID, teamID)
	}
	if !isLeader {
		t.Fatalf("leader task is_leader_task = false, want true")
	}
}

// TestCreateRetryTask_InheritsTeamID locks the retry-clone contract for the
// MUL-3730 fix: a retried leader task must inherit team_id from its parent so
// the team-leader briefing keeps being injected across retries. Parallels
// TestCreateRetryTask_InheritsIsLeaderTask.
func TestCreateRetryTask_InheritsTeamID(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	fx := newTeamCommentTriggerFixture(t)
	issueID := uuidToString(fx.Issue.ID)

	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM agent_task_queue WHERE issue_id = $1`, issueID)
	})

	var runtimeID string
	if err := testPool.QueryRow(ctx, `SELECT runtime_id FROM agent WHERE id = $1`, fx.LeaderID).Scan(&runtimeID); err != nil {
		t.Fatalf("load runtime: %v", err)
	}

	var parentID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO agent_task_queue (agent_id, runtime_id, issue_id, status, attempt, max_attempts, is_leader_task, team_id)
		VALUES ($1, $2, $3, 'failed', 1, 3, TRUE, $4)
		RETURNING id
	`, fx.LeaderID, runtimeID, issueID, fx.TeamID).Scan(&parentID); err != nil {
		t.Fatalf("seed parent task: %v", err)
	}
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM agent_task_queue WHERE id = $1 OR parent_task_id = $1`, parentID)
	})

	child, err := testHandler.Queries.CreateRetryTask(ctx, db.CreateRetryTaskParams{ID: util.MustParseUUID(parentID)})
	if err != nil {
		t.Fatalf("CreateRetryTask: %v", err)
	}
	if !child.TeamID.Valid || util.UUIDToString(child.TeamID) != fx.TeamID {
		t.Fatalf("child.TeamID = %v (valid=%v), want %s", util.UUIDToString(child.TeamID), child.TeamID.Valid, fx.TeamID)
	}
	if !child.IsLeaderTask {
		t.Fatalf("child.IsLeaderTask = false, want true (provenance must survive retry)")
	}
}
