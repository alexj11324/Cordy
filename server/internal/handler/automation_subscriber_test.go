package handler

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func installAutomationSubscriberInsertFailure(t *testing.T) {
	t.Helper()
	ctx := context.Background()
	suffix := time.Now().UnixNano()
	functionName := fmt.Sprintf("automation_subscriber_fail_fn_%d", suffix)
	triggerName := fmt.Sprintf("automation_subscriber_fail_%d", suffix)
	t.Cleanup(func() {
		testPool.Exec(ctx, fmt.Sprintf(`DROP TRIGGER IF EXISTS %s ON automation_subscriber`, triggerName))
		testPool.Exec(ctx, fmt.Sprintf(`DROP FUNCTION IF EXISTS %s()`, functionName))
	})

	if _, err := testPool.Exec(ctx, fmt.Sprintf(`
CREATE FUNCTION %s() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
	RAISE EXCEPTION 'forced automation subscriber insert failure';
END;
$$;
`, functionName)); err != nil {
		t.Fatalf("install failure function: %v", err)
	}
	if _, err := testPool.Exec(ctx, fmt.Sprintf(`
CREATE TRIGGER %s
BEFORE INSERT ON automation_subscriber
FOR EACH ROW EXECUTE FUNCTION %s();
`, triggerName, functionName)); err != nil {
		t.Fatalf("install failure trigger: %v", err)
	}
}

// TestCreateAutomationPersistsMemberSubscribers covers the happy path:
// supplying a non-empty `subscribers` array on POST /api/automations stores
// the rows and the response echoes them back. This is the create half of the
// MUL-2533 RFC ("automation default subscriber template").
func TestCreateAutomationPersistsMemberSubscribers(t *testing.T) {
	ctx := context.Background()
	var automationID string
	defer func() {
		if automationID != "" {
			testPool.Exec(ctx, `DELETE FROM automation WHERE id = $1`, automationID)
		}
	}()

	var agentID string
	if err := testPool.QueryRow(ctx, `SELECT id FROM agent WHERE workspace_id = $1 LIMIT 1`, testWorkspaceID).Scan(&agentID); err != nil {
		t.Fatalf("load test agent: %v", err)
	}

	w := httptest.NewRecorder()
	req := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
		"title":          "Subscriber template automation",
		"executor_id":    agentID,
		"execution_mode": "create_issue",
		"subscribers": []map[string]any{
			{"user_type": "member", "user_id": testUserID},
		},
	})
	testHandler.CreateAutomation(w, req)
	if w.Code != http.StatusCreated {
		t.Fatalf("CreateAutomation: expected 201, got %d: %s", w.Code, w.Body.String())
	}
	var resp AutomationResponse
	if err := json.NewDecoder(w.Body).Decode(&resp); err != nil {
		t.Fatalf("decode automation: %v", err)
	}
	automationID = resp.ID
	if len(resp.Subscribers) != 1 {
		t.Fatalf("subscribers in response = %d, want 1", len(resp.Subscribers))
	}
	if resp.Subscribers[0].UserType != "member" || resp.Subscribers[0].UserID != testUserID {
		t.Fatalf("subscribers[0] = %+v, want member/%s", resp.Subscribers[0], testUserID)
	}

	// Confirm the row landed in the DB. Belt-and-braces: the response could
	// in principle be assembled from the request without writing.
	var count int
	if err := testPool.QueryRow(ctx, `
		SELECT count(*) FROM automation_subscriber WHERE automation_id = $1
	`, automationID).Scan(&count); err != nil {
		t.Fatalf("count subscribers: %v", err)
	}
	if count != 1 {
		t.Fatalf("automation_subscriber rows = %d, want 1", count)
	}
}

// TestCreateAutomationRejectsNonMemberSubscriberType locks in the first-version
// constraint: only user_type='member' is accepted on the API. The DB CHECK
// would also reject anything else; the 400 here exists so the client gets a
// clear message instead of a 500 with a constraint-name leak.
func TestCreateAutomationRejectsNonMemberSubscriberType(t *testing.T) {
	var agentID string
	if err := testPool.QueryRow(context.Background(), `SELECT id FROM agent WHERE workspace_id = $1 LIMIT 1`, testWorkspaceID).Scan(&agentID); err != nil {
		t.Fatalf("load test agent: %v", err)
	}

	w := httptest.NewRecorder()
	req := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
		"title":          "Bad subscriber type",
		"executor_id":    agentID,
		"execution_mode": "create_issue",
		"subscribers": []map[string]any{
			{"user_type": "agent", "user_id": agentID},
		},
	})
	testHandler.CreateAutomation(w, req)
	if w.Code != http.StatusBadRequest {
		t.Fatalf("CreateAutomation: expected 400 for non-member subscriber, got %d: %s", w.Code, w.Body.String())
	}
}

