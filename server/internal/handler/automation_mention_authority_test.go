package handler

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// automationDelegationFixture builds the MUL-4857 create_issue scenario: a
// member-created automation creates an issue, and its dispatched leader agent runs
// a task ON that issue and authors an @mention delegation comment whose
// source_task_id points back at that leader task. The authoring run is
// UNATTRIBUTED (originator NULL) exactly as a schedule/webhook automation run is.
//
// This is the shape the authority fallback must recognise — but ONLY through the
// verified lineage of the speaking task (author == task agent, task.issue_id ==
// this issue), never from the issue's automation provenance alone. The fields are
// exposed so negative cases can rewrite the comment's source_task_id to a foreign
// task and prove the fallback then fails closed.
type automationDelegationFixture struct {
	Issue         db.Issue
	LeaderAgentID string // the automation-dispatched agent authoring the comment
	LeaderTaskID  string // its running task on this issue (comment.source_task_id)
	Comment       db.Comment
	AutomationID   string
	RuntimeID     string
}

func newAutomationDelegationFixture(t *testing.T, targetAgentID, automationCreatorUserID, originType string) automationDelegationFixture {
	t.Helper()
	ctx := context.Background()

	runtimeID := handlerTestRuntimeID(t)

	// The seeded workspace agent stands in for the automation-dispatched leader
	// that authors the delegation comment (distinct from the mentioned target).
	var leaderID string
	if err := testPool.QueryRow(ctx, `
		SELECT id FROM agent WHERE workspace_id = $1 ORDER BY created_at ASC LIMIT 1
	`, testWorkspaceID).Scan(&leaderID); err != nil {
		t.Fatalf("load seeded agent: %v", err)
	}

	// A member-created automation; assignee is the target agent (any valid agent
	// satisfies the assignee reference).
	var automationID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO automation (workspace_id, title, executor_id, execution_mode, created_by_type, created_by_id)
		VALUES ($1, 'MUL-4857 delegation', $2, 'create_issue', 'member', $3) RETURNING id
	`, testWorkspaceID, targetAgentID, automationCreatorUserID).Scan(&automationID); err != nil {
		t.Fatalf("create automation: %v", err)
	}
	t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM automation WHERE id = $1`, automationID) })

	// Next per-workspace issue number (default 0 would trip uq_issue_workspace_number).
	var number int
	if err := testPool.QueryRow(ctx, `
		UPDATE workspace
		SET issue_counter = GREATEST(issue_counter, (SELECT COALESCE(MAX(number), 0) FROM issue WHERE workspace_id = $1)) + 1
		WHERE id = $1 RETURNING issue_counter
	`, testWorkspaceID).Scan(&number); err != nil {
		t.Fatalf("next issue number: %v", err)
	}

	// The issue mirrors an automation-created issue (creator is the dispatched
	// leader agent; provenance is origin_type=automation + origin_id). When
	// originType is not "automation" the issue carries no origin, so no creator
	// can be recovered even from a perfectly-lineaged task.
	var originTypeArg, originIDArg any
	if originType == "automation" {
		originTypeArg = "automation"
		originIDArg = automationID
	}
	var issueID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO issue (workspace_id, creator_type, creator_id, title, executor_type, executor_id, number, origin_type, origin_id)
		VALUES ($1, 'agent', $2, 'MUL-4857 delegation issue', 'agent', $2, $3, $4, $5)
		RETURNING id
	`, testWorkspaceID, leaderID, number, originTypeArg, originIDArg).Scan(&issueID); err != nil {
		t.Fatalf("create issue: %v", err)
	}
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM agent_task_queue WHERE issue_id = $1`, issueID)
		testPool.Exec(context.Background(), `DELETE FROM comment WHERE issue_id = $1`, issueID)
		testPool.Exec(context.Background(), `DELETE FROM issue WHERE id = $1`, issueID)
	})

	// The leader's dispatch task, running ON this issue. In create_issue mode the
	// leader task is enqueued through the ordinary issue-assignment path, so it
	// carries NO automation_run_id — the lineage that matters is agent + issue.
	// Unattributed (originator NULL) like a schedule/webhook automation run.
	var leaderTaskID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO agent_task_queue (agent_id, runtime_id, issue_id, status, priority)
		VALUES ($1, $2, $3, 'running', 0) RETURNING id
	`, leaderID, runtimeID, issueID).Scan(&leaderTaskID); err != nil {
		t.Fatalf("create leader task: %v", err)
	}

	// The delegation comment: authored by the leader agent, mentioning the target,
	// with source_task_id pointing back at the leader's running task (the lineage
	// the reconcile/edit path reads).
	var commentID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO comment (workspace_id, issue_id, author_type, author_id, content, source_task_id)
		VALUES ($1, $2, 'agent', $3, $4, $5) RETURNING id
	`, testWorkspaceID, issueID, leaderID, "[@Worker](mention://agent/"+targetAgentID+") please take this", leaderTaskID).Scan(&commentID); err != nil {
		t.Fatalf("create comment: %v", err)
	}

	issue, err := testHandler.Queries.GetIssue(ctx, util.MustParseUUID(issueID))
	if err != nil {
		t.Fatalf("load issue: %v", err)
	}
	comment, err := testHandler.Queries.GetComment(ctx, util.MustParseUUID(commentID))
	if err != nil {
		t.Fatalf("load comment: %v", err)
	}
	return automationDelegationFixture{
		Issue:         issue,
		LeaderAgentID: leaderID,
		LeaderTaskID:  leaderTaskID,
		Comment:       comment,
		AutomationID:   automationID,
		RuntimeID:     runtimeID,
	}
}

