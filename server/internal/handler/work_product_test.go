package handler

import (
	"context"
	"net/http/httptest"
	"testing"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func testWorkProductUUID(t *testing.T, value string) pgtype.UUID {
	t.Helper()
	id, err := util.ParseUUID(value)
	if err != nil {
		t.Fatalf("parse UUID %q: %v", value, err)
	}
	return id
}

func TestWorkProductPage(t *testing.T) {
	tests := []struct {
		name    string
		query   string
		limit   int32
		offset  int32
		wantErr bool
	}{
		{name: "default", limit: 64, offset: 0},
		{name: "second page", query: "?page=2&per_page=25", limit: 25, offset: 25},
		{name: "invalid page", query: "?page=0", wantErr: true},
		{name: "invalid size", query: "?per_page=101", wantErr: true},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			request := httptest.NewRequest("GET", "/api/work-products/"+test.query, nil)
			limit, offset, err := workProductPage(request)
			if (err != nil) != test.wantErr {
				t.Fatalf("workProductPage() error = %v, wantErr=%v", err, test.wantErr)
			}
			if test.wantErr {
				return
			}
			if limit != test.limit || offset != test.offset {
				t.Fatalf("workProductPage() = (%d, %d), want (%d, %d)", limit, offset, test.limit, test.offset)
			}
		})
	}
}

func TestWorkProductRelationKeyUsesServerOwnedIdentity(t *testing.T) {
	issue := testWorkProductUUID(t, "11111111-1111-4111-8111-111111111111")
	task := testWorkProductUUID(t, "22222222-2222-4222-8222-222222222222")
	run := testWorkProductUUID(t, "33333333-3333-4333-8333-333333333333")
	if got, want := workProductRelationKey(issue, task, run), "issue:11111111-1111-4111-8111-111111111111:task:22222222-2222-4222-8222-222222222222:run:33333333-3333-4333-8333-333333333333"; got != want {
		t.Fatalf("relation key = %q, want %q", got, want)
	}
	if got, want := workProductRelationKey(issue, pgtype.UUID{}, pgtype.UUID{}), "issue:11111111-1111-4111-8111-111111111111:task:manual:run:none"; got != want {
		t.Fatalf("manual relation key = %q, want %q", got, want)
	}
}

func TestWorkProductRepositoryIdentityIsTransportOnly(t *testing.T) {
	valid := []string{
		"owner/repo",
		"https://github.com/Owner/Repo.git",
		"http://github.com/Owner/Repo/",
		"git@github.com:Owner/Repo.git",
		"ssh://git@github.com/Owner/Repo",
	}
	for _, value := range valid {
		got, ok := normalizeWorkProductRepoIdentity(value)
		if !ok || got != "owner/repo" {
			t.Errorf("normalizeWorkProductRepoIdentity(%q) = (%q, %v), want (owner/repo, true)", value, got, ok)
		}
	}
	for _, value := range []string{"PB-123", "github.com/owner/repo", "owner/repo/extra", "owner/repo!"} {
		if got, ok := normalizeWorkProductRepoIdentity(value); ok {
			t.Errorf("normalizeWorkProductRepoIdentity(%q) = (%q, true), want invalid", value, got)
		}
	}
	if !headRepositoryMatches("https://github.com/Owner/Repo.git", "owner/repo") {
		t.Fatal("head repository should match the canonical repository identity")
	}
	if headRepositoryMatches("https://github.com/owner/fork.git", "owner/repo") {
		t.Fatal("fork repository must not match the canonical repository identity")
	}
}

func TestWorkProductWorkspaceRepositoryScopeIsExact(t *testing.T) {
	repos := []byte(`[{"url":"https://github.com/Owner/Repo.git"},{"url":"https://github.com/other/else.git"}]`)
	if !workspaceContainsRepo(repos, "owner/repo") {
		t.Fatal("workspace repository was not found")
	}
	if workspaceContainsRepo(repos, "owner/other") {
		t.Fatal("repository scope matched a different repository")
	}
}

func TestWorkProductExecutionWorkspaceIsTaskOwned(t *testing.T) {
	if !taskExecutionWorkspaceMatches("/srv/executions/task-1", "", "/srv/executions/task-1/worktree") {
		t.Fatal("child execution workspace should match the task work directory")
	}
	if !taskExecutionWorkspaceMatches("/srv/executions/task-1/worktree", "", "/srv/executions/task-1") {
		t.Fatal("task work directory should match its execution workspace")
	}
	if taskExecutionWorkspaceMatches("/srv/executions/task-1", "", "/srv/executions/task-1-other/worktree") {
		t.Fatal("component-prefix collision must not establish ownership")
	}
	if taskExecutionWorkspaceMatches("", "", "/srv/executions/task-1") {
		t.Fatal("an execution without a server-known task path is not owned")
	}
}

