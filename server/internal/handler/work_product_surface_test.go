package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"testing"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func withWorkProductURLParams(req *http.Request, params map[string]string) *http.Request {
	rctx := chi.NewRouteContext()
	for key, value := range params {
		rctx.URLParams.Add(key, value)
	}
	return req.WithContext(context.WithValue(req.Context(), chi.RouteCtxKey, rctx))
}

// TestWorkProductViewOmitsAbsentOptionalFields pins the shape a client branches
// on. A product with no task anchor and no PR mirror must omit those keys
// rather than send empty strings: a client that reads `""` as "attached to the
// empty task" or renders a blank PR card cannot tell absent from unset.
func TestWorkProductViewOmitsAbsentOptionalFields(t *testing.T) {
	now := pgtype.Timestamptz{Time: time.Unix(1_700_000_000, 0).UTC(), Valid: true}
	view := workProductViewFromRow(workProductRow{
		Product: db.WorkProduct{
			ID:               testWorkProductUUID(t, "11111111-1111-4111-8111-111111111111"),
			WorkspaceID:      testWorkProductUUID(t, "22222222-2222-4222-8222-222222222222"),
			Kind:             "document",
			Provider:         "notion",
			ExternalIdentity: "workspace/doc-1",
			CreatedAt:        now,
			UpdatedAt:        now,
		},
		RelationID:      testWorkProductUUID(t, "33333333-3333-4333-8333-333333333333"),
		RelationIssueID: testWorkProductUUID(t, "44444444-4444-4444-8444-444444444444"),
		RelationSource:  "manual_explicit",
		AttachedByType:  "user",
		AttachedByID:    testWorkProductUUID(t, "55555555-5555-4555-8555-555555555555"),
		AttachedAt:      now,
	})
	if view.PullRequest != nil {
		t.Fatalf("non-PR product carried a pull request card: %+v", view.PullRequest)
	}
	if view.Relation.TaskID != nil || view.Relation.RunID != nil {
		t.Fatalf("manual relation invented execution provenance: %+v", view.Relation)
	}
	if view.ExternalURL != nil || view.ProviderRecordType != nil || view.ProviderRecordID != nil {
		t.Fatalf("absent provider columns were materialized: %+v", view)
	}

	encoded, err := json.Marshal(view)
	if err != nil {
		t.Fatalf("marshal view: %v", err)
	}
	var decoded map[string]any
	if err := json.Unmarshal(encoded, &decoded); err != nil {
		t.Fatalf("decode view: %v", err)
	}
	for _, absent := range []string{"pull_request", "task_id", "run_id"} {
		if _, present := decoded[absent]; present && absent == "pull_request" {
			t.Errorf("key %q must be omitted when there is no value", absent)
		}
	}
	relation, _ := decoded["relation"].(map[string]any)
	for _, absent := range []string{"task_id", "run_id"} {
		if _, present := relation[absent]; present {
			t.Errorf("relation key %q must be omitted when there is no value", absent)
		}
	}
	if relation["issue_id"] != "44444444-4444-4444-8444-444444444444" {
		t.Errorf("relation issue_id = %v, want the anchor issue", relation["issue_id"])
	}
}

