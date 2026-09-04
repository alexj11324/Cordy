package handler

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

func dependencyGraphValidationFixture(edges []dependencyGraphEdgeInput) dependencyGraphApplyInput {
	return dependencyGraphApplyInput{
		Goal:          "ship the dependency graph",
		ParentIssueID: "00000000-0000-0000-0000-000000000001",
		Tasks: []dependencyGraphTaskInput{
			{TempID: "a", Title: "first", AcceptanceCriteria: []string{"first is complete"}, Outputs: []string{"first output"}},
			{TempID: "b", Title: "second", AcceptanceCriteria: []string{"second is complete"}, Outputs: []string{"second output"}},
			{TempID: "c", Title: "third", AcceptanceCriteria: []string{"third is complete"}, Outputs: []string{"third output"}},
		},
		Edges: edges,
	}
}

func TestValidateDependencyGraphPlanRejectsCycle(t *testing.T) {
	input := dependencyGraphValidationFixture([]dependencyGraphEdgeInput{
		{From: "a", To: "b", Type: dependencyGraphHardType, Reason: "b needs a", ConsumedOutput: "first output"},
		{From: "b", To: "a", Type: dependencyGraphHardType, Reason: "a needs b", ConsumedOutput: "second output"},
	})
	_, err := validateDependencyGraphPlan(&input)
	if err == nil || !strings.Contains(err.Error(), "cycle") {
		t.Fatal("validateDependencyGraphPlan accepted a cyclic graph")
	}
}

func TestValidateDependencyGraphPlanRejectsTransitiveEdge(t *testing.T) {
	input := dependencyGraphValidationFixture([]dependencyGraphEdgeInput{
		{From: "a", To: "b", Type: dependencyGraphHardType, Reason: "b needs a", ConsumedOutput: "first output"},
		{From: "b", To: "c", Type: dependencyGraphHardType, Reason: "c needs b", ConsumedOutput: "second output"},
		{From: "a", To: "c", Type: dependencyGraphHardType, Reason: "c needs a", ConsumedOutput: "first output"},
	})
	if _, err := validateDependencyGraphPlan(&input); err == nil {
		t.Fatal("validateDependencyGraphPlan accepted a transitively redundant edge")
	}
}

func TestValidateDependencyGraphPlanUsesExplicitRoleContracts(t *testing.T) {
	input := dependencyGraphValidationFixture(nil)
	runtimeID := "00000000-0000-0000-0000-000000000004"
	input.Tasks[0].Owner = &dependencyGraphRoleInput{Type: "member", ID: "00000000-0000-0000-0000-000000000002"}
	input.Tasks[0].Executor = &dependencyGraphRoleInput{Type: "agent", ID: "00000000-0000-0000-0000-000000000003"}
	input.Tasks[0].CandidateExecutors = []dependencyGraphRoleInput{
		{Type: "team", ID: "00000000-0000-0000-0000-000000000005"},
	}
	input.Tasks[0].Reviewer = &dependencyGraphRoleInput{Type: "member", ID: "00000000-0000-0000-0000-000000000006"}
	input.Tasks[0].RuntimeID = &runtimeID
	modelID := "claude-test-model"
	input.Tasks[0].ModelID = &modelID

	if _, err := validateDependencyGraphPlan(&input); err != nil {
		t.Fatalf("validateDependencyGraphPlan rejected explicit role contract: %v", err)
	}
	encoded, err := json.Marshal(input)
	if err != nil {
		t.Fatalf("marshal explicit role contract: %v", err)
	}
	if strings.Contains(string(encoded), "assign"+"ee") {
		t.Fatalf("explicit dependency graph contract still serializes legacy assignee fields: %s", encoded)
	}
	for _, field := range []string{"owner", "executor", "candidate_executors", "reviewer"} {
		if !strings.Contains(string(encoded), `"`+field+`"`) {
			t.Fatalf("explicit dependency graph contract omitted %s: %s", field, encoded)
		}
	}
}