// setCommentSourceTask rewrites the fixture comment's source_task_id and reloads
// the row, so a test can point the lineage at a foreign task (or clear it).
func setCommentSourceTask(t *testing.T, fx *automationDelegationFixture, sourceTaskID any) {
	t.Helper()
	if _, err := testPool.Exec(context.Background(),
		`UPDATE comment SET source_task_id = $1 WHERE id = $2`, sourceTaskID, uuidToString(fx.Comment.ID)); err != nil {
		t.Fatalf("rewrite comment source_task_id: %v", err)
	}
	c, err := testHandler.Queries.GetComment(context.Background(), fx.Comment.ID)
	if err != nil {
		t.Fatalf("reload comment: %v", err)
	}
	fx.Comment = c
}

// seedTaskOnIssue inserts a running task for the given agent on the given issue
// and returns its id, for building foreign-lineage negative cases.
func seedTaskOnIssue(t *testing.T, agentID, issueID, runtimeID string) string {
	t.Helper()
	var taskID string
	if err := testPool.QueryRow(context.Background(), `
		INSERT INTO agent_task_queue (agent_id, runtime_id, issue_id, status, priority, context)
		VALUES ($1, $2, $3, 'running', 0, jsonb_build_object('side_chat_parent_task_id', gen_random_uuid()::text)) RETURNING id
	`, agentID, runtimeID, issueID).Scan(&taskID); err != nil {
		t.Fatalf("seed task on issue: %v", err)
	}
	t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM agent_task_queue WHERE id = $1`, taskID) })
	return taskID
}

// TestAutomationDelegationAuthority_LineageBinding is the MUL-4857 fix, guarded by
// the review's confused-deputy finding: an unattributed automation run may borrow
// its automation creator's invoke rights to delegate mid-chain, but ONLY when the
// speaking task's lineage is verified against THIS issue — never from the issue's
// automation provenance plus an empty originator alone.
func TestAutomationDelegationAuthority_LineageBinding(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()

	// agentID: private (default) agent owned by ownerID. plainMemberID: unrelated.
	agentID, ownerID, plainMemberID := privateAgentTestFixture(t)

	// authorityFor resolves the delegation authority the reconcile/edit path uses,
	// straight from the persisted comment's source_task_id lineage.
	authorityFor := func(fx automationDelegationFixture) string {
		return testHandler.automationDelegationAuthorityFromComment(ctx, fx.Issue, fx.Comment)
	}
	// mentionTriggersTarget wires that resolved authority into the trigger compute
	// exactly as the live paths do, and reports whether the private target fires.
	mentionTriggersTarget := func(fx automationDelegationFixture) bool {
		triggers, _ := testHandler.computeCommentAgentTriggers(
			ctx, fx.Issue, fx.Comment.Content, nil, "agent", fx.LeaderAgentID,
			commentTriggerComputeOptions{
				ExcludeTriggerCommentID:            fx.Comment.ID,
				AutomationDelegationAuthorityUserID: authorityFor(fx),
			},
		)
		for _, tr := range triggers {
			if uuidToString(tr.Agent.ID) == agentID {
				return true
			}
		}
		return false
	}

	t.Run("verified lineage + creator owns target -> triggers", func(t *testing.T) {
		fx := newAutomationDelegationFixture(t, agentID, ownerID, "automation")
		if got := authorityFor(fx); got != ownerID {
			t.Fatalf("delegation authority = %q, want automation creator %q", got, ownerID)
		}
		if !mentionTriggersTarget(fx) {
			t.Fatal("expected the private agent to be triggered via the lineage-verified automation-creator authority")
		}
	})

	t.Run("creator cannot invoke target -> still denied", func(t *testing.T) {
		// Lineage is perfect but the creator (plainMemberID) is neither the target's
		// owner nor on any allow-list: the authority resolves but the gate denies.
		fx := newAutomationDelegationFixture(t, agentID, plainMemberID, "automation")
		if got := authorityFor(fx); got != plainMemberID {
			t.Fatalf("delegation authority = %q, want %q", got, plainMemberID)
		}
		if mentionTriggersTarget(fx) {
			t.Fatal("automation creator without invoke rights must not reach a private agent")
		}
	})

	t.Run("non-automation issue -> no authority", func(t *testing.T) {
		fx := newAutomationDelegationFixture(t, agentID, ownerID, "")
		if got := authorityFor(fx); got != "" {
			t.Fatalf("non-automation issue must resolve no authority, got %q", got)
		}
		if mentionTriggersTarget(fx) {
			t.Fatal("a non-automation unattributed run must not invoke a private agent")
		}
	})

	t.Run("missing source task -> no authority", func(t *testing.T) {
		// The previous fix's blind spot: an unattributed comment with no verifiable
		// lineage (source_task_id NULL) must NOT inherit the creator's authority.
		fx := newAutomationDelegationFixture(t, agentID, ownerID, "automation")
		setCommentSourceTask(t, &fx, nil)
		if got := authorityFor(fx); got != "" {
			t.Fatalf("comment without source_task_id must resolve no authority, got %q", got)
		}
		if mentionTriggersTarget(fx) {
			t.Fatal("a comment with no verifiable task lineage must not borrow creator authority")
		}
	})

	t.Run("source task on a different issue -> no authority", func(t *testing.T) {
		// Confused-deputy: a run working on ANOTHER issue comments here. Its task's
		// issue_id != this issue, so it cannot borrow this automation's authority even
		// though its agent authored the comment.
		fx := newAutomationDelegationFixture(t, agentID, ownerID, "automation")
		other := newAutomationDelegationFixture(t, agentID, ownerID, "automation")
		foreignTask := seedTaskOnIssue(t, fx.LeaderAgentID, uuidToString(other.Issue.ID), fx.RuntimeID)
		setCommentSourceTask(t, &fx, foreignTask)
		if got := authorityFor(fx); got != "" {
			t.Fatalf("cross-issue source task must resolve no authority, got %q", got)
		}
		if mentionTriggersTarget(fx) {
			t.Fatal("a task from a different issue must not borrow this automation's creator authority")
		}
	})

	t.Run("author is not the source task's agent -> no authority", func(t *testing.T) {
		// The comment author is the leader, but its source task belongs to a
		// different agent (the target). Author/agent mismatch fails closed.
		fx := newAutomationDelegationFixture(t, agentID, ownerID, "automation")
		mismatchTask := seedTaskOnIssue(t, agentID, uuidToString(fx.Issue.ID), fx.RuntimeID)
		setCommentSourceTask(t, &fx, mismatchTask)
		if got := authorityFor(fx); got != "" {
			t.Fatalf("author != task agent must resolve no authority, got %q", got)
		}
		if mentionTriggersTarget(fx) {
			t.Fatal("a source task owned by a different agent must not confer authority on the comment author")
		}
	})
}

// TestCreateComment_AutomationLeaderMentionEnqueuesPrivateWorker is the MUL-4857
// end-to-end: the automation-dispatched leader posts an @mention delegation on the
// automation-created issue through the real HTTP CreateComment surface (X-Agent-ID
// + X-Task-ID), and the mentioned DEFAULT-private worker is actually enqueued —
// keyed on the automation creator's invoke rights, resolved from the request's
// trusted X-Task-ID lineage. This exercises handler -> comment persistence ->
// trigger -> enqueue, not just the compute function.
func TestCreateComment_AutomationLeaderMentionEnqueuesPrivateWorker(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	// Private worker owned by ownerID; the automation is created by that same owner
	// so the creator legitimately owns the worker.
	workerID, ownerID, _ := privateAgentTestFixture(t)
	fx := newAutomationDelegationFixture(t, workerID, ownerID, "automation")
	issueID := uuidToString(fx.Issue.ID)

	// The leader posts the mention comment in its agent identity. resolveActor
	// trusts the header pair because fx.LeaderTaskID belongs to the leader agent.
	w := httptest.NewRecorder()
	r := newRequest("POST", "/api/issues/"+issueID+"/comments", map[string]any{
		"content": "[@Worker](mention://agent/" + workerID + ") please handle",
	})
	r.Header.Set("X-Agent-ID", fx.LeaderAgentID)
	r.Header.Set("X-Task-ID", fx.LeaderTaskID)
	r = withURLParam(r, "id", issueID)
	testHandler.CreateComment(w, r)
	if w.Code != http.StatusCreated {
		t.Fatalf("leader mention CreateComment: expected 201, got %d: %s", w.Code, w.Body.String())
	}

	var workerTasks int
	if err := testPool.QueryRow(context.Background(), `
		SELECT count(*) FROM agent_task_queue
		WHERE issue_id = $1 AND agent_id = $2 AND status = 'queued'
	`, issueID, workerID).Scan(&workerTasks); err != nil {
		t.Fatalf("count worker tasks: %v", err)
	}
	if workerTasks != 1 {
		t.Fatalf("expected the private worker to be enqueued once via automation-creator authority, got %d queued tasks", workerTasks)
	}

	// The enqueued run must stay UNATTRIBUTED: the creator authority is used for
	// the gate only, never written onto the delegated task's originator (MUL-4302).
	var workerOriginatorValid bool
	if err := testPool.QueryRow(context.Background(), `
		SELECT originator_user_id IS NOT NULL FROM agent_task_queue
		WHERE issue_id = $1 AND agent_id = $2 AND status = 'queued'
	`, issueID, workerID).Scan(&workerOriginatorValid); err != nil {
		t.Fatalf("read worker originator: %v", err)
	}
	if workerOriginatorValid {
		t.Fatal("the delegated worker task must remain unattributed; the creator authority is authorization-only")
	}
}

// nextWorkspaceIssueNumber advances and returns the test workspace's issue
// counter so a directly-inserted issue does not collide on uq_issue_workspace_number.
func nextWorkspaceIssueNumber(t *testing.T) int {
	t.Helper()
	var number int
	if err := testPool.QueryRow(context.Background(), `
		UPDATE workspace
		SET issue_counter = GREATEST(issue_counter, (SELECT COALESCE(MAX(number), 0) FROM issue WHERE workspace_id = $1)) + 1
		WHERE id = $1 RETURNING issue_counter
	`, testWorkspaceID).Scan(&number); err != nil {
		t.Fatalf("next issue number: %v", err)
	}
	return number
}

// seedBareIssue inserts a plain (non-automation) issue authored by the given agent
// and returns its id, for building a cross-issue editing context.
func seedBareIssue(t *testing.T, creatorAgentID string) string {
	t.Helper()
	var issueID string
	if err := testPool.QueryRow(context.Background(), `
		INSERT INTO issue (workspace_id, creator_type, creator_id, title, number)
		VALUES ($1, 'agent', $2, 'MUL-4857 unrelated issue', $3) RETURNING id
	`, testWorkspaceID, creatorAgentID, nextWorkspaceIssueNumber(t)).Scan(&issueID); err != nil {
		t.Fatalf("seed bare issue: %v", err)
	}
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM agent_task_queue WHERE issue_id = $1`, issueID)
		testPool.Exec(context.Background(), `DELETE FROM comment WHERE issue_id = $1`, issueID)
		testPool.Exec(context.Background(), `DELETE FROM issue WHERE id = $1`, issueID)
	})
	return issueID
}