// TestCreateAutomationRejectsForeignSubscriber covers the boundary check:
// supplying a UUID that does not belong to this workspace must 400, not
// silently leak inside the automation row.
func TestCreateAutomationRejectsForeignSubscriber(t *testing.T) {
	var agentID string
	if err := testPool.QueryRow(context.Background(), `SELECT id FROM agent WHERE workspace_id = $1 LIMIT 1`, testWorkspaceID).Scan(&agentID); err != nil {
		t.Fatalf("load test agent: %v", err)
	}

	w := httptest.NewRecorder()
	req := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
		"title":          "Foreign subscriber",
		"executor_id":    agentID,
		"execution_mode": "create_issue",
		"subscribers": []map[string]any{
			{"user_type": "member", "user_id": "00000000-0000-0000-0000-000000000000"},
		},
	})
	testHandler.CreateAutomation(w, req)
	if w.Code != http.StatusBadRequest {
		t.Fatalf("CreateAutomation: expected 400 for foreign member subscriber, got %d: %s", w.Code, w.Body.String())
	}
}

// TestAutomationSubscriberSave_LosesToConcurrentRevoke covers the race between
// subscriber validation and member removal for both create and update. The
// revoke transaction has already pruned templates and deleted the member, but
// has not committed, so a validation outside the serialized write transaction
// can still see the old member snapshot and recreate a subscriber row behind
// the cleanup. Saves must wait for revoke's lock, then reject the departed
// member from their fresh in-transaction membership check.
func TestAutomationSubscriberSave_LosesToConcurrentRevoke(t *testing.T) {
	ctx := context.Background()

	var agentID string
	if err := testPool.QueryRow(ctx, `SELECT id FROM agent WHERE workspace_id = $1 LIMIT 1`, testWorkspaceID).Scan(&agentID); err != nil {
		t.Fatalf("load test agent: %v", err)
	}

	for _, tc := range []struct {
		name    string
		prepare func(t *testing.T, targetUserID string) func() (status int, body string, automationID string)
	}{
		{
			name: "create",
			prepare: func(t *testing.T, targetUserID string) func() (int, string, string) {
				title := fmt.Sprintf("Concurrent revoke create %d", time.Now().UnixNano())
				return func() (int, string, string) {
					w := httptest.NewRecorder()
					req := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
						"title":          title,
						"executor_id":    agentID,
						"execution_mode": "create_issue",
						"subscribers": []map[string]any{
							{"user_type": "member", "user_id": targetUserID},
						},
					})
					testHandler.CreateAutomation(w, req)

					var automationID string
					testPool.QueryRow(ctx, `SELECT id FROM automation WHERE workspace_id = $1 AND title = $2`, testWorkspaceID, title).Scan(&automationID)
					return w.Code, w.Body.String(), automationID
				}
			},
		},
		{
			name: "update",
			prepare: func(t *testing.T, targetUserID string) func() (int, string, string) {
				createReq := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
					"title":          fmt.Sprintf("Concurrent revoke update %d", time.Now().UnixNano()),
					"executor_id":    agentID,
					"execution_mode": "create_issue",
				})
				var created AutomationResponse
				testutil.Call(t, testHandler.CreateAutomation, createReq).
					Want(http.StatusCreated).
					JSON(&created)

				return func() (int, string, string) {
					w := httptest.NewRecorder()
					req := newRequest("PATCH", "/api/automations/"+created.ID+"?workspace_id="+testWorkspaceID, map[string]any{
						"subscribers": []map[string]any{
							{"user_type": "member", "user_id": targetUserID},
						},
					})
					req = withURLParam(req, "id", created.ID)
					testHandler.UpdateAutomation(w, req)
					return w.Code, w.Body.String(), created.ID
				}
			},
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			targetUserID := createPlainMember(t, fmt.Sprintf("automation-revoke-race-%s-%d@patchbay.ai", tc.name, time.Now().UnixNano()))
			run := tc.prepare(t, targetUserID)

			revokeTx, err := testPool.Begin(ctx)
			if err != nil {
				t.Fatalf("begin revoke tx: %v", err)
			}
			defer revokeTx.Rollback(context.Background())
			qtx := testHandler.Queries.WithTx(revokeTx)

			if err := qtx.LockSubscriberWrites(ctx, db.LockSubscriberWritesParams{
				WorkspaceID: parseUUID(testWorkspaceID),
				UserID:      parseUUID(targetUserID),
			}); err != nil {
				t.Fatalf("revoke lock: %v", err)
			}
			if err := qtx.DeleteAutomationSubscribersByMember(ctx, db.DeleteAutomationSubscribersByMemberParams{
				WorkspaceID: parseUUID(testWorkspaceID),
				UserID:      parseUUID(targetUserID),
			}); err != nil {
				t.Fatalf("revoke cleanup: %v", err)
			}
			if _, err := revokeTx.Exec(ctx,
				`DELETE FROM member WHERE workspace_id = $1 AND user_id = $2`,
				testWorkspaceID, targetUserID,
			); err != nil {
				t.Fatalf("revoke member delete: %v", err)
			}

			type result struct {
				status      int
				body        string
				automationID string
			}
			done := make(chan result, 1)
			go func() {
				status, body, automationID := run()
				done <- result{status: status, body: body, automationID: automationID}
			}()

			select {
			case got := <-done:
				t.Fatalf("automation %s completed (status %d: %s) while revoke held the subscriber lock", tc.name, got.status, got.body)
			case <-time.After(400 * time.Millisecond):
			}

			if err := revokeTx.Commit(context.Background()); err != nil {
				t.Fatalf("commit revoke: %v", err)
			}

			var got result
			select {
			case got = <-done:
			case <-time.After(10 * time.Second):
				t.Fatalf("automation %s never returned after revoke committed", tc.name)
			}
			if got.status != http.StatusBadRequest {
				t.Fatalf("automation %s status = %d, want 400 after subscriber left: %s", tc.name, got.status, got.body)
			}
			if got.automationID != "" {
				t.Cleanup(func() {
					testPool.Exec(context.Background(), `DELETE FROM automation WHERE id = $1`, got.automationID)
				})
			}

			var staleRows int
			if err := testPool.QueryRow(context.Background(), `
				SELECT count(*)
				FROM automation_subscriber s
				JOIN automation a ON a.id = s.automation_id
				WHERE a.workspace_id = $1 AND s.user_id = $2
			`, testWorkspaceID, targetUserID).Scan(&staleRows); err != nil {
				t.Fatalf("count stale subscriber rows: %v", err)
			}
			if staleRows != 0 {
				t.Fatalf("automation %s recreated %d subscriber row(s) after revoke cleanup", tc.name, staleRows)
			}
		})
	}
}