func TestClassifyBranchDiscovery(t *testing.T) {
	tests := []struct {
		name       string
		headState  string
		others     int
		matches    int
		wantStatus string
		wantReason string
	}{
		{name: "default branch", headState: "default", matches: 1, wantStatus: "ineligible", wantReason: "default_branch"},
		{name: "detached head", headState: "detached", matches: 1, wantStatus: "ineligible", wantReason: "detached_head"},
		{name: "unknown head", headState: "unknown", matches: 1, wantStatus: "ineligible", wantReason: "unknown_head_state"},
		{name: "no match", headState: "attached", wantStatus: "unassociated"},
		{name: "one exact match", headState: "attached", matches: 1, wantStatus: "associated"},
		{name: "multiple matches", headState: "attached", matches: 2, wantStatus: "ambiguous", wantReason: "multiple_pull_requests_for_exact_head"},
		{name: "other execution", headState: "attached", others: 1, matches: 1, wantStatus: "ambiguous", wantReason: "branch_used_by_other_execution"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			got := classifyBranchDiscovery(test.headState, test.others, test.matches)
			if got.Status != test.wantStatus || got.Reason != test.wantReason {
				t.Fatalf("classifyBranchDiscovery() = %+v, want status=%q reason=%q", got, test.wantStatus, test.wantReason)
			}
		})
	}
}

func TestNormalizeWorkProductProvenanceEnforcesDurableFacts(t *testing.T) {
	task := db.AgentTaskQueue{
		WorkDir:        pgtype.Text{String: "/srv/executions/task-1", Valid: true},
		DurableWorkDir: pgtype.Text{String: "/srv/durable/task-1", Valid: true},
	}
	values, err := normalizeWorkProductProvenance(workProductProvenanceRequest{
		RepoIdentity:       "https://github.com/Owner/Repo.git",
		ExecutionWorkspace: "/srv/executions/task-1/worktree",
		HeadBranch:         "feature/work-product",
		HeadSHA:            "abc123",
		HeadState:          "attached",
		DiscoveryStatus:    "not_attempted",
	}, task)
	if err != nil {
		t.Fatalf("normalizeWorkProductProvenance() error = %v", err)
	}
	if values.RepoIdentity != "owner/repo" || values.HeadBranch == nil || *values.HeadBranch != "feature/work-product" {
		t.Fatalf("normalized provenance = %+v", values)
	}
	for _, test := range []workProductProvenanceRequest{
		{RepoIdentity: "https://github.com/Owner/Repo.git", ExecutionWorkspace: "relative", HeadState: "attached", HeadBranch: "main"},
		{RepoIdentity: "https://github.com/Owner/Repo.git", ExecutionWorkspace: "/srv/other", HeadState: "attached", HeadBranch: "main"},
		{RepoIdentity: "https://github.com/Owner/Repo.git", ExecutionWorkspace: "/srv/executions/task-1", HeadState: "attached"},
		{DiscoveryStatus: "associated"},
	} {
		if _, err := normalizeWorkProductProvenance(test, task); err == nil {
			t.Errorf("normalizeWorkProductProvenance(%+v) accepted invalid durable facts", test)
		}
	}
}

func TestWorkProductExecutionFactsFromCancelAck(t *testing.T) {
	facts := workProductExecutionFactsFromCancelAckRequest(TaskCancelAckRequest{
		ExecutionRepoIdentity: "owner/repo",
		ExecutionWorkspace:    "/srv/executions/task-1",
		ExecutionHeadBranch:   "agent/task-1",
		ExecutionHeadSHA:      "0123456789abcdef",
		ExecutionHeadState:    "attached",
	})
	if facts.repoIdentity != "owner/repo" ||
		facts.executionWorkspace != "/srv/executions/task-1" ||
		facts.headBranch != "agent/task-1" ||
		facts.headSHA != "0123456789abcdef" ||
		facts.headState != "attached" {
		t.Fatalf("cancel facts = %+v", facts)
	}
}

func TestCreateWorkProductRelationAllowsTaskOnlyScope(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	var agentID, runtimeID string
	dbfx.QueryRow(t, `
		SELECT id, runtime_id
		FROM agent
		WHERE workspace_id = $1
		LIMIT 1
	`, testWorkspaceID).Scan(&agentID, &runtimeID)
	taskID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id": runtimeID,
		"issue_id":   nil,
		"status":     "completed",
	})

	product, err := testHandler.Queries.CreateWorkProduct(ctx, db.CreateWorkProductParams{
		WorkspaceID:      parseUUID(testWorkspaceID),
		Kind:             "pull_request",
		Provider:         "github",
		ExternalIdentity: "owner/repo#task-only-test",
	})
	if err != nil {
		t.Fatalf("create work product: %v", err)
	}
	t.Cleanup(func() {
		_, _ = testPool.Exec(context.Background(), `DELETE FROM work_product_relation WHERE work_product_id = $1`, product.ID)
		_, _ = testPool.Exec(context.Background(), `DELETE FROM work_product WHERE id = $1`, product.ID)
	})

	relation, err := testHandler.Queries.CreateWorkProductRelation(ctx, db.CreateWorkProductRelationParams{
		WorkspaceID:    parseUUID(testWorkspaceID),
		WorkProductID:  product.ID,
		RelationKey:    "task:" + taskID,
		RelationSource: "task_explicit",
		AttachedByType: "agent",
		AttachedByID:   parseUUID(agentID),
		TaskID:         parseUUID(taskID),
	})
	if err != nil {
		t.Fatalf("create task-only work product relation: %v", err)
	}
	if relation.IssueID.Valid {
		t.Fatalf("issue_id = %s, want NULL for task-only relation", uuidToString(relation.IssueID))
	}
	if !sameWorkProductUUID(relation.TaskID, parseUUID(taskID)) {
		t.Fatalf("task_id = %s, want %s", uuidToString(relation.TaskID), taskID)
	}
}
