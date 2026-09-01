package handler

import (
	"net/http"
	"strings"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/testutil"
)

func automationChildIssueRequest(t *testing.T, assigneeType, assigneeID, parentIssueID, status, actorAgentID, taskID string) *http.Request {
	t.Helper()

	r := newRequest(http.MethodPost, "/api/issues?workspace_id="+testWorkspaceID, map[string]any{
		"title":           "automation private-assignee child " + t.Name(),
		"status":          status,
		"priority":        "low",
		"executor_type":   assigneeType,
		"executor_id":     assigneeID,
		"parent_issue_id": parentIssueID,
		"allow_duplicate": true,
	})
	if actorAgentID != "" {
		r.Header.Set("X-Agent-ID", actorAgentID)
	}
	if taskID != "" {
		r.Header.Set("X-Task-ID", taskID)
	}
	return r
}

func cleanupAutomationChildIssue(t *testing.T, issueID string) {
	t.Helper()
	dbfx.Cleanup(t, `DELETE FROM issue WHERE id = $1`, issueID)
	dbfx.Cleanup(t, `DELETE FROM agent_task_queue WHERE issue_id = $1`, issueID)
}

func TestCreateIssue_AutomationLeaderAssignsPrivateWorker(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	t.Run("verified lineage parks backlog child without enqueue", func(t *testing.T) {
		workerID, ownerID, _ := privateAgentTestFixture(t)
		fx := newAutomationDelegationFixture(t, workerID, ownerID, "automation")
		parentIssueID := uuidToString(fx.Issue.ID)

		var created IssueResponse
		testutil.Call(t, testHandler.CreateIssue,
			automationChildIssueRequest(t, "agent", workerID, parentIssueID, "backlog", fx.LeaderAgentID, fx.LeaderTaskID),
		).Want(http.StatusCreated).JSON(&created)
		cleanupAutomationChildIssue(t, created.ID)
		if created.ParentIssueID == nil || *created.ParentIssueID != parentIssueID {
			t.Fatalf("created child parent_issue_id = %v, want %q", created.ParentIssueID, parentIssueID)
		}
		if created.ExecutorType == nil || *created.ExecutorType != "agent" || created.ExecutorID == nil || *created.ExecutorID != workerID {
			t.Fatalf("created child assignee = (%v, %v), want (agent, %s)", created.ExecutorType, created.ExecutorID, workerID)
		}

		var queued int
		dbfx.QueryRow(t, `
			SELECT count(*) FROM agent_task_queue
			WHERE issue_id = $1 AND agent_id = $2
		`, created.ID, workerID).Scan(&queued)
		if queued != 0 {
			t.Fatalf("backlog child must not enqueue the private worker, got %d tasks", queued)
		}
	})

	t.Run("verified lineage creates active child and enqueues once", func(t *testing.T) {
		workerID, ownerID, _ := privateAgentTestFixture(t)
		fx := newAutomationDelegationFixture(t, workerID, ownerID, "automation")

		var created IssueResponse
		testutil.Call(t, testHandler.CreateIssue,
			automationChildIssueRequest(t, "agent", workerID, uuidToString(fx.Issue.ID), "todo", fx.LeaderAgentID, fx.LeaderTaskID),
		).Want(http.StatusCreated).JSON(&created)
		cleanupAutomationChildIssue(t, created.ID)

		var taskCount int
		var originatorCount int
		dbfx.QueryRow(t, `
			SELECT count(*), count(originator_user_id) FROM agent_task_queue
			WHERE issue_id = $1 AND agent_id = $2
		`, created.ID, workerID).Scan(&taskCount, &originatorCount)
		if taskCount != 1 {
			t.Fatalf("active child must enqueue the private worker exactly once, got %d tasks", taskCount)
		}
		if originatorCount != 0 {
			t.Fatal("automation creator authority is authorization-only; worker task must remain unattributed")
		}
	})

	t.Run("verified lineage creates team child and enqueues its private leader once", func(t *testing.T) {
		workerID, ownerID, _ := privateAgentTestFixture(t)
		fx := newAutomationDelegationFixture(t, workerID, ownerID, "automation")
		teamID := dbfx.Team(t, "Automation Private Leader Team", workerID)

		var created IssueResponse
		testutil.Call(t, testHandler.CreateIssue,
			automationChildIssueRequest(t, "team", teamID, uuidToString(fx.Issue.ID), "todo", fx.LeaderAgentID, fx.LeaderTaskID),
		).Want(http.StatusCreated).JSON(&created)
		cleanupAutomationChildIssue(t, created.ID)
		if created.ExecutorType == nil || *created.ExecutorType != "team" || created.ExecutorID == nil || *created.ExecutorID != teamID {
			t.Fatalf("created child assignee = (%v, %v), want (team, %s)", created.ExecutorType, created.ExecutorID, teamID)
		}

		var taskCount int
		var originatorCount int
		var teamTaskCount int
		dbfx.QueryRow(t, `
			SELECT count(*), count(originator_user_id), count(*) FILTER (WHERE team_id = $3)
			FROM agent_task_queue
			WHERE issue_id = $1 AND agent_id = $2
		`, created.ID, workerID, teamID).Scan(&taskCount, &originatorCount, &teamTaskCount)
		if taskCount != 1 || teamTaskCount != 1 {
			t.Fatalf("active team child must enqueue its private leader exactly once with team lineage, got %d tasks (%d with team_id)", taskCount, teamTaskCount)
		}
		if originatorCount != 0 {
			t.Fatal("automation creator authority is authorization-only; team leader task must remain unattributed")
		}
	})

	t.Run("creator without invoke rights is denied", func(t *testing.T) {
		workerID, _, plainMemberID := privateAgentTestFixture(t)
		fx := newAutomationDelegationFixture(t, workerID, plainMemberID, "automation")

		testutil.Call(t, testHandler.CreateIssue,
			automationChildIssueRequest(t, "agent", workerID, uuidToString(fx.Issue.ID), "backlog", fx.LeaderAgentID, fx.LeaderTaskID),
		).Want(http.StatusForbidden)
	})

	t.Run("real originator takes precedence over automation creator", func(t *testing.T) {
		workerID, ownerID, plainMemberID := privateAgentTestFixture(t)
		fx := newAutomationDelegationFixture(t, workerID, ownerID, "automation")
		dbfx.Exec(t, `
			UPDATE agent_task_queue
			SET originator_user_id = $1, accountable_user_id = $1, originator_source = 'direct_human'
			WHERE id = $2
		`, plainMemberID, fx.LeaderTaskID)

		testutil.Call(t, testHandler.CreateIssue,
			automationChildIssueRequest(t, "agent", workerID, uuidToString(fx.Issue.ID), "backlog", fx.LeaderAgentID, fx.LeaderTaskID),
		).Want(http.StatusForbidden)
	})

	t.Run("missing task lineage is denied", func(t *testing.T) {
		workerID, ownerID, _ := privateAgentTestFixture(t)
		fx := newAutomationDelegationFixture(t, workerID, ownerID, "automation")

		testutil.Call(t, testHandler.CreateIssue,
			automationChildIssueRequest(t, "agent", workerID, uuidToString(fx.Issue.ID), "backlog", fx.LeaderAgentID, ""),
		).Want(http.StatusForbidden)
	})

	t.Run("task actor mismatch is denied", func(t *testing.T) {
		workerID, ownerID, _ := privateAgentTestFixture(t)
		fx := newAutomationDelegationFixture(t, workerID, ownerID, "automation")
		workerTaskID := seedTaskOnIssue(t, workerID, uuidToString(fx.Issue.ID), fx.RuntimeID)

		testutil.Call(t, testHandler.CreateIssue,
			automationChildIssueRequest(t, "agent", workerID, uuidToString(fx.Issue.ID), "backlog", fx.LeaderAgentID, workerTaskID),
		).Want(http.StatusForbidden)
	})

	t.Run("task bound to another issue is denied", func(t *testing.T) {
		workerID, ownerID, _ := privateAgentTestFixture(t)
		fx := newAutomationDelegationFixture(t, workerID, ownerID, "automation")
		otherIssueID := seedBareIssue(t, fx.LeaderAgentID)
		otherTaskID := seedTaskOnIssue(t, fx.LeaderAgentID, otherIssueID, fx.RuntimeID)

		testutil.Call(t, testHandler.CreateIssue,
			automationChildIssueRequest(t, "agent", workerID, uuidToString(fx.Issue.ID), "backlog", fx.LeaderAgentID, otherTaskID),
		).Want(http.StatusForbidden)
	})

	t.Run("cross-workspace parent is rejected before assignee authorization", func(t *testing.T) {
		workerID, ownerID, _ := privateAgentTestFixture(t)
		fx := newAutomationDelegationFixture(t, workerID, ownerID, "automation")
		teamID := dbfx.Team(t, "Automation Cross-Workspace Team", workerID)
		foreignWorkspaceID := dbfx.Workspace(t, "Automation Foreign Parent", "automation-foreign-parent-"+workerID[:8])
		foreignParentID := dbfx.Issue(t, "Automation foreign parent", testutil.Cols{
			"workspace_id": foreignWorkspaceID,
			"number":       1,
		})

		for _, target := range []struct {
			name, assigneeType, assigneeID string
		}{
			{name: "agent", assigneeType: "agent", assigneeID: workerID},
			{name: "team", assigneeType: "team", assigneeID: teamID},
		} {
			t.Run(target.name, func(t *testing.T) {
				resp := testutil.Call(t, testHandler.CreateIssue,
					automationChildIssueRequest(t, target.assigneeType, target.assigneeID, foreignParentID, "backlog", fx.LeaderAgentID, fx.LeaderTaskID),
				).Want(http.StatusBadRequest)
				if !strings.Contains(resp.Text(), "parent issue not found in this workspace") {
					t.Fatalf("cross-workspace parent rejection = %q, want workspace boundary error", resp.Text())
				}

				title := "automation private-assignee child " + t.Name()
				if count := dbfx.Count(t, `SELECT count(*) FROM issue WHERE workspace_id = $1 AND title = $2`, testWorkspaceID, title); count != 0 {
					t.Fatalf("cross-workspace parent rejection created %d issue rows", count)
				}
			})
		}
	})

	t.Run("non-automation parent is denied", func(t *testing.T) {
		workerID, ownerID, _ := privateAgentTestFixture(t)
		fx := newAutomationDelegationFixture(t, workerID, ownerID, "")

		testutil.Call(t, testHandler.CreateIssue,
			automationChildIssueRequest(t, "agent", workerID, uuidToString(fx.Issue.ID), "backlog", fx.LeaderAgentID, fx.LeaderTaskID),
		).Want(http.StatusForbidden)
	})
}