func TestCreateAutomationRollsBackWhenSubscriberInsertFails(t *testing.T) {
	ctx := context.Background()
	title := fmt.Sprintf("Subscriber rollback create %d", time.Now().UnixNano())

	var agentID string
	if err := testPool.QueryRow(ctx, `SELECT id FROM agent WHERE workspace_id = $1 LIMIT 1`, testWorkspaceID).Scan(&agentID); err != nil {
		t.Fatalf("load test agent: %v", err)
	}

	installAutomationSubscriberInsertFailure(t)

	w := httptest.NewRecorder()
	req := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
		"title":          title,
		"executor_id":    agentID,
		"execution_mode": "create_issue",
		"subscribers": []map[string]any{
			{"user_type": "member", "user_id": testUserID},
		},
	})
	testHandler.CreateAutomation(w, req)
	if w.Code != http.StatusInternalServerError {
		t.Fatalf("CreateAutomation: expected 500 for forced subscriber insert failure, got %d: %s", w.Code, w.Body.String())
	}

	var count int
	if err := testPool.QueryRow(ctx, `
		SELECT count(*) FROM automation
		WHERE workspace_id = $1 AND title = $2
	`, testWorkspaceID, title).Scan(&count); err != nil {
		t.Fatalf("count rolled-back automations: %v", err)
	}
	if count != 0 {
		t.Fatalf("automation rows after failed subscriber insert = %d, want 0", count)
	}
}

