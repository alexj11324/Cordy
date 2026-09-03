package handler

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
	"github.com/patchbay-ai/patchbay/server/internal/entitlement"
	"github.com/patchbay-ai/patchbay/server/internal/entitlement/entitlementtest"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
)

func TestAdmitHostedWorkspaceOwnership(t *testing.T) {
	limit := int64(2)
	limitThree := int64(3)
	tests := []struct {
		name       string
		owned      int64
		policy     hostedWorkspacePolicy
		wantStatus int
		wantCode   string
	}{
		{"free allowance", 1, unavailableHostedWorkspacePolicy(), 0, ""},
		{"observe bypass", 2, hostedWorkspacePolicy{action: entitlement.ActionObserve, limit: &limit}, 0, ""},
		{"unlimited", 20, hostedWorkspacePolicy{action: entitlement.ActionEnforce}, 0, ""},
		{"below limit", 2, hostedWorkspacePolicy{action: entitlement.ActionEnforce, limit: &limitThree}, 0, ""},
		{"at limit", 2, hostedWorkspacePolicy{action: entitlement.ActionEnforce, limit: &limit}, http.StatusForbidden, hostedWorkspaceLimitCode},
		{"policy unavailable", 2, unavailableHostedWorkspacePolicy(), http.StatusServiceUnavailable, hostedWorkspaceQuotaCode},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			status, code, _ := admitHostedWorkspaceOwnership(test.owned, test.policy)
			if status != test.wantStatus || code != test.wantCode {
				t.Fatalf("admission = (%d, %q), want (%d, %q)", status, code, test.wantStatus, test.wantCode)
			}
		})
	}
}

func TestCreateWorkspaceGuestLimitSerializesConcurrentRequests(t *testing.T) {
	if testHandler == nil {
		t.Skip("database not available")
	}
	t.Setenv("PATCHBAY_APP_URL", "")
	t.Setenv("FRONTEND_ORIGIN", "")
	ctx := context.Background()
	userID := uuid.New()
	key := uuid.NewString()
	firstSlug := "guest-first-" + key
	secondSlug := "guest-second-" + key
	if _, err := testPool.Exec(ctx, `INSERT INTO "user" (id, name, email, is_guest) VALUES ($1, 'Guest quota user', $2, true)`, userID, fmt.Sprintf("guest-%s@example.test", key)); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_, _ = testPool.Exec(context.Background(), `
DELETE FROM issue_status WHERE workspace_id IN (SELECT id FROM workspace WHERE slug = ANY($1));
DELETE FROM member WHERE user_id = $2;
DELETE FROM workspace WHERE slug = ANY($1);
DELETE FROM "user" WHERE id = $2`, []string{firstSlug, secondSlug}, userID)
	})

	start := make(chan struct{})
	statuses := make(chan int, 2)
	var workers sync.WaitGroup
	for index, slug := range []string{firstSlug, secondSlug} {
		workers.Add(1)
		go func(index int, slug string) {
			defer workers.Done()
			<-start
			req := newRequestAs(userID.String(), http.MethodPost, "/api/workspaces", map[string]any{"name": fmt.Sprintf("Guest %d", index), "slug": slug})
			recorder := httptest.NewRecorder()
			testHandler.CreateWorkspace(recorder, req)
			statuses <- recorder.Code
		}(index, slug)
	}
	close(start)
	workers.Wait()
	close(statuses)
	counts := map[int]int{}
	for status := range statuses {
		counts[status]++
	}
	if counts[http.StatusCreated] != 1 || counts[http.StatusForbidden] != 1 {
		t.Fatalf("statuses = %#v, want one 201 and one 403", counts)
	}
}