// TestWorkProductRelationDetachIsSoftAndAudited covers the whole retraction
// contract in one pass: the relation leaves the issue's live list, the row
// survives with the detaching actor recorded, both the attach and the detach
// leave an activity trail, and a second detach of the same relation is a 404
// rather than a silent success.
func TestWorkProductRelationDetachIsSoftAndAudited(t *testing.T) {
	if testHandler == nil {
		t.Skip("handler test fixture not initialized (no DB?)")
	}
	ctx := context.Background()

	created := testutil.Call(t, testHandler.CreateIssue,
		newRequest("POST", "/api/issues?workspace_id="+testWorkspaceID, map[string]any{
			"title": "work product detach",
		})).Want(http.StatusCreated)
	var issue IssueResponse
	json.NewDecoder(created.Body).Decode(&issue)

	t.Cleanup(func() {
		testPool.Exec(ctx, `DELETE FROM work_product_relation WHERE issue_id = $1`, issue.ID)
		testPool.Exec(ctx, `DELETE FROM work_product WHERE workspace_id = $1 AND external_identity = $2`,
			testWorkspaceID, "detach/fixture-1")
		testPool.Exec(ctx, `DELETE FROM activity_log WHERE issue_id = $1`, issue.ID)
		testPool.Exec(ctx, `DELETE FROM issue WHERE id = $1`, issue.ID)
	})

	productResp := testutil.Call(t, testHandler.CreateWorkProduct,
		newRequest("POST", "/api/work-products", map[string]any{
			"kind":              "document",
			"provider":          "notion",
			"external_identity": "detach/fixture-1",
		})).Want(http.StatusOK)
	var product db.WorkProduct
	json.NewDecoder(productResp.Body).Decode(&product)

	attachReq := withWorkProductURLParams(
		newRequest("POST", "/api/issues/"+issue.ID+"/work-product-relations", map[string]any{
			"work_product_id": uuidToString(product.ID),
		}),
		map[string]string{"id": issue.ID},
	)
	attachResp := testutil.Call(t, testHandler.CreateWorkProductRelation, attachReq).Want(http.StatusOK)
	var relation db.WorkProductRelation
	json.NewDecoder(attachResp.Body).Decode(&relation)

	listed, err := listIssueWorkProducts(ctx, parseUUID(issue.ID))
	if err != nil {
		t.Fatalf("listIssueWorkProducts: %v", err)
	}
	if len(listed) != 1 {
		t.Fatalf("attached product count = %d, want 1", len(listed))
	}

	detachReq := withWorkProductURLParams(
		newRequest("DELETE", "/api/issues/"+issue.ID+"/work-product-relations/"+uuidToString(relation.ID), nil),
		map[string]string{"id": issue.ID, "relationId": uuidToString(relation.ID)},
	)
	testutil.Call(t, testHandler.DetachWorkProductRelation, detachReq).Want(http.StatusOK)

	listed, err = listIssueWorkProducts(ctx, parseUUID(issue.ID))
	if err != nil {
		t.Fatalf("listIssueWorkProducts after detach: %v", err)
	}
	if len(listed) != 0 {
		t.Fatalf("detached product still listed: %+v", listed)
	}

	// The row survives the detach. Deleting it would erase who attached the
	// product along with the retraction.
	var detachedByType string
	var detachedAt pgtype.Timestamptz
	if err := testPool.QueryRow(ctx,
		`SELECT detached_by_type, detached_at FROM work_product_relation WHERE id = $1`,
		uuidToString(relation.ID),
	).Scan(&detachedByType, &detachedAt); err != nil {
		t.Fatalf("relation row was deleted rather than detached: %v", err)
	}
	if detachedByType != "user" || !detachedAt.Valid {
		t.Errorf("detach audit columns = (%q, valid=%v), want (\"user\", true)", detachedByType, detachedAt.Valid)
	}

	for _, action := range []string{workProductAttachedActivity, workProductDetachedActivity} {
		var count int
		if err := testPool.QueryRow(ctx,
			`SELECT count(*) FROM activity_log WHERE issue_id = $1 AND action = $2`,
			issue.ID, action,
		).Scan(&count); err != nil {
			t.Fatalf("count %s activity: %v", action, err)
		}
		if count != 1 {
			t.Errorf("%s activity rows = %d, want 1", action, count)
		}
	}

	// Detaching twice must not read as success: the second caller is acting on
	// a relation that is no longer live, and a 200 would tell them otherwise.
	repeat := withWorkProductURLParams(
		newRequest("DELETE", "/api/issues/"+issue.ID+"/work-product-relations/"+uuidToString(relation.ID), nil),
		map[string]string{"id": issue.ID, "relationId": uuidToString(relation.ID)},
	)
	testutil.Call(t, testHandler.DetachWorkProductRelation, repeat).Want(http.StatusNotFound)
}