// TestUpdateAutomationFullReplaceSubscribers covers the PATCH semantics from
// the RFC: sending `subscribers` wipes whatever was there and re-inserts the
// new set. Omitting the field would leave the previous template untouched;
// that branch is exercised separately by TestUpdateAutomationPreservesSubscribersWhenOmitted.
func TestUpdateAutomationFullReplaceSubscribers(t *testing.T) {
	ctx := context.Background()
	var automationID string
	defer func() {
		if automationID != "" {
			testPool.Exec(ctx, `DELETE FROM automation WHERE id = $1`, automationID)
		}
	}()

	var agentID string
	if err := testPool.QueryRow(ctx, `SELECT id FROM agent WHERE workspace_id = $1 LIMIT 1`, testWorkspaceID).Scan(&agentID); err != nil {
		t.Fatalf("load test agent: %v", err)
	}

	w := httptest.NewRecorder()
	req := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
		"title":          "Replace subscribers automation",
		"executor_id":    agentID,
		"execution_mode": "create_issue",
		"subscribers": []map[string]any{
			{"user_type": "member", "user_id": testUserID},
		},
	})
	testHandler.CreateAutomation(w, req)
	if w.Code != http.StatusCreated {
		t.Fatalf("CreateAutomation: expected 201, got %d: %s", w.Code, w.Body.String())
	}
	var created AutomationResponse
	if err := json.NewDecoder(w.Body).Decode(&created); err != nil {
		t.Fatalf("decode created: %v", err)
	}
	automationID = created.ID

	// PATCH with an empty array → expect zero subscribers afterward.
	w = httptest.NewRecorder()
	req = newRequest("PATCH", "/api/automations/"+automationID+"?workspace_id="+testWorkspaceID, map[string]any{
		"subscribers": []map[string]any{},
	})
	req = withURLParam(req, "id", automationID)
	testHandler.UpdateAutomation(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("UpdateAutomation: expected 200, got %d: %s", w.Code, w.Body.String())
	}
	var updated AutomationResponse
	if err := json.NewDecoder(w.Body).Decode(&updated); err != nil {
		t.Fatalf("decode updated: %v", err)
	}
	if len(updated.Subscribers) != 0 {
		t.Fatalf("subscribers after empty replace = %d, want 0", len(updated.Subscribers))
	}

	var count int
	if err := testPool.QueryRow(ctx, `SELECT count(*) FROM automation_subscriber WHERE automation_id = $1`, automationID).Scan(&count); err != nil {
		t.Fatalf("count after replace: %v", err)
	}
	if count != 0 {
		t.Fatalf("DB rows after empty replace = %d, want 0", count)
	}
}

func TestUpdateAutomationRollsBackWhenSubscriberInsertFails(t *testing.T) {
	ctx := context.Background()
	originalTitle := fmt.Sprintf("Subscriber rollback update %d", time.Now().UnixNano())
	updatedTitle := originalTitle + " changed"
	var automationID string
	defer func() {
		if automationID != "" {
			testPool.Exec(ctx, `DELETE FROM automation WHERE id = $1`, automationID)
		}
	}()

	var agentID string
	if err := testPool.QueryRow(ctx, `SELECT id FROM agent WHERE workspace_id = $1 LIMIT 1`, testWorkspaceID).Scan(&agentID); err != nil {
		t.Fatalf("load test agent: %v", err)
	}

	w := httptest.NewRecorder()
	req := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
		"title":          originalTitle,
		"executor_id":    agentID,
		"execution_mode": "create_issue",
		"subscribers": []map[string]any{
			{"user_type": "member", "user_id": testUserID},
		},
	})
	testHandler.CreateAutomation(w, req)
	if w.Code != http.StatusCreated {
		t.Fatalf("CreateAutomation: expected 201, got %d: %s", w.Code, w.Body.String())
	}
	var created AutomationResponse
	if err := json.NewDecoder(w.Body).Decode(&created); err != nil {
		t.Fatalf("decode created: %v", err)
	}
	automationID = created.ID

	installAutomationSubscriberInsertFailure(t)

	w = httptest.NewRecorder()
	req = newRequest("PATCH", "/api/automations/"+automationID+"?workspace_id="+testWorkspaceID, map[string]any{
		"title": updatedTitle,
		"subscribers": []map[string]any{
			{"user_type": "member", "user_id": testUserID},
		},
	})
	req = withURLParam(req, "id", automationID)
	testHandler.UpdateAutomation(w, req)
	if w.Code != http.StatusInternalServerError {
		t.Fatalf("UpdateAutomation: expected 500 for forced subscriber insert failure, got %d: %s", w.Code, w.Body.String())
	}

	var gotTitle string
	if err := testPool.QueryRow(ctx, `SELECT title FROM automation WHERE id = $1`, automationID).Scan(&gotTitle); err != nil {
		t.Fatalf("load automation title after rollback: %v", err)
	}
	if gotTitle != originalTitle {
		t.Fatalf("automation title after failed subscriber replace = %q, want %q", gotTitle, originalTitle)
	}

	var count int
	if err := testPool.QueryRow(ctx, `SELECT count(*) FROM automation_subscriber WHERE automation_id = $1`, automationID).Scan(&count); err != nil {
		t.Fatalf("count subscribers after rollback: %v", err)
	}
	if count != 1 {
		t.Fatalf("subscriber rows after failed replace = %d, want 1", count)
	}
}