func TestValidateDependencyGraphPlanRejectsRoleKindMismatch(t *testing.T) {
	tests := []struct {
		name   string
		mutate func(*dependencyGraphTaskInput)
		want   string
	}{
		{
			name: "owner cannot be an agent",
			mutate: func(task *dependencyGraphTaskInput) {
				task.Owner = &dependencyGraphRoleInput{Type: "agent", ID: "00000000-0000-0000-0000-000000000002"}
			},
			want: "tasks[0].owner.type must be member",
		},
		{
			name: "executor cannot be a member",
			mutate: func(task *dependencyGraphTaskInput) {
				task.Executor = &dependencyGraphRoleInput{Type: "member", ID: "00000000-0000-0000-0000-000000000002"}
			},
			want: "tasks[0].executor.type must be agent or team",
		},
		{
			name: "candidate cannot be a member",
			mutate: func(task *dependencyGraphTaskInput) {
				task.CandidateExecutors = []dependencyGraphRoleInput{{Type: "member", ID: "00000000-0000-0000-0000-000000000002"}}
			},
			want: "tasks[0].candidate_executors[0].type must be agent or team",
		},
		{
			name: "reviewer accepts all role kinds",
			mutate: func(task *dependencyGraphTaskInput) {
				task.Reviewer = &dependencyGraphRoleInput{Type: "service", ID: "00000000-0000-0000-0000-000000000002"}
			},
			want: "tasks[0].reviewer.type must be member, agent, or team",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			input := dependencyGraphValidationFixture(nil)
			tt.mutate(&input.Tasks[0])
			if _, err := validateDependencyGraphPlan(&input); err == nil || !strings.Contains(err.Error(), tt.want) {
				t.Fatalf("validateDependencyGraphPlan error = %v, want %q", err, tt.want)
			}
		})
	}
}

func TestValidateDependencyGraphExecutionTargetRequiresPair(t *testing.T) {
	runtimeID := "00000000-0000-0000-0000-000000000002"
	modelID := "claude-test-model"
	if _, _, err := validateDependencyGraphExecutionTarget(&runtimeID, nil, "tasks[0]"); err == nil || !strings.Contains(err.Error(), "must be provided together") {
		t.Fatalf("runtime without model error = %v, want paired-target validation", err)
	}
	if _, _, err := validateDependencyGraphExecutionTarget(nil, &modelID, "tasks[0]"); err == nil || !strings.Contains(err.Error(), "must be provided together") {
		t.Fatalf("model without runtime error = %v, want paired-target validation", err)
	}
}

func TestDependencyGraphCursorProjectMismatch(t *testing.T) {
	projectA := pgtype.UUID{Bytes: [16]byte{1}, Valid: true}
	projectB := pgtype.UUID{Bytes: [16]byte{2}, Valid: true}
	plan := db.DependencyGraphPlan{
		ID:        pgtype.UUID{Bytes: [16]byte{3}, Valid: true},
		UpdatedAt: pgtype.Timestamptz{Time: time.Unix(10, 0).UTC(), Valid: true},
	}
	cursor, err := encodeDependencyGraphCursor(&projectA, plan)
	if err != nil {
		t.Fatalf("encode cursor: %v", err)
	}
	_, err = decodeDependencyGraphCursor(cursor, &projectB)
	var graphErr *dependencyGraphError
	if !errors.As(err, &graphErr) || graphErr.code != "cursor_project_mismatch" {
		t.Fatalf("decode cursor error = %v, want cursor_project_mismatch", err)
	}
}

type nestedGraphResponse struct {
	Plan struct {
		ID string `json:"id"`
	} `json:"plan"`
	Parent struct {
		ID string `json:"id"`
	} `json:"parent"`
	Children []struct {
		ID string `json:"id"`
	} `json:"children"`
	Nodes []struct {
		IssueID string `json:"issue_id"`
		Issue   struct {
			ID string `json:"id"`
		} `json:"issue"`
		Readiness struct {
			State                  string `json:"state"`
			GateOpen               bool   `json:"gate_open"`
			SatisfiedPrerequisites int    `json:"satisfied_prerequisites"`
			TotalPrerequisites     int    `json:"total_prerequisites"`
		} `json:"readiness"`
	} `json:"nodes"`
	Edges []struct {
		From      string `json:"from"`
		To        string `json:"to"`
		Satisfied bool   `json:"satisfied"`
	} `json:"edges"`
	Readiness struct {
		Total   int `json:"total"`
		Ready   int `json:"ready"`
		Blocked int `json:"blocked"`
	} `json:"readiness"`
	Waves [][]string `json:"waves"`
}

func dependencyGraphTestString(value any, key string) string {
	record, ok := value.(map[string]any)
	if !ok {
		return ""
	}
	result, _ := record[key].(string)
	return result
}

