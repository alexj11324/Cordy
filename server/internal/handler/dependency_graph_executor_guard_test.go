package handler

import (
	"encoding/json"
	"net/http"
	"strings"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/testutil"
)

func TestApplyDependencyGraphRejectsUnassignedBlockedNodes(t *testing.T) {
	if testHandler == nil || dbfx == nil {
		t.Skip("requires test database")
	}
	workspaceID := dbfx.Workspace(t, "Executor guard", "graph-executor-guard", testutil.Cols{"issue_counter": 1})
	dbfx.Member(t, workspaceID, testUserID, "owner")
	parentID := dbfx.Issue(t, "Guard parent", testutil.Cols{"workspace_id": workspaceID})
	// These rows only exist in the success control below. No global policy changes.
	for _, table := range []string{"issue", "dependency_graph_plan", "dependency_graph_node", "dependency_graph_edge", "dependency_graph_issue_created_outbox"} {
		dbfx.Cleanup(t, "DELETE FROM "+table+" WHERE workspace_id=$1", workspaceID)
	}
	input := dependencyGraphValidationFixture([]dependencyGraphEdgeInput{{From: "a", To: "b", Type: "hard", Reason: "b consumes a", ConsumedOutput: "first output"}})
	input.ParentIssueID = parentID
	for index := range input.Tasks {
		input.Tasks[index].Context = json.RawMessage(`{}`)
	}
	call := func(key string) *testutil.Response {
		req := testutil.JSONRequest(http.MethodPost, "/api/issues/"+parentID+"/dependency-graph/apply", input)
		testutil.WithHeaders(req, "X-User-ID", testUserID, "X-Workspace-ID", workspaceID, "Idempotency-Key", key)
		req = testutil.WithURLParams(req, "id", parentID)
		return testutil.Call(t, testHandler.ApplyIssueDependencyGraph, req)
	}
	response := call("missing-executor").Want(http.StatusUnprocessableEntity).Map()
	if response["code"] != "active_executor_required" {
		t.Fatalf("wrong error: %v", response)
	}
	if !strings.Contains(response["error"].(string), "tasks[1]") {
		t.Fatalf("error must identify the unassigned task: %v", response)
	}
	for _, table := range []string{"dependency_graph_plan", "dependency_graph_node", "dependency_graph_edge", "dependency_graph_issue_created_outbox"} {
		if n := dbfx.Count(t, "SELECT count(*) FROM "+table+" WHERE workspace_id=$1", workspaceID); n != 0 {
			t.Fatalf("rejected plan left %d %s rows", n, table)
		}
	}
	if n := dbfx.Count(t, "SELECT count(*) FROM issue WHERE workspace_id=$1", workspaceID); n != 1 {
		t.Fatalf("rejected plan left child issues: %d", n)
	}
	if n := dbfx.Count(t, "SELECT count(*) FROM agent_task_queue q JOIN issue i ON i.id=q.issue_id WHERE i.workspace_id=$1", workspaceID); n != 0 {
		t.Fatalf("rejected plan queued tasks: %d", n)
	}
	// The database guard preserves existing admission policy for independent roots.
	input.Edges = nil
	call("independent-roots").Want(http.StatusCreated)
	if n := dbfx.Count(t, "SELECT count(*) FROM issue WHERE parent_issue_id=$1 AND status='todo' AND executor_id IS NULL", parentID); n != 3 {
		t.Fatalf("independent roots = %d, want 3", n)
	}
}
