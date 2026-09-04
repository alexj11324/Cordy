package handler

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestListGroupedIssuesExecutorPaginatesPerGroup(t *testing.T) {
	ctx := context.Background()

	suffix := time.Now().UnixNano()
	var ownerID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO "user" (name, email)
		VALUES ($1, $2)
		RETURNING id
	`, "Grouped Issues Test User", fmt.Sprintf("grouped-%d@patchbay.ai", suffix)).Scan(&ownerID); err != nil {
		t.Fatalf("create owner user: %v", err)
	}
	t.Cleanup(func() {
		_, _ = testPool.Exec(context.Background(), `DELETE FROM "user" WHERE id = $1`, ownerID)
	})

	if _, err := testPool.Exec(ctx, `
		INSERT INTO member (workspace_id, user_id, role)
		VALUES ($1, $2, 'member')
	`, testWorkspaceID, ownerID); err != nil {
		t.Fatalf("create owner member: %v", err)
	}

	var agentID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO agent (
			workspace_id, name, description, runtime_mode, runtime_config,
			runtime_id, visibility, max_concurrent_tasks, owner_id
		)
		VALUES ($1, $2, '', 'cloud', '{}'::jsonb, $3, 'workspace', 1, $4)
		RETURNING id
	`, testWorkspaceID, "Grouped Issues Test Agent", testRuntimeID, testUserID).Scan(&agentID); err != nil {
		t.Fatalf("create agent: %v", err)
	}
	t.Cleanup(func() {
		_, _ = testPool.Exec(context.Background(), `DELETE FROM agent WHERE id = $1`, agentID)
	})

	createIssue := func(title, executorType, executorID string, position float64, startDate *time.Time, stage *int32) string {
		t.Helper()
		var number int32
		if err := testPool.QueryRow(ctx, `
			UPDATE workspace
			SET issue_counter = GREATEST(
				issue_counter,
				(SELECT COALESCE(MAX(number), 0) FROM issue WHERE workspace_id = $1)
			) + 1
			WHERE id = $1
			RETURNING issue_counter
		`, testWorkspaceID).Scan(&number); err != nil {
			t.Fatalf("next issue number: %v", err)
		}

		var id string
		if err := testPool.QueryRow(ctx, `
			INSERT INTO issue (
				workspace_id, title, description, status, priority,
				owner_type, owner_id, executor_type, executor_id, creator_type, creator_id,
				position, number, start_date, stage
			)
			VALUES ($1, $2, NULL, 'todo', 'none', $3, $4, $5, $6, 'member', $7, $8, $9, $10, $11)
			RETURNING id
		`, testWorkspaceID, title, nil, nil, executorType, executorID, testUserID, position, number, startDate, stage).Scan(&id); err != nil {
			t.Fatalf("create issue %q: %v", title, err)
		}
		t.Cleanup(func() {
			_, _ = testPool.Exec(context.Background(), `DELETE FROM issue WHERE id = $1`, id)
		})
		return id
	}

	stageTwo := int32(2)
	startDate := time.Date(2026, 3, 1, 0, 0, 0, 0, time.UTC)
	createIssue("Grouped executor one", "agent", agentID, 1, &startDate, &stageTwo)
	createIssue("Grouped executor two", "agent", agentID, 2, nil, nil)
	createIssue("Grouped executor three", "agent", agentID, 3, nil, nil)
	createIssue("Grouped executor four", "agent", agentID, 4, nil, nil)

	path := fmt.Sprintf(
		"/api/issues/grouped?workspace_id=%s&group_by=executor&statuses=todo&limit=2&executor_filters=agent:%s",
		testWorkspaceID,
		agentID,
	)
	w := httptest.NewRecorder()
	testHandler.ListGroupedIssues(w, newRequest("GET", path, nil))
	if w.Code != http.StatusOK {
		t.Fatalf("ListGroupedIssues: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp GroupedIssuesResponse
	if err := json.NewDecoder(w.Body).Decode(&resp); err != nil {
		t.Fatalf("decode grouped response: %v", err)
	}

	agentGroupID := "executor:agent:" + agentID
	groups := map[string]IssueExecutorGroupResponse{}
	for _, group := range resp.Groups {
		groups[group.ID] = group
	}

	agentGroup, ok := groups[agentGroupID]
	if !ok {
		t.Fatalf("missing agent group %s in %#v", agentGroupID, resp.Groups)
	}
	if agentGroup.Total != 4 || len(agentGroup.Issues) != 2 {
		t.Fatalf("agent group total/page mismatch: total=%d len=%d", agentGroup.Total, len(agentGroup.Issues))
	}
	if agentGroup.Issues[0].Title != "Grouped executor one" || agentGroup.Issues[1].Title != "Grouped executor two" {
		t.Fatalf("executor group order mismatch: %#v", agentGroup.Issues)
	}
	if agentGroup.Issues[0].Stage == nil || *agentGroup.Issues[0].Stage != stageTwo {
		t.Fatalf("executor group first issue stage = %#v, want %d", agentGroup.Issues[0].Stage, stageTwo)
	}
	if agentGroup.Issues[0].StartDate == nil || *agentGroup.Issues[0].StartDate != "2026-03-01" {
		t.Fatalf("executor group first issue start_date = %#v, want 2026-03-01", agentGroup.Issues[0].StartDate)
	}

	nextPath := fmt.Sprintf(
		"/api/issues/grouped?workspace_id=%s&group_by=executor&statuses=todo&limit=2&offset=2&group_executor_type=agent&group_executor_id=%s",
		testWorkspaceID,
		agentID,
	)
	next := httptest.NewRecorder()
	testHandler.ListGroupedIssues(next, newRequest("GET", nextPath, nil))
	if next.Code != http.StatusOK {
		t.Fatalf("ListGroupedIssues next page: expected 200, got %d: %s", next.Code, next.Body.String())
	}

	var nextResp GroupedIssuesResponse
	if err := json.NewDecoder(next.Body).Decode(&nextResp); err != nil {
		t.Fatalf("decode next grouped response: %v", err)
	}
	if len(nextResp.Groups) != 1 {
		t.Fatalf("expected one next-page group, got %#v", nextResp.Groups)
	}
	if nextResp.Groups[0].ID != agentGroupID || nextResp.Groups[0].Total != 4 || len(nextResp.Groups[0].Issues) != 2 {
		t.Fatalf("unexpected next-page group: %#v", nextResp.Groups[0])
	}
	if nextResp.Groups[0].Issues[0].Title != "Grouped executor three" || nextResp.Groups[0].Issues[1].Title != "Grouped executor four" {
		t.Fatalf("unexpected next-page issue: %#v", nextResp.Groups[0].Issues[0])
	}
}