func TestListDependencyGraphsReturnsNestedResponses(t *testing.T) {
	if testHandler == nil || testPool == nil || dbfx == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()

	parentID := dbfx.Issue(t, "graph parent")
	issueAID := dbfx.Issue(t, "graph task a")
	issueBID := dbfx.Issue(t, "graph task b")

	var planID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO dependency_graph_plan (workspace_id, parent_issue_id, idempotency_key, request_hash, goal, status, created_by_type, created_by_id)
		VALUES ($1, $2, $3, $4, $5, 'active', 'member', $6)
		RETURNING id
	`, testWorkspaceID, parentID, "test-nested-"+t.Name(), "hash-nested", "ship it", testUserID).Scan(&planID); err != nil {
		t.Fatalf("insert plan: %v", err)
	}
	for _, n := range []struct {
		temp, issue, title string
		wave              int
	}{
		{"a", issueAID, "task a", 0},
		{"b", issueBID, "task b", 1},
	} {
		if _, err := testPool.Exec(ctx, `
			INSERT INTO dependency_graph_node (plan_id, workspace_id, temp_id, issue_id, title, wave)
			VALUES ($1, $2, $3, $4, $5, $6)
		`, planID, testWorkspaceID, n.temp, n.issue, n.title, n.wave); err != nil {
			t.Fatalf("insert node %s: %v", n.temp, err)
		}
	}
	if _, err := testPool.Exec(ctx, `
		INSERT INTO dependency_graph_edge (plan_id, workspace_id, from_issue_id, to_issue_id, type, reason, consumed_output)
		VALUES ($1, $2, $3, $4, 'hard', 'b needs a', '')
	`, planID, testWorkspaceID, issueAID, issueBID); err != nil {
		t.Fatalf("insert edge: %v", err)
	}
	t.Cleanup(func() {
		c := context.Background()
		testPool.Exec(c, `DELETE FROM dependency_graph_edge WHERE plan_id = $1`, planID)
		testPool.Exec(c, `DELETE FROM dependency_graph_node WHERE plan_id = $1`, planID)
		testPool.Exec(c, `DELETE FROM dependency_graph_plan WHERE id = $1`, planID)
		for _, id := range []string{parentID, issueAID, issueBID} {
			testPool.Exec(c, `DELETE FROM issue WHERE id = $1`, id)
		}
	})

	req := httptest.NewRequest(http.MethodGet, "/api/dependency-graphs", nil)
	req.Header.Set("X-User-ID", testUserID)
	req.Header.Set("X-Workspace-Slug", handlerTestWorkspaceSlug)
	w := httptest.NewRecorder()
	testHandler.ListDependencyGraphs(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("list: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var body struct {
		Graphs []nestedGraphResponse `json:"graphs"`
	}
	bodyBytes := w.Body.Bytes()
	if err := json.Unmarshal(bodyBytes, &body); err != nil {
		t.Fatalf("decode list: %v (body: %s)", err, bodyBytes)
	}
	var found *nestedGraphResponse
	for i := range body.Graphs {
		if body.Graphs[i].Plan.ID == planID {
			found = &body.Graphs[i]
			break
		}
	}
	if found == nil {
		t.Fatalf("list: plan %s missing from response (body: %s)", planID, bodyBytes)
	}
	if found.Parent.ID != parentID || len(found.Children) != 2 {
		t.Fatalf("list: parent/children = %s/%d, want %s/2 (body: %s)", found.Parent.ID, len(found.Children), parentID, bodyBytes)
	}
	if len(found.Waves) != 2 || len(found.Waves[0]) != 1 || found.Waves[0][0] != "a" || len(found.Waves[1]) != 1 || found.Waves[1][0] != "b" {
		t.Fatalf("list: waves = %+v, want [[a] [b]] (body: %s)", found.Waves, bodyBytes)
	}
	if len(found.Nodes) != 2 {
		t.Fatalf("list: expected 2 nodes, got %d (body: %s)", len(found.Nodes), bodyBytes)
	}
	byIssue := map[string]int{}
	for i, n := range found.Nodes {
		byIssue[n.IssueID] = i
		if n.Issue.ID == "" {
			t.Fatalf("list: node %s missing embedded issue (body: %s)", n.IssueID, bodyBytes)
		}
	}
	nodeA := found.Nodes[byIssue[issueAID]]
	if nodeA.Readiness.State != "ready" || !nodeA.Readiness.GateOpen {
		t.Fatalf("list: node A readiness = %+v, want state=ready gate open (body: %s)", nodeA.Readiness, bodyBytes)
	}
	nodeB := found.Nodes[byIssue[issueBID]]
	if nodeB.Readiness.State != "blocked" || nodeB.Readiness.TotalPrerequisites != 1 {
		t.Fatalf("list: node B readiness = %+v, want state=blocked total=1 (body: %s)", nodeB.Readiness, bodyBytes)
	}
	if len(found.Edges) != 1 || found.Edges[0].From != "a" || found.Edges[0].To != "b" || found.Edges[0].Satisfied {
		t.Fatalf("list: edges = %+v, want one unsatisfied a->b edge with temp-id endpoints (body: %s)", found.Edges, bodyBytes)
	}
	if found.Readiness.Total != 2 || found.Readiness.Ready != 1 || found.Readiness.Blocked != 1 {
		t.Fatalf("list: graph readiness = %+v, want total=2 ready=1 blocked=1 (body: %s)", found.Readiness, bodyBytes)
	}
}

func TestRetireDependencyGraphCancelsChildrenAndTasksAtomically(t *testing.T) {
	if testHandler == nil || testPool == nil || dbfx == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	parentID := dbfx.Issue(t, "graph retirement parent")
	agentID := dbfx.Agent(t, "graph retirement agent", testRuntimeID)
	childID := dbfx.Issue(t, "graph retirement child", testutil.Cols{
		"status":        "in_progress",
		"executor_type": "agent",
		"executor_id":   agentID,
		"parent_issue_id": parentID,
	})
	taskID := dbfx.Task(t, agentID, testutil.Cols{
		"issue_id":   childID,
		"runtime_id": testRuntimeID,
		"status":     "waiting_capacity",
	})

	var planID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO dependency_graph_plan (workspace_id, parent_issue_id, idempotency_key, request_hash, goal, status, created_by_type, created_by_id)
		VALUES ($1, $2, $3, $4, 'retire graph', 'active', 'member', $5)
		RETURNING id
	`, testWorkspaceID, parentID, "test-retire-"+t.Name(), "hash-retire", testUserID).Scan(&planID); err != nil {
		t.Fatalf("insert retirement plan: %v", err)
	}
	t.Cleanup(func() {
		cleanupCtx := context.Background()
		_, _ = testPool.Exec(cleanupCtx, `DELETE FROM dependency_graph_node WHERE plan_id = $1`, planID)
		_, _ = testPool.Exec(cleanupCtx, `DELETE FROM dependency_graph_plan WHERE id = $1`, planID)
		_, _ = testPool.Exec(cleanupCtx, `DELETE FROM agent_task_queue WHERE id = $1`, taskID)
		_, _ = testPool.Exec(cleanupCtx, `DELETE FROM issue WHERE id IN ($1, $2)`, parentID, childID)
		_, _ = testPool.Exec(cleanupCtx, `DELETE FROM agent WHERE id = $1`, agentID)
	})
	if _, err := testPool.Exec(ctx, `
		INSERT INTO dependency_graph_node (plan_id, workspace_id, temp_id, issue_id, title, wave)
		VALUES ($1, $2, 'child', $3, 'graph retirement child', 0)
	`, planID, testWorkspaceID, childID); err != nil {
		t.Fatalf("insert retirement node: %v", err)
	}

	req := httptest.NewRequest(http.MethodPost, "/api/dependency-graphs/"+planID+"/retire", nil)
	req.Header.Set("X-User-ID", testUserID)
	req.Header.Set("X-Workspace-Slug", handlerTestWorkspaceSlug)
	req = withURLParam(req, "id", planID)
	w := httptest.NewRecorder()
	testHandler.RetireDependencyGraph(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("retire: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var planStatus, issueStatus, taskStatus string
	if err := testPool.QueryRow(ctx, `
		SELECT plan.status, issue.status, task.status
		FROM dependency_graph_plan plan
		JOIN issue ON issue.id = $2
		JOIN agent_task_queue task ON task.id = $3
		WHERE plan.id = $1
	`, planID, childID, taskID).Scan(&planStatus, &issueStatus, &taskStatus); err != nil {
		t.Fatalf("read retirement state: %v", err)
	}
	if planStatus != "cancelled" || issueStatus != "cancelled" || taskStatus != "cancelled" {
		t.Fatalf("retirement state = plan %q, issue %q, task %q; want all cancelled", planStatus, issueStatus, taskStatus)
	}
}

func TestDeleteIssueCleansAffectedDependencyGraph(t *testing.T) {
	if testHandler == nil || testPool == nil || dbfx == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	parentID := dbfx.Issue(t, "graph delete parent")
	agentID := dbfx.Agent(t, "graph delete agent", testRuntimeID)
	childID := dbfx.Issue(t, "graph delete child", testutil.Cols{
		"status":          "in_progress",
		"executor_type":   "agent",
		"executor_id":     agentID,
		"parent_issue_id": parentID,
	})
	taskID := dbfx.Task(t, agentID, testutil.Cols{
		"issue_id":   childID,
		"runtime_id": testRuntimeID,
		"status":     "waiting_capacity",
	})

	var planID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO dependency_graph_plan (workspace_id, parent_issue_id, idempotency_key, request_hash, goal, status, created_by_type, created_by_id)
		VALUES ($1, $2, $3, $4, 'delete graph', 'active', 'member', $5)
		RETURNING id
	`, testWorkspaceID, parentID, "test-delete-"+t.Name(), "hash-delete", testUserID).Scan(&planID); err != nil {
		t.Fatalf("insert delete plan: %v", err)
	}
	t.Cleanup(func() {
		cleanupCtx := context.Background()
		_, _ = testPool.Exec(cleanupCtx, `DELETE FROM dependency_graph_node WHERE plan_id = $1`, planID)
		_, _ = testPool.Exec(cleanupCtx, `DELETE FROM dependency_graph_plan WHERE id = $1`, planID)
		_, _ = testPool.Exec(cleanupCtx, `DELETE FROM agent_task_queue WHERE id = $1`, taskID)
		_, _ = testPool.Exec(cleanupCtx, `DELETE FROM issue WHERE id IN ($1, $2)`, parentID, childID)
		_, _ = testPool.Exec(cleanupCtx, `DELETE FROM agent WHERE id = $1`, agentID)
	})
	if _, err := testPool.Exec(ctx, `
		INSERT INTO dependency_graph_node (plan_id, workspace_id, temp_id, issue_id, title, wave)
		VALUES ($1, $2, 'child', $3, 'graph delete child', 0)
	`, planID, testWorkspaceID, childID); err != nil {
		t.Fatalf("insert delete node: %v", err)
	}

	req := newRequest(http.MethodDelete, "/api/issues/"+parentID, nil)
	req = withURLParam(req, "id", parentID)
	w := httptest.NewRecorder()
	testHandler.DeleteIssue(w, req)
	if w.Code != http.StatusNoContent {
		t.Fatalf("delete: expected 204, got %d: %s", w.Code, w.Body.String())
	}

	var issueStatus, taskStatus string
	if err := testPool.QueryRow(ctx, `SELECT status FROM issue WHERE id = $1`, childID).Scan(&issueStatus); err != nil {
		t.Fatalf("read surviving graph child: %v", err)
	}
	if err := testPool.QueryRow(ctx, `SELECT status FROM agent_task_queue WHERE id = $1`, taskID).Scan(&taskStatus); err != nil {
		t.Fatalf("read cancelled graph task: %v", err)
	}
	if issueStatus != "cancelled" || taskStatus != "cancelled" {
		t.Fatalf("delete cancellation = issue %q, task %q; want both cancelled", issueStatus, taskStatus)
	}
	var graphRows int
	if err := testPool.QueryRow(ctx, `
		SELECT (SELECT COUNT(*) FROM dependency_graph_plan WHERE id = $1)
		     + (SELECT COUNT(*) FROM dependency_graph_node WHERE plan_id = $1)
	`, planID).Scan(&graphRows); err != nil {
		t.Fatalf("count deleted graph rows: %v", err)
	}
	if graphRows != 0 {
		t.Fatalf("dependency graph rows after parent delete = %d, want 0", graphRows)
	}
}

// TestApplyDependencyGraphRoundTripsRolesAndRealtime is the handler-level
// acceptance for the Rust graph contract. It exercises the durable boundary,
// not only JSON validation: apply creates role-explicit child issues/nodes,
// stores the prerequisite gate, read returns the same parent/children/waves,
// and an idempotent replay emits the graph refresh event without duplicating
// the plan or children.
func TestApplyDependencyGraphRoundTripsRolesAndRealtime(t *testing.T) {
	if testHandler == nil || testPool == nil || dbfx == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	parentID := dbfx.Issue(t, "graph apply parent")
	agentID := handlerSeededAgentID(t)
	key := "graph-apply-" + strings.ToLower(strings.ReplaceAll(t.Name(), "/", "-"))

	t.Cleanup(func() {
		cleanupCtx := context.Background()
		_, _ = testPool.Exec(cleanupCtx, `
			DELETE FROM agent_task_queue
			WHERE issue_id IN (SELECT id FROM issue WHERE parent_issue_id = $1)`, parentID)
		_, _ = testPool.Exec(cleanupCtx, `
			DELETE FROM dependency_graph_issue_created_outbox
			WHERE plan_id IN (SELECT id FROM dependency_graph_plan WHERE parent_issue_id = $1)`, parentID)
		_, _ = testPool.Exec(cleanupCtx, `
			DELETE FROM dependency_graph_edge
			WHERE plan_id IN (SELECT id FROM dependency_graph_plan WHERE parent_issue_id = $1)`, parentID)
		_, _ = testPool.Exec(cleanupCtx, `
			DELETE FROM dependency_graph_node
			WHERE plan_id IN (SELECT id FROM dependency_graph_plan WHERE parent_issue_id = $1)`, parentID)
		_, _ = testPool.Exec(cleanupCtx, `DELETE FROM dependency_graph_plan WHERE parent_issue_id = $1`, parentID)
		_, _ = testPool.Exec(cleanupCtx, `DELETE FROM issue WHERE parent_issue_id = $1`, parentID)
	})

	var graphEvents []events.Event
	testHandler.Bus.Subscribe(protocol.EventDependencyGraphUpdated, func(event events.Event) {
		payload, ok := event.Payload.(map[string]any)
		if !ok || payload["parent_issue_id"] != parentID {
			return
		}
		graphEvents = append(graphEvents, event)
	})

	role := func(kind, id string) map[string]any {
		return map[string]any{"type": kind, "id": id}
	}
	body := map[string]any{
		"goal":            "round trip a dependency graph",
		"parent_issue_id": parentID,
		"tasks": []any{
			map[string]any{
				"temp_id":             "root",
				"title":               "graph root",
				"description":         "",
				"acceptance_criteria": []string{"root is complete"},
				"context":             map[string]any{},
				"outputs":             []string{"root artifact"},
				"owner":               role("member", testUserID),
				"candidate_executors": []any{role("agent", agentID)},
				"reviewer":            role("member", testUserID),
			},
			map[string]any{
				"temp_id":             "child",
				"title":               "graph child",
				"description":         "",
				"acceptance_criteria": []string{"child is complete"},
				"context":             map[string]any{},
				"outputs":             []string{"child artifact"},
				"owner":               role("member", testUserID),
				"candidate_executors": []any{role("agent", agentID)},
				"reviewer":            role("member", testUserID),
			},
		},
		"edges": []any{
			map[string]any{
				"from":            "root",
				"to":              "child",
				"type":            "hard",
				"reason":          "child consumes the root artifact",
				"consumed_output": "root artifact",
			},
		},
	}

	apply := func() (*httptest.ResponseRecorder, map[string]any) {
		req := withURLParam(newRequest(http.MethodPost, "/api/issues/"+parentID+"/dependency-graph/apply", body), "id", parentID)
		req.Header.Set("Idempotency-Key", key)
		w := httptest.NewRecorder()
		testHandler.ApplyIssueDependencyGraph(w, req)
		var decoded map[string]any
		if err := json.Unmarshal(w.Body.Bytes(), &decoded); err != nil {
			t.Fatalf("decode apply response: %v\n%s", err, w.Body.String())
		}
		return w, decoded
	}

	first, firstBody := apply()
	if first.Code != http.StatusCreated {
		t.Fatalf("apply: expected 201, got %d: %s", first.Code, first.Body.String())
	}
	if firstBody["replayed"] != false {
		t.Fatalf("first apply replayed = %#v, want false", firstBody["replayed"])
	}
	plan, ok := firstBody["plan"].(map[string]any)
	if !ok || dependencyGraphTestString(plan, "id") == "" {
		t.Fatalf("first apply plan = %#v, want an id", firstBody["plan"])
	}
	planID := dependencyGraphTestString(plan, "id")
	if dependencyGraphTestString(firstBody, "parent") != "" {
		// The parent is an object in the wire contract; this branch only keeps
		// the failure message useful if a server accidentally regresses it to a
		// scalar.
		t.Fatalf("first apply parent unexpectedly serialized as a string: %#v", firstBody["parent"])
	}
	parentResponse, ok := firstBody["parent"].(map[string]any)
	if !ok || dependencyGraphTestString(parentResponse, "id") != parentID {
		t.Fatalf("first apply parent = %#v, want %s", firstBody["parent"], parentID)
	}
	children, ok := firstBody["children"].([]any)
	if !ok || len(children) != 2 {
		t.Fatalf("first apply children = %#v, want two child issues", firstBody["children"])
	}
	waves, ok := firstBody["waves"].([]any)
	if !ok || len(waves) != 2 {
		t.Fatalf("first apply waves = %#v, want two waves", firstBody["waves"])
	}
	nodes, ok := firstBody["nodes"].([]any)
	if !ok || len(nodes) != 2 {
		t.Fatalf("first apply nodes = %#v, want two nodes", firstBody["nodes"])
	}
	byTempID := make(map[string]map[string]any, len(nodes))
	for _, raw := range nodes {
		node, ok := raw.(map[string]any)
		if !ok {
			t.Fatalf("first apply node = %#v, want object", raw)
		}
		byTempID[dependencyGraphTestString(node, "temp_id")] = node
	}
	root := byTempID["root"]
	child := byTempID["child"]
	if root == nil || child == nil {
		t.Fatalf("first apply node temp ids = %#v", byTempID)
	}
	if dependencyGraphTestString(root, "owner_type") != "member" || dependencyGraphTestString(root, "owner_id") != testUserID || dependencyGraphTestString(root, "reviewer_type") != "member" || dependencyGraphTestString(root, "reviewer_id") != testUserID {
		t.Fatalf("root role projection = %#v", root)
	}
	if dependencyGraphTestString(child, "status") != "blocked" {
		t.Fatalf("child status = %q, want blocked", dependencyGraphTestString(child, "status"))
	}
	childReadiness, ok := child["readiness"].(map[string]any)
	if !ok || dependencyGraphTestString(childReadiness, "state") != "blocked" || childReadiness["gate_open"] != false || childReadiness["total_prerequisites"] != float64(1) || !strings.Contains(dependencyGraphTestString(childReadiness, "unlock_condition"), "All 1 hard prerequisites") {
		t.Fatalf("child readiness = %#v", child["readiness"])
	}
	edges, ok := firstBody["edges"].([]any)
	if !ok || len(edges) != 1 {
		t.Fatalf("first apply edges = %#v, want one edge", firstBody["edges"])
	}
	edge, ok := edges[0].(map[string]any)
	if !ok || dependencyGraphTestString(edge, "from") != "root" || dependencyGraphTestString(edge, "to") != "child" {
		t.Fatalf("first apply edge endpoints = %#v, want temp ids root -> child", edges[0])
	}
	if len(graphEvents) != 1 || graphEvents[0].Type != protocol.EventDependencyGraphUpdated {
		t.Fatalf("graph events after apply = %+v, want one dependency_graph:updated event", graphEvents)
	}

	var rootStatus, childStatus string
	var rootAcceptance, childAcceptance []byte
	if err := testPool.QueryRow(ctx, `
		SELECT root_issue.status, root_issue.acceptance_criteria,
		       child_issue.status, child_issue.acceptance_criteria
		FROM dependency_graph_node root_node
		JOIN issue root_issue ON root_issue.id = root_node.issue_id
		JOIN dependency_graph_edge graph_edge
		  ON graph_edge.plan_id = root_node.plan_id
		 AND graph_edge.from_issue_id = root_node.issue_id
		JOIN dependency_graph_node child_node
		  ON child_node.plan_id = graph_edge.plan_id
		 AND child_node.issue_id = graph_edge.to_issue_id
		JOIN issue child_issue ON child_issue.id = child_node.issue_id
		WHERE root_node.plan_id = $1
		  AND root_node.temp_id = 'root'
		  AND child_node.temp_id = 'child'`, planID).Scan(&rootStatus, &rootAcceptance, &childStatus, &childAcceptance); err != nil {
		t.Fatalf("read persisted child issues: %v", err)
	}
	if rootStatus != "in_progress" || childStatus != "blocked" {
		t.Fatalf("persisted statuses = %s/%s, want in_progress/blocked", rootStatus, childStatus)
	}
	if string(rootAcceptance) != `["root is complete"]` || string(childAcceptance) != `["child is complete"]` {
		t.Fatalf("persisted acceptance criteria = %s/%s", rootAcceptance, childAcceptance)
	}

	// The apply handler returns its pre-admission snapshot, while the committed
	// root is already in progress and has one normal queued task. Completing the
	// prerequisite through the same status transition hook must promote the
	// blocked child and enqueue its task exactly once.
	rootIssueID := parseUUID(dependencyGraphTestString(root, "issue_id"))
	childIssueID := parseUUID(dependencyGraphTestString(child, "issue_id"))
	rootBefore, err := testHandler.Queries.GetIssueInWorkspace(ctx, db.GetIssueInWorkspaceParams{
		ID:          rootIssueID,
		WorkspaceID: parseUUID(testWorkspaceID),
	})
	if err != nil {
		t.Fatalf("load admitted graph root: %v", err)
	}
	rootDone, err := testHandler.Queries.UpdateIssueStatus(ctx, db.UpdateIssueStatusParams{
		ID:          rootIssueID,
		Status:      "done",
		WorkspaceID: rootBefore.WorkspaceID,
	})
	if err != nil {
		t.Fatalf("complete graph root: %v", err)
	}
	testHandler.reconcileDependencyGraphTransition(ctx, rootBefore, rootDone)

	var promotedChildStatus string
	var graphTaskCount int
	if err := testPool.QueryRow(ctx, `SELECT status FROM issue WHERE id = $1`, childIssueID).Scan(&promotedChildStatus); err != nil {
		t.Fatalf("read promoted graph child: %v", err)
	}
	if err := testPool.QueryRow(ctx, `
		SELECT COUNT(*) FROM agent_task_queue
		WHERE issue_id IN ($1, $2) AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'waiting_capacity', 'deferred')
	`, rootIssueID, childIssueID).Scan(&graphTaskCount); err != nil {
		t.Fatalf("count graph tasks: %v", err)
	}
	if promotedChildStatus != "in_progress" || graphTaskCount != 2 {
		t.Fatalf("dependency scheduler result = child status %q, active tasks %d; want in_progress/2", promotedChildStatus, graphTaskCount)
	}

	get := withURLParam(newRequest(http.MethodGet, "/api/issues/"+parentID+"/dependency-graph", nil), "id", parentID)
	getResponse := httptest.NewRecorder()
	testHandler.GetIssueDependencyGraph(getResponse, get)
	if getResponse.Code != http.StatusOK {
		t.Fatalf("read by parent: expected 200, got %d: %s", getResponse.Code, getResponse.Body.String())
	}
	var readBody map[string]any
	if err := json.Unmarshal(getResponse.Body.Bytes(), &readBody); err != nil {
		t.Fatalf("decode read response: %v", err)
	}
	if dependencyGraphTestString(readBody["plan"].(map[string]any), "id") != planID || len(readBody["children"].([]any)) != 2 || len(readBody["waves"].([]any)) != 2 {
		t.Fatalf("read by parent lost graph material: %#v", readBody)
	}

	getPlan := withURLParam(newRequest(http.MethodGet, "/api/dependency-graphs/"+planID, nil), "id", planID)
	getPlanResponse := httptest.NewRecorder()
	testHandler.GetDependencyGraphByID(getPlanResponse, getPlan)
	if getPlanResponse.Code != http.StatusOK {
		t.Fatalf("read by plan: expected 200, got %d: %s", getPlanResponse.Code, getPlanResponse.Body.String())
	}

	replay, replayBody := apply()
	if replay.Code != http.StatusOK || replayBody["replayed"] != true || dependencyGraphTestString(replayBody["plan"].(map[string]any), "id") != planID {
		t.Fatalf("replay = %d/%#v, want 200, replayed=true, same plan", replay.Code, replayBody)
	}
	if len(graphEvents) != 2 || graphEvents[1].Payload.(map[string]any)["replayed"] != true {
		t.Fatalf("graph events after replay = %+v, want second replay event", graphEvents)
	}

	var plans, childCount int64
	if err := testPool.QueryRow(ctx, `SELECT COUNT(*) FROM dependency_graph_plan WHERE parent_issue_id = $1`, parentID).Scan(&plans); err != nil {
		t.Fatalf("count plans: %v", err)
	}
	if err := testPool.QueryRow(ctx, `SELECT COUNT(*) FROM issue WHERE parent_issue_id = $1`, parentID).Scan(&childCount); err != nil {
		t.Fatalf("count child issues: %v", err)
	}
	if plans != 1 || childCount != 2 {
		t.Fatalf("replay duplicated durable rows: plans=%d children=%d, want 1/2", plans, childCount)
	}
}