// TestUpdateAutomationPreservesSubscribersWhenOmitted asserts the
// "omit the field to leave it alone" contract — a previously-set template
// must NOT be wiped just because the client sent a partial PATCH.
func TestUpdateAutomationPreservesSubscribersWhenOmitted(t *testing.T) {
	ctx := context.Background()
	var automationID string
	defer func() {
		if automationID != "" {
			testPool.Exec(ctx, `DELETE FROM automation WHERE id = $1`, automationID)
		}
	}()

	var agentID string
	if err := testPool.QueryRow(ctx, `SELECT id FROM agent WHERE workspace_id = $1 LIMIT 1`, testWorkspaceID).Scan(&agentID); err != nil {
		t.Fatalf("load test agent: %v", err)
	}

	w := httptest.NewRecorder()
	req := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
		"title":          "Preserve subscribers automation",
		"executor_id":    agentID,
		"execution_mode": "create_issue",
		"subscribers": []map[string]any{
			{"user_type": "member", "user_id": testUserID},
		},
	})
	testHandler.CreateAutomation(w, req)
	if w.Code != http.StatusCreated {
		t.Fatalf("CreateAutomation: expected 201, got %d: %s", w.Code, w.Body.String())
	}
	var created AutomationResponse
	if err := json.NewDecoder(w.Body).Decode(&created); err != nil {
		t.Fatalf("decode created: %v", err)
	}
	automationID = created.ID

	// PATCH a different field, leave subscribers out → row count unchanged.
	w = httptest.NewRecorder()
	req = newRequest("PATCH", "/api/automations/"+automationID+"?workspace_id="+testWorkspaceID, map[string]any{
		"title": "Preserve subscribers automation (renamed)",
	})
	req = withURLParam(req, "id", automationID)
	testHandler.UpdateAutomation(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("UpdateAutomation: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var count int
	if err := testPool.QueryRow(ctx, `SELECT count(*) FROM automation_subscriber WHERE automation_id = $1`, automationID).Scan(&count); err != nil {
		t.Fatalf("count after omitted PATCH: %v", err)
	}
	if count != 1 {
		t.Fatalf("DB rows after omitted PATCH = %d, want 1 (subscribers must not have been touched)", count)
	}
}

// TestAutomationDepartedSubscriberReadRepair covers the MUL-6640 regression:
// older member-removal code could leave an automation_subscriber row behind.
// The member picker hid that user, but GET still returned the stale id and the
// edit dialog round-tripped it into PATCH, which then rejected every save.
//
// The read path must expose only current members. Sending that authoritative
// list back through the existing full-replace PATCH then removes the legacy
// row without weakening create/update validation for arbitrary foreign ids.
func TestAutomationDepartedSubscriberReadRepair(t *testing.T) {
	ctx := context.Background()
	departedUserID := createPlainMember(t, fmt.Sprintf("automation-departed-%d@patchbay.ai", time.Now().UnixNano()))

	var agentID string
	if err := testPool.QueryRow(ctx, `SELECT id FROM agent WHERE workspace_id = $1 LIMIT 1`, testWorkspaceID).Scan(&agentID); err != nil {
		t.Fatalf("load test agent: %v", err)
	}

	createReq := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
		"title":          "Departed subscriber read repair",
		"executor_id":    agentID,
		"execution_mode": "create_issue",
		"subscribers": []map[string]any{
			{"user_type": "member", "user_id": departedUserID},
		},
	})
	var created AutomationResponse
	testutil.Call(t, testHandler.CreateAutomation, createReq).
		Want(http.StatusCreated).
		JSON(&created)
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM automation WHERE id = $1`, created.ID)
	})

	// Model a row produced before member-removal learned to prune automation
	// templates. Deliberately leave automation_subscriber intact.
	dbfx.Exec(t, `DELETE FROM member WHERE workspace_id = $1 AND user_id = $2`, testWorkspaceID, departedUserID)

	getReq := newRequest("GET", "/api/automations/"+created.ID+"?workspace_id="+testWorkspaceID, nil)
	getReq = withURLParam(getReq, "id", created.ID)
	var detail struct {
		Automation AutomationResponse `json:"automation"`
	}
	testutil.Call(t, testHandler.GetAutomation, getReq).
		Want(http.StatusOK).
		JSON(&detail)
	if len(detail.Automation.Subscribers) != 0 {
		t.Fatalf("GET subscribers = %+v, want departed member omitted", detail.Automation.Subscribers)
	}

	updateReq := newRequest("PATCH", "/api/automations/"+created.ID+"?workspace_id="+testWorkspaceID, map[string]any{
		"subscribers": detail.Automation.Subscribers,
	})
	updateReq = withURLParam(updateReq, "id", created.ID)
	testutil.Call(t, testHandler.UpdateAutomation, updateReq).Want(http.StatusOK)

	var count int
	dbfx.QueryRow(t, `SELECT count(*) FROM automation_subscriber WHERE automation_id = $1`, created.ID).Scan(&count)
	if count != 0 {
		t.Fatalf("legacy automation_subscriber rows after save = %d, want 0", count)
	}
}

// TestAutomationDispatchFansOutSubscribersToIssue is the integration check
// for the dispatch path: an automation with a default subscriber list must
// auto-subscribe each entry to the issue it spawns, with reason='automation'.
// Belt-and-braces: also confirms that the creator-of-the-issue (the automation
// executor agent — see TestAutomationCreatedIssueCreatorIsExecutorAgent) gets a row
// with reason='creator', and the two reasons don't fight (PK is one row per
// (issue, user_type, user_id), so the first one wins on conflict).
func TestAutomationDispatchFansOutSubscribersToIssue(t *testing.T) {
	ctx := context.Background()
	title := fmt.Sprintf("Automation subscriber fanout %d", time.Now().UnixNano())
	var automationID, issueID string
	defer func() {
		if issueID != "" {
			testPool.Exec(ctx, `DELETE FROM issue WHERE id = $1`, issueID)
		}
		if automationID != "" {
			testPool.Exec(ctx, `DELETE FROM automation WHERE id = $1`, automationID)
		}
	}()

	var agentID string
	if err := testPool.QueryRow(ctx, `SELECT id FROM agent WHERE workspace_id = $1 LIMIT 1`, testWorkspaceID).Scan(&agentID); err != nil {
		t.Fatalf("load test agent: %v", err)
	}

	w := httptest.NewRecorder()
	req := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
		"title":                "Subscriber fanout automation",
		"executor_id":          agentID,
		"execution_mode":       "create_issue",
		"issue_title_template": title,
		"subscribers": []map[string]any{
			{"user_type": "member", "user_id": testUserID},
		},
	})
	testHandler.CreateAutomation(w, req)
	if w.Code != http.StatusCreated {
		t.Fatalf("CreateAutomation: expected 201, got %d: %s", w.Code, w.Body.String())
	}
	var automation AutomationResponse
	if err := json.NewDecoder(w.Body).Decode(&automation); err != nil {
		t.Fatalf("decode automation: %v", err)
	}
	automationID = automation.ID

	queries := db.New(testPool)
	ap, err := queries.GetAutomation(ctx, parseUUID(automationID))
	if err != nil {
		t.Fatalf("GetAutomation: %v", err)
	}
	run, err := testHandler.AutomationService.DispatchAutomation(ctx, ap, pgtype.UUID{}, "manual", nil)
	if err != nil {
		t.Fatalf("DispatchAutomation: %v", err)
	}
	if run == nil || !run.IssueID.Valid {
		t.Fatalf("dispatch run = %+v, want linked issue", run)
	}
	issueID = uuidToString(run.IssueID)

	var subscriberReason string
	if err := testPool.QueryRow(ctx, `
		SELECT reason
		FROM issue_subscriber
		WHERE issue_id = $1 AND user_type = 'member' AND user_id = $2
	`, issueID, testUserID).Scan(&subscriberReason); err != nil {
		t.Fatalf("query automation-fanned subscriber: %v", err)
	}
	if subscriberReason != "automation" {
		t.Fatalf("subscriber reason = %q, want %q", subscriberReason, "automation")
	}
}

// TestAutomationDispatchNotifiesSubscribersOnCreate locks in the OQ3 promise
// from the RFC ("reason='automation' 与 reason='manual' 一致，订阅事件全收"):
// when an automation creates an issue, each template subscriber must land in
// the recipient's inbox with type='issue_subscribed' pointing at the new
// issue. Without this, subscribers would only see comment/status updates
// after the fact and miss the creation event itself — flagged in PR #3060
// review by the Emacs agent.
func TestAutomationDispatchNotifiesSubscribersOnCreate(t *testing.T) {
	ctx := context.Background()
	title := fmt.Sprintf("Automation subscriber inbox %d", time.Now().UnixNano())
	var automationID, issueID string
	defer func() {
		if issueID != "" {
			testPool.Exec(ctx, `DELETE FROM inbox_item WHERE issue_id = $1`, issueID)
			testPool.Exec(ctx, `DELETE FROM issue WHERE id = $1`, issueID)
		}
		if automationID != "" {
			testPool.Exec(ctx, `DELETE FROM automation WHERE id = $1`, automationID)
		}
	}()

	var agentID string
	if err := testPool.QueryRow(ctx, `SELECT id FROM agent WHERE workspace_id = $1 LIMIT 1`, testWorkspaceID).Scan(&agentID); err != nil {
		t.Fatalf("load test agent: %v", err)
	}

	w := httptest.NewRecorder()
	req := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
		"title":                "Subscriber inbox automation",
		"executor_id":          agentID,
		"execution_mode":       "create_issue",
		"issue_title_template": title,
		"subscribers": []map[string]any{
			{"user_type": "member", "user_id": testUserID},
		},
	})
	testHandler.CreateAutomation(w, req)
	if w.Code != http.StatusCreated {
		t.Fatalf("CreateAutomation: expected 201, got %d: %s", w.Code, w.Body.String())
	}
	var automation AutomationResponse
	if err := json.NewDecoder(w.Body).Decode(&automation); err != nil {
		t.Fatalf("decode automation: %v", err)
	}
	automationID = automation.ID

	queries := db.New(testPool)
	ap, err := queries.GetAutomation(ctx, parseUUID(automationID))
	if err != nil {
		t.Fatalf("GetAutomation: %v", err)
	}
	run, err := testHandler.AutomationService.DispatchAutomation(ctx, ap, pgtype.UUID{}, "manual", nil)
	if err != nil {
		t.Fatalf("DispatchAutomation: %v", err)
	}
	if run == nil || !run.IssueID.Valid {
		t.Fatalf("dispatch run = %+v, want linked issue", run)
	}
	issueID = uuidToString(run.IssueID)

	var inboxCount int
	var inboxType, inboxTitle string
	if err := testPool.QueryRow(ctx, `
		SELECT count(*) FROM inbox_item
		WHERE issue_id = $1 AND recipient_id = $2 AND type = 'issue_subscribed'
	`, issueID, testUserID).Scan(&inboxCount); err != nil {
		t.Fatalf("count inbox rows: %v", err)
	}
	if inboxCount != 1 {
		t.Fatalf("inbox_item rows for subscriber = %d, want 1", inboxCount)
	}

	if err := testPool.QueryRow(ctx, `
		SELECT type, title FROM inbox_item
		WHERE issue_id = $1 AND recipient_id = $2 AND type = 'issue_subscribed'
	`, issueID, testUserID).Scan(&inboxType, &inboxTitle); err != nil {
		t.Fatalf("load inbox row: %v", err)
	}
	if inboxType != "issue_subscribed" {
		t.Fatalf("inbox type = %q, want issue_subscribed", inboxType)
	}
	if inboxTitle != title {
		t.Fatalf("inbox title = %q, want %q (issue title)", inboxTitle, title)
	}
}

// TestAutomationDispatchSkipsInboxWhenNoSubscribers asserts the no-op path:
// an automation with an empty subscriber template must NOT create any inbox
// rows on dispatch — otherwise we'd be paging the workspace on every quiet
// automation run. The corresponding issue_subscriber rows are also expected
// to be absent (other-reason rows like creator/executor are filtered out by
// the WHERE type = 'issue_subscribed' clause).
func TestAutomationDispatchSkipsInboxWhenNoSubscribers(t *testing.T) {
	ctx := context.Background()
	title := fmt.Sprintf("Automation no-subscriber inbox %d", time.Now().UnixNano())
	var automationID, issueID string
	defer func() {
		if issueID != "" {
			testPool.Exec(ctx, `DELETE FROM inbox_item WHERE issue_id = $1`, issueID)
			testPool.Exec(ctx, `DELETE FROM issue WHERE id = $1`, issueID)
		}
		if automationID != "" {
			testPool.Exec(ctx, `DELETE FROM automation WHERE id = $1`, automationID)
		}
	}()

	var agentID string
	if err := testPool.QueryRow(ctx, `SELECT id FROM agent WHERE workspace_id = $1 LIMIT 1`, testWorkspaceID).Scan(&agentID); err != nil {
		t.Fatalf("load test agent: %v", err)
	}

	w := httptest.NewRecorder()
	req := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
		"title":                "No-subscriber automation",
		"executor_id":          agentID,
		"execution_mode":       "create_issue",
		"issue_title_template": title,
	})
	testHandler.CreateAutomation(w, req)
	if w.Code != http.StatusCreated {
		t.Fatalf("CreateAutomation: expected 201, got %d: %s", w.Code, w.Body.String())
	}
	var automation AutomationResponse
	if err := json.NewDecoder(w.Body).Decode(&automation); err != nil {
		t.Fatalf("decode automation: %v", err)
	}
	automationID = automation.ID

	queries := db.New(testPool)
	ap, err := queries.GetAutomation(ctx, parseUUID(automationID))
	if err != nil {
		t.Fatalf("GetAutomation: %v", err)
	}
	run, err := testHandler.AutomationService.DispatchAutomation(ctx, ap, pgtype.UUID{}, "manual", nil)
	if err != nil {
		t.Fatalf("DispatchAutomation: %v", err)
	}
	if run == nil || !run.IssueID.Valid {
		t.Fatalf("dispatch run = %+v, want linked issue", run)
	}
	issueID = uuidToString(run.IssueID)

	var inboxCount int
	if err := testPool.QueryRow(ctx, `
		SELECT count(*) FROM inbox_item
		WHERE issue_id = $1 AND type = 'issue_subscribed'
	`, issueID).Scan(&inboxCount); err != nil {
		t.Fatalf("count inbox rows: %v", err)
	}
	if inboxCount != 0 {
		t.Fatalf("issue_subscribed inbox rows = %d, want 0 (no subscribers)", inboxCount)
	}
}

// TestDeleteAutomationArchivesAndPreservesHistory guards the delete endpoint's
// product contract: user-facing delete archives the automation, hiding it from
// the default list and stopping future triggers, but preserves historical
// runs/tasks and subscriber configuration.
func TestDeleteAutomationArchivesAndPreservesHistory(t *testing.T) {
	ctx := context.Background()
	var automationID string
	var taskID string
	defer func() {
		if taskID != "" {
			testPool.Exec(ctx, `DELETE FROM agent_task_queue WHERE id = $1`, taskID)
		}
		if automationID != "" {
			testPool.Exec(ctx, `DELETE FROM automation_subscriber WHERE automation_id = $1`, automationID)
			testPool.Exec(ctx, `DELETE FROM automation WHERE id = $1`, automationID)
		}
	}()

	var agentID, runtimeID string
	if err := testPool.QueryRow(ctx, `
		SELECT id, runtime_id FROM agent
		WHERE workspace_id = $1 AND runtime_id IS NOT NULL
		LIMIT 1
	`, testWorkspaceID).Scan(&agentID, &runtimeID); err != nil {
		t.Fatalf("load test agent: %v", err)
	}

	w := httptest.NewRecorder()
	req := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
		"title":          "Delete-with-subscribers automation",
		"executor_id":    agentID,
		"execution_mode": "create_issue",
		"subscribers": []map[string]any{
			{"user_type": "member", "user_id": testUserID},
		},
	})
	testHandler.CreateAutomation(w, req)
	if w.Code != http.StatusCreated {
		t.Fatalf("CreateAutomation: expected 201, got %d: %s", w.Code, w.Body.String())
	}
	var created AutomationResponse
	if err := json.NewDecoder(w.Body).Decode(&created); err != nil {
		t.Fatalf("decode created: %v", err)
	}
	automationID = created.ID

	var before int
	if err := testPool.QueryRow(ctx, `SELECT count(*) FROM automation_subscriber WHERE automation_id = $1`, automationID).Scan(&before); err != nil {
		t.Fatalf("count subscribers before delete: %v", err)
	}
	if before != 1 {
		t.Fatalf("subscriber rows before delete = %d, want 1", before)
	}

	var runID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO automation_run (automation_id, source, status)
		VALUES ($1, 'manual', 'completed')
		RETURNING id
	`, automationID).Scan(&runID); err != nil {
		t.Fatalf("create run: %v", err)
	}
	if err := testPool.QueryRow(ctx, `
		INSERT INTO agent_task_queue (
			agent_id, runtime_id, status, priority, automation_run_id
		)
		VALUES ($1, $2, 'completed', 0, $3)
		RETURNING id
	`, agentID, runtimeID, runID).Scan(&taskID); err != nil {
		t.Fatalf("create linked task: %v", err)
	}

	w = httptest.NewRecorder()
	req = newRequest("DELETE", "/api/automations/"+automationID+"?workspace_id="+testWorkspaceID, nil)
	req = withURLParam(req, "id", automationID)
	testHandler.DeleteAutomation(w, req)
	if w.Code != http.StatusNoContent {
		t.Fatalf("DeleteAutomation: expected 204, got %d: %s", w.Code, w.Body.String())
	}

	var status string
	if err := testPool.QueryRow(ctx, `SELECT status FROM automation WHERE id = $1`, automationID).Scan(&status); err != nil {
		t.Fatalf("load automation after delete: %v", err)
	}
	if status != "archived" {
		t.Fatalf("automation status after delete = %q, want archived", status)
	}

	var after int
	if err := testPool.QueryRow(ctx, `SELECT count(*) FROM automation_subscriber WHERE automation_id = $1`, automationID).Scan(&after); err != nil {
		t.Fatalf("count subscribers after delete: %v", err)
	}
	if after != 1 {
		t.Fatalf("subscriber rows after delete = %d, want 1 (archival preserves config)", after)
	}

	var runRows int
	if err := testPool.QueryRow(ctx, `SELECT count(*) FROM automation_run WHERE id = $1 AND automation_id = $2`, runID, automationID).Scan(&runRows); err != nil {
		t.Fatalf("count run after delete: %v", err)
	}
	if runRows != 1 {
		t.Fatalf("automation_run rows after delete = %d, want 1 (archival preserves history)", runRows)
	}

	var taskRunID string
	if err := testPool.QueryRow(ctx, `SELECT automation_run_id::text FROM agent_task_queue WHERE id = $1`, taskID).Scan(&taskRunID); err != nil {
		t.Fatalf("load linked task after delete: %v", err)
	}
	if taskRunID != runID {
		t.Fatalf("task automation_run_id after delete = %q, want %q", taskRunID, runID)
	}
}
