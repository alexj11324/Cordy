package handler

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/linear"
	"github.com/orvilo-ai/orvilo/server/internal/middleware"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	"github.com/orvilo-ai/orvilo/server/internal/util/secretbox"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

type linearSyncWorkerSpy struct {
	wakes        int
	resolved     bool
	workspaceID  pgtype.UUID
	conflictID   pgtype.UUID
	actorID      pgtype.UUID
	resolution   string
	manual       any
	conflict     db.LinearSyncConflict
	issue        db.Issue
	resolveError error
}

func (*linearSyncWorkerSpy) Run(context.Context) {}

func (s *linearSyncWorkerSpy) Wake() { s.wakes++ }

func (*linearSyncWorkerSpy) AccessToken(context.Context, pgtype.UUID) (string, error) {
	return "access", nil
}

func (s *linearSyncWorkerSpy) ResolveConflict(_ context.Context, workspaceID, conflictID, actorID pgtype.UUID, resolution string, manual any) (db.LinearSyncConflict, db.Issue, error) {
	s.resolved = true
	s.workspaceID = workspaceID
	s.conflictID = conflictID
	s.actorID = actorID
	s.resolution = resolution
	s.manual = manual
	return s.conflict, s.issue, s.resolveError
}

type linearRevokeAPI struct {
	linear.API
	revoked []string
	err     error
}

func (a *linearRevokeAPI) RevokeToken(_ context.Context, token, _, _ string) error {
	a.revoked = append(a.revoked, token)
	return a.err
}

type linearHTTPFixture struct {
	box            *secretbox.Box
	connectionID   string
	organizationID string
}

func setupLinearHTTPFixture(t *testing.T) linearHTTPFixture {
	t.Helper()
	if testPool == nil {
		t.Skip("database not available")
	}
	box, err := secretbox.New(make([]byte, secretbox.KeySize))
	if err != nil {
		t.Fatal(err)
	}
	sealedAccess, err := box.Seal([]byte("access"))
	if err != nil {
		t.Fatal(err)
	}
	sealedRefresh, err := box.Seal([]byte("refresh"))
	if err != nil {
		t.Fatal(err)
	}
	organizationID := "org-" + uuid.NewString()
	connectionID := dbfx.Insert(t, "linear_connection", testutil.Cols{
		"workspace_id":            testWorkspaceID,
		"organization_id":         organizationID,
		"organization_name":       "Test org",
		"actor_id":                "actor",
		"access_token_encrypted":  sealedAccess,
		"refresh_token_encrypted": sealedRefresh,
		"token_expires_at":        testutil.Raw("now() + interval '1 hour'"),
		"scopes":                  testutil.Raw("'[]'::jsonb"),
		"created_by_id":           testUserID,
	})
	// Webhook rows are created by the handler rather than the fixture, so remove
	// them before the fixture deletes the connection.
	dbfx.Cleanup(t, `DELETE FROM linear_sync_inbox WHERE connection_id=$1`, connectionID)
	return linearHTTPFixture{box: box, connectionID: connectionID, organizationID: organizationID}
}

func linearHTTPHandler(f linearHTTPFixture, worker LinearSyncWorker, api linear.API) *Handler {
	return &Handler{
		Queries:             db.New(testPool),
		DB:                  testPool,
		TxStarter:           testPool,
		FeatureFlags:        linearTestFlags(true),
		LinearSecretBox:     f.box,
		LinearClientID:      "client",
		LinearClientSecret:  "secret",
		LinearWebhookSecret: "webhook-secret",
		LinearWorker:        worker,
		LinearProvider:      api,
	}
}