// seedCompletedTaskOnIssueBefore inserts a completed task for the agent on the
// issue with a created_at safely before any comment made during the test, so the
// completion-reconcile pass (ListReconcilableCommentsForIssueSince) picks those
// comments up.
func seedCompletedTaskOnIssueBefore(t *testing.T, agentID, issueID, runtimeID string) string {
	t.Helper()
	var taskID string
	if err := testPool.QueryRow(context.Background(), `
		INSERT INTO agent_task_queue (agent_id, runtime_id, issue_id, status, priority, created_at)
		VALUES ($1, $2, $3, 'completed', 0, now() - interval '1 hour') RETURNING id
	`, agentID, runtimeID, issueID).Scan(&taskID); err != nil {
		t.Fatalf("seed completed task: %v", err)
	}
	t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM agent_task_queue WHERE id = $1`, taskID) })
	return taskID
}

// seedLeaderPlainComment inserts a plain (no-mention) agent comment stamped with
// the given source_task_id, so a test can later edit it to add a mention.
func seedLeaderPlainComment(t *testing.T, issueID, leaderID, sourceTaskID string) string {
	t.Helper()
	var commentID string
	if err := testPool.QueryRow(context.Background(), `
		INSERT INTO comment (workspace_id, issue_id, author_type, author_id, content, source_task_id)
		VALUES ($1, $2, 'agent', $3, 'starting on this', $4) RETURNING id
	`, testWorkspaceID, issueID, leaderID, sourceTaskID).Scan(&commentID); err != nil {
		t.Fatalf("seed plain comment: %v", err)
	}
	return commentID
}

// TestReconcileCommentsOnCompletion_AutomationDelegationRestoresAuthority is the
// MUL-4857 must-fix #1 (review round 2): when the mentioned target was BUSY at
// delegation time, the delegation is deferred to the target's completion
// reconcile. That replay must restore the SAME automation-creator authority from
// the comment's source_task_id — otherwise the unattributed automation chain's
// follow-up is gate-denied again and the delegation is silently lost.
func TestReconcileCommentsOnCompletion_AutomationDelegationRestoresAuthority(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	workerID, ownerID, plainMemberID := privateAgentTestFixture(t)

	// followUps drives the reconcile: an automation delegation comment mentions the
	// busy private worker, the worker's task then completes, and we count the
	// follow-up tasks the completion reconcile enqueues for it.
	followUps := func(t *testing.T, creatorUserID string) (string, int) {
		fx := newAutomationDelegationFixture(t, workerID, creatorUserID, "automation")
		issueID := uuidToString(fx.Issue.ID)
		workerTaskID := seedCompletedTaskOnIssueBefore(t, workerID, issueID, fx.RuntimeID)
		workerTask, err := testHandler.Queries.GetAgentTask(ctx, util.MustParseUUID(workerTaskID))
		if err != nil {
			t.Fatalf("load worker task: %v", err)
		}
		testHandler.reconcileCommentsOnCompletion(ctx, &workerTask)
		var queued int
		if err := testPool.QueryRow(ctx, `
			SELECT count(*) FROM agent_task_queue
			WHERE issue_id = $1 AND agent_id = $2 AND status = 'queued'
		`, issueID, workerID).Scan(&queued); err != nil {
			t.Fatalf("count follow-ups: %v", err)
		}
		return issueID, queued
	}

	t.Run("creator owns busy target -> one unattributed follow-up", func(t *testing.T) {
		issueID, queued := followUps(t, ownerID)
		if queued != 1 {
			t.Fatalf("expected exactly 1 reconcile follow-up for the freed worker, got %d", queued)
		}
		var originatorValid bool
		if err := testPool.QueryRow(ctx, `
			SELECT originator_user_id IS NOT NULL FROM agent_task_queue
			WHERE issue_id = $1 AND agent_id = $2 AND status = 'queued'
		`, issueID, workerID).Scan(&originatorValid); err != nil {
			t.Fatalf("read follow-up originator: %v", err)
		}
		if originatorValid {
			t.Fatal("the reconcile follow-up must stay unattributed; creator authority is authorization-only")
		}
	})

	t.Run("creator without rights -> no follow-up", func(t *testing.T) {
		if _, queued := followUps(t, plainMemberID); queued != 0 {
			t.Fatalf("a creator without invoke rights must not spawn a reconcile follow-up, got %d", queued)
		}
	})
}

// TestUpdateComment_AutomationAuthorityReStampedToEditingTask is the MUL-4857
// must-fix #2 (review round 2): an edit is a NEW action, so it must judge (and
// persist) authority by the CURRENT editing task, not the comment's original
// authoring task. A same-issue edit keeps the automation-creator authority; a
// cross-issue edit re-stamps source_task_id to the EDITING task and still fails
// closed, so it can never borrow the old automation run's authority (preview and
// save now agree). Since MUL-6490 the lineage itself is always persisted — the
// same-issue requirement lives in automationDelegationAuthority, which is what
// rejects the cross-issue editing task here.
func TestUpdateComment_AutomationAuthorityReStampedToEditingTask(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	workerID, ownerID, _ := privateAgentTestFixture(t)

	editAddingMention := func(t *testing.T, editTaskID, commentID, issueID string, fx automationDelegationFixture) {
		w := httptest.NewRecorder()
		r := newRequest(http.MethodPut, "/api/comments/"+commentID, map[string]any{
			"content": "[@Worker](mention://agent/" + workerID + ") please take this",
		})
		r.Header.Set("X-Agent-ID", fx.LeaderAgentID)
		r.Header.Set("X-Task-ID", editTaskID)
		r = withURLParam(r, "commentId", commentID)
		testHandler.UpdateComment(w, r)
		if w.Code != http.StatusOK {
			t.Fatalf("UpdateComment: expected 200, got %d: %s", w.Code, w.Body.String())
		}
	}
	countQueued := func(t *testing.T, issueID string) int {
		var n int
		if err := testPool.QueryRow(ctx, `
			SELECT count(*) FROM agent_task_queue
			WHERE issue_id = $1 AND agent_id = $2 AND status = 'queued'
		`, issueID, workerID).Scan(&n); err != nil {
			t.Fatalf("count queued: %v", err)
		}
		return n
	}

	t.Run("same-issue edit keeps creator authority and triggers", func(t *testing.T) {
		fx := newAutomationDelegationFixture(t, workerID, ownerID, "automation")
		issueID := uuidToString(fx.Issue.ID)
		commentID := seedLeaderPlainComment(t, issueID, fx.LeaderAgentID, fx.LeaderTaskID)
		// Edit from the leader's own task on THIS automation issue.
		editAddingMention(t, fx.LeaderTaskID, commentID, issueID, fx)
		if got := countQueued(t, issueID); got != 1 {
			t.Fatalf("same-issue edit should enqueue the private worker once, got %d", got)
		}
	})

	t.Run("cross-issue edit re-stamps source task to the editing task and fails closed", func(t *testing.T) {
		fx := newAutomationDelegationFixture(t, workerID, ownerID, "automation")
		issueID := uuidToString(fx.Issue.ID)
		commentID := seedLeaderPlainComment(t, issueID, fx.LeaderAgentID, fx.LeaderTaskID)
		// The leader now runs an UNATTRIBUTED task on an unrelated issue and edits
		// its old automation comment from there.
		otherIssueID := seedBareIssue(t, fx.LeaderAgentID)
		crossTaskID := seedTaskOnIssue(t, fx.LeaderAgentID, otherIssueID, fx.RuntimeID)
		editAddingMention(t, crossTaskID, commentID, issueID, fx)
		if got := countQueued(t, issueID); got != 0 {
			t.Fatalf("cross-issue edit must not borrow the old automation authority; got %d queued", got)
		}
		// MUL-6490: the lineage now records the EDITING run (that is what "which
		// run wrote this" means, and it is how a human originator survives a
		// cross-issue hop). The old authoring task must NOT survive the edit —
		// that is the value the automation authority would have accepted.
		var sourceTaskID pgtype.UUID
		if err := testPool.QueryRow(ctx, `SELECT source_task_id FROM comment WHERE id = $1`, commentID).Scan(&sourceTaskID); err != nil {
			t.Fatalf("read comment source_task_id: %v", err)
		}
		if uuidToString(sourceTaskID) != crossTaskID {
			t.Fatalf("source_task_id = %q, want the editing task %q", uuidToString(sourceTaskID), crossTaskID)
		}
	})
}

// TestCreateComment_AutomationWorkerResultWakesTeamLeader locks the review's
// accepted behavior: effectiveInvoker() lets the automation-creator authority reach
// the plain (non-@mention) assigned-team-leader fallback too, so a worker's
// result comment on the automation issue can still wake the private team leader
// and close the leader -> worker -> leader loop under the automation chain.
func TestCreateComment_AutomationWorkerResultWakesTeamLeader(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	runtimeID := handlerTestRuntimeID(t)

	// Private team leader owned by the automation creator; the worker is a distinct
	// seeded agent so the leader self-trigger guard does not apply.
	leaderID, ownerID, _ := privateAgentTestFixture(t)
	var workerID string
	if err := testPool.QueryRow(ctx, `
		SELECT id FROM agent WHERE workspace_id = $1 AND id <> $2 ORDER BY created_at ASC LIMIT 1
	`, testWorkspaceID, leaderID).Scan(&workerID); err != nil {
		t.Fatalf("load worker agent: %v", err)
	}

	var automationID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO automation (workspace_id, title, executor_id, execution_mode, created_by_type, created_by_id)
		VALUES ($1, 'MUL-4857 team', $2, 'create_issue', 'member', $3) RETURNING id
	`, testWorkspaceID, leaderID, ownerID).Scan(&automationID); err != nil {
		t.Fatalf("create automation: %v", err)
	}
	t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM automation WHERE id = $1`, automationID) })

	var teamID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO team (workspace_id, name, description, leader_id, creator_id)
		VALUES ($1, 'MUL-4857 Team', '', $2, $3) RETURNING id
	`, testWorkspaceID, leaderID, ownerID).Scan(&teamID); err != nil {
		t.Fatalf("create team: %v", err)
	}
	t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM team WHERE id = $1`, teamID) })

	var issueID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO issue (workspace_id, creator_type, creator_id, title, executor_type, executor_id, number, origin_type, origin_id)
		VALUES ($1, 'agent', $2, 'MUL-4857 team issue', 'team', $3, $4, 'automation', $5) RETURNING id
	`, testWorkspaceID, leaderID, teamID, nextWorkspaceIssueNumber(t), automationID).Scan(&issueID); err != nil {
		t.Fatalf("create team issue: %v", err)
	}
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM agent_task_queue WHERE issue_id = $1`, issueID)
		testPool.Exec(context.Background(), `DELETE FROM comment WHERE issue_id = $1`, issueID)
		testPool.Exec(context.Background(), `DELETE FROM issue WHERE id = $1`, issueID)
	})

	// The worker is running an unattributed task on this automation issue.
	workerTaskID := seedTaskOnIssue(t, workerID, issueID, runtimeID)

	// The worker posts a PLAIN result comment (no @mention) via HTTP.
	w := httptest.NewRecorder()
	r := newRequest("POST", "/api/issues/"+issueID+"/comments", map[string]any{
		"content": "done — pushed the change",
	})
	r.Header.Set("X-Agent-ID", workerID)
	r.Header.Set("X-Task-ID", workerTaskID)
	r = withURLParam(r, "id", issueID)
	testHandler.CreateComment(w, r)
	if w.Code != http.StatusCreated {
		t.Fatalf("worker result CreateComment: expected 201, got %d: %s", w.Code, w.Body.String())
	}

	var leaderTasks int
	if err := testPool.QueryRow(ctx, `
		SELECT count(*) FROM agent_task_queue
		WHERE issue_id = $1 AND agent_id = $2 AND status = 'queued' AND is_leader_task = TRUE
	`, issueID, leaderID).Scan(&leaderTasks); err != nil {
		t.Fatalf("count leader tasks: %v", err)
	}
	if leaderTasks != 1 {
		t.Fatalf("expected the private team leader to be woken once via automation-creator authority, got %d", leaderTasks)
	}
}

// TestUpdateComment_AdminEditOfAgentCommentClearsStaleLineage is the MUL-4857
// must-fix (review round 3): a workspace admin may EDIT another author's comment,
// but that manage right is NOT an invoke right over the author's private agents
// (canInvokeAgent is deny-by-default for private agents — no admin bypass). When an
// admin edits an automation Agent's comment to add a private @mention while the
// target is busy, the immediate save is blocked on the admin's own member identity,
// AND the persisted source_task_id MUST be cleared. Otherwise the deferred
// completion-reconcile — which routes the comment under its ORIGINAL agent author on
// the unattributed automation chain — would read the stale lineage and resurrect the
// automation creator's authority once the target frees up.
func TestUpdateComment_AdminEditOfAgentCommentClearsStaleLineage(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	workerID, ownerID, _ := privateAgentTestFixture(t)

	// The automation is created by ownerID, who owns the private worker — so the
	// ORIGINAL agent lineage genuinely carries invoke authority. That is precisely
	// the authority an admin edit must not be able to borrow.
	fx := newAutomationDelegationFixture(t, workerID, ownerID, "automation")
	issueID := uuidToString(fx.Issue.ID)
	// Neutralise the fixture's own (valid) mention comment so the ONLY comment whose
	// reconcile fate is under test is the admin-edited one below.
	setCommentSourceTask(t, &fx, nil)

	// A plain (no-mention) leader comment stamped with the leader's real task
	// lineage — the comment the admin edits to inject the mention.
	commentID := seedLeaderPlainComment(t, issueID, fx.LeaderAgentID, fx.LeaderTaskID)

	// A workspace admin who is NEITHER the worker owner nor the comment author.
	adminID := createPermissionTestAdmin(t, "mul4857-edit-admin@patchbay.test")

	countQueued := func() int {
		var n int
		if err := testPool.QueryRow(ctx, `
			SELECT count(*) FROM agent_task_queue
			WHERE issue_id = $1 AND agent_id = $2 AND status = 'queued'
		`, issueID, workerID).Scan(&n); err != nil {
			t.Fatalf("count queued: %v", err)
		}
		return n
	}

	// The admin edits the leader's comment to add the private @Worker mention.
	w := httptest.NewRecorder()
	r := newRequestAs(adminID, http.MethodPut, "/api/comments/"+commentID, map[string]any{
		"content": "[@Worker](mention://agent/" + workerID + ") please take this",
	})
	r = withURLParam(r, "commentId", commentID)
	testHandler.UpdateComment(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("admin UpdateComment: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	// Immediate save is judged on the admin's member identity, which holds no invoke
	// right over the private worker — nothing is enqueued.
	if got := countQueued(); got != 0 {
		t.Fatalf("admin edit must be blocked immediately (no invoke right over a private agent); got %d queued", got)
	}

	// The stale automation lineage MUST be cleared so the deferred reconcile fails closed.
	var sourceTaskValid bool
	if err := testPool.QueryRow(ctx, `SELECT source_task_id IS NOT NULL FROM comment WHERE id = $1`, commentID).Scan(&sourceTaskValid); err != nil {
		t.Fatalf("read comment source_task_id: %v", err)
	}
	if sourceTaskValid {
		t.Fatal("an admin edit of an agent comment must clear source_task_id so the deferred reconcile cannot borrow the original automation creator authority")
	}

	// The busy worker now completes: the completion reconcile routes the comment
	// under its original agent author (unattributed automation chain). With the
	// lineage cleared it must NOT resurrect the creator authority or enqueue a
	// follow-up. (Without the fix this reconcile would enqueue exactly one.)
	workerTaskID := seedCompletedTaskOnIssueBefore(t, workerID, issueID, fx.RuntimeID)
	workerTask, err := testHandler.Queries.GetAgentTask(ctx, util.MustParseUUID(workerTaskID))
	if err != nil {
		t.Fatalf("load worker task: %v", err)
	}
	testHandler.reconcileCommentsOnCompletion(ctx, &workerTask)
	if got := countQueued(); got != 0 {
		t.Fatalf("completion reconcile must not borrow the stale automation authority after an admin edit; got %d queued", got)
	}
}