func TestCreateWorkspaceHostedLimitSerializesConcurrentOwnership(t *testing.T) {
	if testHandler == nil {
		t.Skip("database not available")
	}
	t.Setenv("PATCHBAY_APP_URL", "https://patchbay.aspectlylabs.com")
	ctx := context.Background()
	userID := uuid.New()
	sourceWorkspaceID := uuid.New()
	key := uuid.NewString()
	sourceSlug := "hosted-source-" + key
	firstSlug := "hosted-first-" + key
	secondSlug := "hosted-second-" + key
	_, err := testPool.Exec(ctx, `
INSERT INTO "user" (id, name, email) VALUES ($1, 'Hosted quota user', $2);
INSERT INTO workspace (id, name, slug, issue_prefix) VALUES ($3, 'Hosted source', $4, 'HOST');
INSERT INTO member (workspace_id, user_id, role) VALUES ($3, $1, 'owner')`, userID, fmt.Sprintf("hosted-%s@example.test", key), sourceWorkspaceID, sourceSlug)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_, _ = testPool.Exec(context.Background(), `
DELETE FROM issue_status WHERE workspace_id IN (SELECT id FROM workspace WHERE slug = ANY($1));
DELETE FROM member WHERE user_id = $2;
DELETE FROM workspace WHERE slug = ANY($1);
DELETE FROM "user" WHERE id = $2`, []string{sourceSlug, firstSlug, secondSlug}, userID)
	})
	limit := 2
	stub := entitlementtest.New()
	stub.Set(sourceWorkspaceID, entitlement.GateHostedWorkspaceLimit, entitlement.Decision{Gate: entitlement.Gate{Action: entitlement.ActionEnforce, Limit: &limit}})
	previous := testHandler.Entitlements
	testHandler.Entitlements = stub
	t.Cleanup(func() { testHandler.Entitlements = previous })

	start := make(chan struct{})
	statuses := make(chan int, 2)
	var workers sync.WaitGroup
	for index, slug := range []string{firstSlug, secondSlug} {
		workers.Add(1)
		go func(index int, slug string) {
			defer workers.Done()
			<-start
			req := newRequestAs(userID.String(), http.MethodPost, "/api/workspaces", map[string]any{"name": fmt.Sprintf("Hosted %d", index), "slug": slug})
			recorder := httptest.NewRecorder()
			testHandler.CreateWorkspace(recorder, req)
			statuses <- recorder.Code
		}(index, slug)
	}
	close(start)
	workers.Wait()
	close(statuses)
	counts := map[int]int{}
	for status := range statuses {
		counts[status]++
	}
	if counts[http.StatusCreated] != 1 || counts[http.StatusForbidden] != 1 {
		t.Fatalf("statuses = %#v, want one 201 and one 403", counts)
	}
	var owned int64
	if err := testPool.QueryRow(ctx, `SELECT count(*)::bigint FROM member WHERE user_id = $1 AND role = 'owner'`, userID).Scan(&owned); err != nil || owned != 2 {
		t.Fatalf("owned workspaces = %d, err = %v; want 2", owned, err)
	}
}

func TestUpdateMemberHostedLimitBlocksOwnerPromotion(t *testing.T) {
	if testHandler == nil {
		t.Skip("database not available")
	}
	t.Setenv("PATCHBAY_APP_URL", "https://patchbay.aspectlylabs.com")
	ctx := context.Background()
	targetUserID := uuid.New()
	sourceOne := uuid.New()
	sourceTwo := uuid.New()
	targetWorkspaceID := uuid.New()
	targetMemberID := uuid.New()
	key := uuid.NewString()
	slugs := []string{"promotion-one-" + key, "promotion-two-" + key, "promotion-target-" + key}
	_, err := testPool.Exec(ctx, `
INSERT INTO "user" (id, name, email) VALUES ($1, 'Promotion target', $2);
INSERT INTO workspace (id, name, slug, issue_prefix) VALUES
  ($3, 'Promotion one', $6, 'PRO1'),
  ($4, 'Promotion two', $7, 'PRO2'),
  ($5, 'Promotion target', $8, 'PRO3');
INSERT INTO member (workspace_id, user_id, role) VALUES
  ($3, $1, 'owner'),
  ($4, $1, 'owner'),
  ($5, $9, 'owner');
INSERT INTO member (id, workspace_id, user_id, role) VALUES ($10, $5, $1, 'member')`, targetUserID, fmt.Sprintf("promotion-%s@example.test", key), sourceOne, sourceTwo, targetWorkspaceID, slugs[0], slugs[1], slugs[2], testUserID, targetMemberID)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_, _ = testPool.Exec(context.Background(), `
DELETE FROM member WHERE workspace_id = ANY($1);
DELETE FROM workspace WHERE id = ANY($1);
DELETE FROM "user" WHERE id = $2`, []uuid.UUID{sourceOne, sourceTwo, targetWorkspaceID}, targetUserID)
	})
	limit := 2
	stub := entitlementtest.New()
	stub.Set(sourceOne, entitlement.GateHostedWorkspaceLimit, entitlement.Decision{Gate: entitlement.Gate{Action: entitlement.ActionEnforce, Limit: &limit}})
	previous := testHandler.Entitlements
	testHandler.Entitlements = stub
	t.Cleanup(func() { testHandler.Entitlements = previous })

	path := "/api/workspaces/" + targetWorkspaceID.String() + "/members/" + targetMemberID.String()
	req := newRequestAs(testUserID, http.MethodPatch, path, map[string]any{"role": "owner"})
	routeContext := chi.NewRouteContext()
	routeContext.URLParams.Add("id", targetWorkspaceID.String())
	routeContext.URLParams.Add("memberId", targetMemberID.String())
	req = req.WithContext(context.WithValue(req.Context(), chi.RouteCtxKey, routeContext))
	recorder := testutil.Call(t, testHandler.UpdateMember, req).Want(http.StatusForbidden)
	if !strings.Contains(recorder.Body.String(), hostedWorkspaceLimitCode) {
		t.Fatalf("response = %s", recorder.Body.String())
	}
	var role string
	if err := testPool.QueryRow(ctx, `SELECT role FROM member WHERE id = $1`, targetMemberID).Scan(&role); err != nil || role != "member" {
		t.Fatalf("role = %q, err = %v; want member", role, err)
	}
}