// The webhook endpoint is the wake path: it persists the delivery, dedupes
// redeliveries on the provider's own delivery id, and nudges the sync worker.
func TestHandleLinearWebhookPersistsDedupesAndWakesWorker(t *testing.T) {
	f := setupLinearHTTPFixture(t)
	worker := &linearSyncWorkerSpy{}
	h := linearHTTPHandler(f, worker, nil)
	timestamp := time.Now().UnixMilli()
	body, err := json.Marshal(map[string]any{
		"type": "Issue", "action": "update", "organizationId": f.organizationID,
		"webhookId":        "linear-hook-1",
		"webhookTimestamp": timestamp,
		"data":             map[string]any{"id": "40000000-0000-0000-0000-000000000004"},
	})
	if err != nil {
		t.Fatal(err)
	}
	send := func() *httptest.ResponseRecorder {
		request := httptest.NewRequest(http.MethodPost, "/api/webhooks/linear", strings.NewReader(string(body)))
		request.Header.Set("Linear-Signature", linearTestSignature(t, "webhook-secret", body))
		request.Header.Set("Linear-Timestamp", fmt.Sprint(timestamp))
		request.Header.Set("Linear-Delivery", "delivery-1")
		recorder := httptest.NewRecorder()
		h.HandleLinearWebhook(recorder, request)
		return recorder
	}

	first := send()
	if first.Code != http.StatusOK {
		t.Fatalf("status = %d body = %s", first.Code, first.Body.String())
	}
	if worker.wakes != 1 {
		t.Fatalf("worker wakes = %d, want 1", worker.wakes)
	}
	replay := send()
	if replay.Code != http.StatusOK || !strings.Contains(replay.Body.String(), `"duplicate":true`) {
		t.Fatalf("redelivery status = %d body = %s", replay.Code, replay.Body.String())
	}
	if worker.wakes != 1 {
		t.Fatalf("duplicate webhook woke worker; wakes = %d", worker.wakes)
	}
	var stored int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_inbox WHERE connection_id=$1 AND delivery_id='delivery-1'`, f.connectionID).Scan(&stored); err != nil {
		t.Fatal(err)
	}
	if stored != 1 {
		t.Fatalf("redelivery stored %d rows", stored)
	}
}

func TestDisconnectLinearUsesProviderAndPreservesSyncHistory(t *testing.T) {
	f := setupLinearHTTPFixture(t)
	projectID := dbfx.Project(t, "Linear disconnect project")
	issueID := dbfx.Issue(t, "Linked", testutil.Cols{"project_id": projectID})
	bindingID := dbfx.Insert(t, "linear_project_binding", testutil.Cols{
		"workspace_id":            testWorkspaceID,
		"connection_id":           f.connectionID,
		"orvilo_project_id":       projectID,
		"linear_project_id":       "linear-project",
		"linear_team_id":          "linear-team",
		"status":                  "active",
		"sync_mode":               "two_way",
		"initial_source_of_truth": "linear",
		"created_by_id":           testUserID,
	})
	linkID := dbfx.Insert(t, "linear_issue_link", testutil.Cols{
		"workspace_id":         testWorkspaceID,
		"binding_id":           bindingID,
		"orvilo_issue_id":      issueID,
		"linear_issue_id":      "linear-issue",
		"linear_identifier":    "ENG-1",
		"last_common_snapshot": testutil.Raw("'{}'::jsonb"),
	})
	api := &linearRevokeAPI{}
	h := linearHTTPHandler(f, nil, api)
	request := httptest.NewRequest(http.MethodDelete, "/api/workspaces/"+testWorkspaceID+"/linear/connection", nil)
	routeCtx := chi.NewRouteContext()
	routeCtx.URLParams.Add("id", testWorkspaceID)
	request = request.WithContext(context.WithValue(request.Context(), chi.RouteCtxKey, routeCtx))
	recorder := httptest.NewRecorder()

	h.DisconnectLinear(recorder, request)

	if recorder.Code != http.StatusNoContent {
		t.Fatalf("status = %d body = %s", recorder.Code, recorder.Body.String())
	}
	if len(api.revoked) != 1 || api.revoked[0] != "access" {
		t.Fatalf("revoked = %v, want the stored access token", api.revoked)
	}
	var status string
	if err := testPool.QueryRow(context.Background(), `SELECT status FROM linear_connection WHERE id=$1`, f.connectionID).Scan(&status); err != nil {
		t.Fatal(err)
	}
	if status != "revoked" {
		t.Fatalf("connection status = %q, want revoked", status)
	}
	var bindings, links, issues int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_project_binding WHERE id=$1`, bindingID).Scan(&bindings); err != nil {
		t.Fatal(err)
	}
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_issue_link WHERE id=$1`, linkID).Scan(&links); err != nil {
		t.Fatal(err)
	}
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM issue WHERE id=$1`, issueID).Scan(&issues); err != nil {
		t.Fatal(err)
	}
	if bindings != 1 || links != 1 || issues != 1 {
		t.Fatalf("disconnect removed audit state: bindings=%d links=%d issues=%d", bindings, links, issues)
	}
}

