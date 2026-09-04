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
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
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
	for _, n := range []struct{ temp, issue, title string }{
		{"a", issueAID, "task a"},
		{"b", issueBID, "task b"},
	} {
		if _, err := testPool.Exec(ctx, `
			INSERT INTO dependency_graph_node (plan_id, workspace_id, temp_id, issue_id, title, wave)
			VALUES ($1, $2, $3, $4, $5, 0)
		`, planID, testWorkspaceID, n.temp, n.issue, n.title); err != nil {
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
	if len(found.Edges) != 1 || found.Edges[0].From != issueAID || found.Edges[0].To != issueBID || found.Edges[0].Satisfied {
		t.Fatalf("list: edges = %+v, want one unsatisfied a->b edge (body: %s)", found.Edges, bodyBytes)
	}
	if found.Readiness.Total != 2 || found.Readiness.Ready != 1 || found.Readiness.Blocked != 1 {
		t.Fatalf("list: graph readiness = %+v, want total=2 ready=1 blocked=1 (body: %s)", found.Readiness, bodyBytes)
	}
}