func TestResolveLinearSyncConflictDelegatesToWorker(t *testing.T) {
	workspaceID := parseUUID("80000000-0000-0000-0000-000000000008")
	conflictID := parseUUID("90000000-0000-0000-0000-000000000009")
	actorID := parseUUID("a0000000-0000-0000-0000-00000000000a")
	worker := &linearSyncWorkerSpy{conflict: db.LinearSyncConflict{
		ID:            conflictID,
		WorkspaceID:   workspaceID,
		BindingID:     parseUUID("b0000000-0000-0000-0000-00000000000b"),
		LinkID:        parseUUID("c0000000-0000-0000-0000-00000000000c"),
		OrviloIssueID: parseUUID("d0000000-0000-0000-0000-00000000000d"),
		LinearIssueID: "linear-issue",
		Field:         "title",
		Status:        "resolved",
		Resolution:    pgtype.Text{String: "manual", Valid: true},
		ResolvedValue: []byte(`"chosen title"`),
		ResolvedByID:  actorID,
	}}
	h := &Handler{FeatureFlags: linearTestFlags(true), LinearWorker: worker}
	request := httptest.NewRequest(http.MethodPost, "/", strings.NewReader(`{"resolution":"manual","manual_value":"chosen title"}`))
	routeCtx := chi.NewRouteContext()
	routeCtx.URLParams.Add("id", uuidToString(workspaceID))
	routeCtx.URLParams.Add("conflictId", uuidToString(conflictID))
	request = request.WithContext(context.WithValue(request.Context(), chi.RouteCtxKey, routeCtx))
	request = request.WithContext(middleware.SetMemberContext(request.Context(), uuidToString(workspaceID), db.Member{UserID: actorID}))
	recorder := httptest.NewRecorder()

	h.ResolveLinearSyncConflict(recorder, request)

	if recorder.Code != http.StatusOK {
		t.Fatalf("status = %d body = %s", recorder.Code, recorder.Body.String())
	}
	if !worker.resolved || worker.workspaceID != workspaceID || worker.conflictID != conflictID || worker.actorID != actorID || worker.resolution != "manual" || worker.manual != "chosen title" {
		t.Fatalf("worker call = resolved %v workspace %v conflict %v actor %v resolution %q manual %#v", worker.resolved, worker.workspaceID, worker.conflictID, worker.actorID, worker.resolution, worker.manual)
	}
}

func TestResolveLinearSyncConflictRejectsUnavailableWorker(t *testing.T) {
	request := httptest.NewRequest(http.MethodPost, "/", strings.NewReader(`{"resolution":"local"}`))
	routeCtx := chi.NewRouteContext()
	routeCtx.URLParams.Add("id", "80000000-0000-0000-0000-000000000008")
	routeCtx.URLParams.Add("conflictId", "90000000-0000-0000-0000-000000000009")
	request = request.WithContext(context.WithValue(request.Context(), chi.RouteCtxKey, routeCtx))
	recorder := httptest.NewRecorder()

	(&Handler{FeatureFlags: linearTestFlags(true)}).ResolveLinearSyncConflict(recorder, request)

	if recorder.Code != http.StatusServiceUnavailable {
		t.Fatalf("status = %d body = %s", recorder.Code, recorder.Body.String())
	}
}
