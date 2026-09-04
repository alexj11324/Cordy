package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"slices"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/patchbay-ai/patchbay/server/internal/auth"
	"github.com/patchbay-ai/patchbay/server/internal/middleware"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
)

// The provider authorization slice answers one question at runtime: may this
// task, on this machine, spend this human's provider credential right now? The
// tests below pin the answer at the boundaries that matter — who may grant,
// what a lease stops authorizing once it is revoked or fenced, and what a
// replayed claim can obtain.

const providerAuthTestDaemonID = "provider-auth-daemon"

type providerLeaseFixture struct {
	runtimeID  string
	agentID    string
	taskID     string
	leaseToken string
	leaseID    string
	bootstrapGrantID string
}

// newProviderLeaseFixture claims a real task through the daemon claim endpoint
// rather than inserting a task_token by hand. The lease under test is therefore
// the one production mints, including the scope it computes — a claim that
// stopped issuing the provider capability would fail these tests instead of
// silently authorizing nothing.
func newProviderLeaseFixture(t *testing.T, originatorUserID string) providerLeaseFixture {
	return newProviderLeaseFixtureWithBootstrapGrant(t, originatorUserID, false)
}

func newProviderLeaseFixtureWithBootstrapGrant(t *testing.T, originatorUserID string, bootstrapGrant bool) providerLeaseFixture {
	t.Helper()

	runtimeID := dbfx.Runtime(t, "Provider authorization runtime", testutil.Cols{
		"provider":  "claude",
		"daemon_id": providerAuthTestDaemonID,
	})
	agentID := dbfx.Agent(t, "Provider authorization agent", runtimeID, testutil.Cols{
		"model": "claude-test-model",
	})
	issueID := dbfx.Issue(t, "Provider authorization issue")
	taskID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":          runtimeID,
		"issue_id":            issueID,
		"status":              "queued",
		"originator_source":   "direct_human",
		"originator_user_id":  originatorUserID,
		"accountable_user_id": originatorUserID,
	})
	var bootstrapGrantID string
	if bootstrapGrant {
		bootstrapGrantID = uuid.NewString()
		dbfx.Exec(t, `INSERT INTO authorization_grant
			(id, workspace_id, principal_type, principal_id, action, resource_type, resource_id, effect, conditions, expires_at, created_by)
			VALUES ($1, $2, 'user', $3, 'credential.use', 'provider_identity', $4, 'allow', $5::jsonb, now() + interval '1 hour', $6)`,
			bootstrapGrantID, testWorkspaceID, originatorUserID, runtimeID,
			`{"provider":"claude","provider_action":"provider.invoke","device_id":"`+runtimeID+`","models":["claude-test-model"],"max_tokens":100000}`,
			testUserID)
		dbfx.Cleanup(t, `DELETE FROM authorization_grant WHERE id = $1`, bootstrapGrantID)
	}

	claim := httptest.NewRecorder()
	request := newDaemonTokenRequest(http.MethodPost, "/api/daemon/runtimes/"+runtimeID+"/tasks/claim", nil,
		testWorkspaceID, providerAuthTestDaemonID)
	testHandler.ClaimTaskByRuntime(claim, withURLParam(request, "runtimeId", runtimeID))
	if claim.Code != http.StatusOK {
		t.Fatalf("claim task: status=%d body=%s", claim.Code, claim.Body.String())
	}
	var claimEnvelope struct {
		Task struct {
			ID        string `json:"id"`
			AuthToken string `json:"auth_token"`
		} `json:"task"`
	}
	if err := json.NewDecoder(claim.Body).Decode(&claimEnvelope); err != nil {
		t.Fatalf("decode claim: %v", err)
	}
	claimed := claimEnvelope.Task
	if claimed.ID != taskID || claimed.AuthToken == "" {
		t.Fatalf("claim returned task=%q token_present=%t, want the seeded task with a lease", claimed.ID, claimed.AuthToken != "")
	}

	var leaseID string
	dbfx.QueryRow(t, `SELECT id FROM task_token WHERE task_id = $1`, taskID).Scan(&leaseID)
	return providerLeaseFixture{
		runtimeID:  runtimeID,
		agentID:    agentID,
		taskID:     taskID,
		leaseToken: claimed.AuthToken,
		leaseID:    leaseID,
		bootstrapGrantID: bootstrapGrantID,
	}
}

type providerDecisionBody struct {
	Allowed         bool     `json:"allowed"`
	Decision        string   `json:"decision"`
	Reason          string   `json:"reason"`
	DecisionID      string   `json:"decision_id"`
	MatchedGrantIDs []string `json:"matched_grant_ids"`
	Error           string   `json:"error"`
}

func authorizeProviderOperation(t *testing.T, fixture providerLeaseFixture, body map[string]any) (int, providerDecisionBody) {
	t.Helper()

	w := httptest.NewRecorder()
	request := newDaemonTokenRequest(http.MethodPost,
		"/api/daemon/runtimes/"+fixture.runtimeID+"/tasks/"+fixture.taskID+"/provider-authorization",
		body, testWorkspaceID, providerAuthTestDaemonID)
	request = withURLParams(request, "runtimeId", fixture.runtimeID, "taskId", fixture.taskID)
	testHandler.AuthorizeProviderOperation(w, request)

	var decoded providerDecisionBody
	_ = json.NewDecoder(w.Body).Decode(&decoded)
	return w.Code, decoded
}

func providerLeaseRequest(fixture providerLeaseFixture, model string, maxTokens int64) map[string]any {
	return map[string]any{
		"lease_token": fixture.leaseToken,
		"provider":    "claude",
		"model":       model,
		"max_tokens":  maxTokens,
	}
}

// TestProviderOperationRequiresLiveLease is the whole point of the gate: the
// runtime owner running their own task is allowed, and every way a lease can
// stop being live takes that authorization away without the grant ledger
// changing at all.
func TestProviderOperationRequiresLiveLease(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	t.Run("owner use is allowed and explained", func(t *testing.T) {
		fixture := newProviderLeaseFixture(t, testUserID)
		status, decision := authorizeProviderOperation(t, fixture, providerLeaseRequest(fixture, "claude-test-model", 500))
		if status != http.StatusOK || !decision.Allowed || decision.Decision != "allow" {
			t.Fatalf("owner provider use: status=%d body=%#v", status, decision)
		}
		if decision.DecisionID == "" {
			t.Fatalf("allow decision was not written to the explain ledger: %#v", decision)
		}

		explain := httptest.NewRecorder()
		request := withURLParam(newRequest("GET", "/api/provider-authorizations/decisions/"+decision.DecisionID, nil),
			"decisionId", decision.DecisionID)
		testHandler.ExplainProviderAuthorizationDecision(explain, withChatTestWorkspaceCtx(t, request))
		if explain.Code != http.StatusOK {
			t.Fatalf("explain decision: status=%d body=%s", explain.Code, explain.Body.String())
		}
		var explained struct {
			Decision     string `json:"decision"`
			ResourceType string `json:"resource_type"`
			Action       string `json:"action"`
			DeviceID     string `json:"device_id"`
		}
		if err := json.NewDecoder(explain.Body).Decode(&explained); err != nil {
			t.Fatalf("decode explain: %v", err)
		}
		if explained.Decision != "allow" || explained.Action != "credential.use" ||
			explained.ResourceType != "provider_identity" || explained.DeviceID != fixture.runtimeID {
			t.Fatalf("explained decision = %#v, want the credential.use decision for this runtime", explained)
		}
	})

	t.Run("revoked lease cannot act", func(t *testing.T) {
		fixture := newProviderLeaseFixture(t, testUserID)
		revoke := httptest.NewRecorder()
		request := withURLParam(newRequest("DELETE", "/api/provider-authorizations/leases/"+fixture.leaseID+"?reason=stop", nil),
			"leaseId", fixture.leaseID)
		testHandler.RevokeProviderCapabilityLease(revoke, withChatTestWorkspaceCtx(t, request))
		if revoke.Code != http.StatusNoContent {
			t.Fatalf("revoke lease: status=%d body=%s", revoke.Code, revoke.Body.String())
		}

		status, decision := authorizeProviderOperation(t, fixture, providerLeaseRequest(fixture, "claude-test-model", 500))
		if status != http.StatusForbidden || decision.Allowed {
			t.Fatalf("revoked lease authorized an operation: status=%d body=%#v", status, decision)
		}
	})

	t.Run("re-dispatched claim fences the old lease", func(t *testing.T) {
		fixture := newProviderLeaseFixture(t, testUserID)
		// A re-dispatch is what a reclaim after a daemon restart looks like.
		// The lease the previous daemon still holds is bound to the claim it
		// was minted for, so it must stop authorizing work the moment that
		// claim is superseded — even though nobody revoked it by hand.
		dbfx.Exec(t, `UPDATE agent_task_queue SET dispatched_at = now() + interval '1 second' WHERE id = $1`, fixture.taskID)

		status, decision := authorizeProviderOperation(t, fixture, providerLeaseRequest(fixture, "claude-test-model", 500))
		if status != http.StatusForbidden || decision.Allowed {
			t.Fatalf("superseded lease authorized an operation: status=%d body=%#v", status, decision)
		}
	})

	t.Run("finished task cannot spend its lease", func(t *testing.T) {
		fixture := newProviderLeaseFixture(t, testUserID)
		dbfx.Exec(t, `UPDATE agent_task_queue SET status = 'completed', completed_at = now() WHERE id = $1`, fixture.taskID)

		status, decision := authorizeProviderOperation(t, fixture, providerLeaseRequest(fixture, "claude-test-model", 500))
		if status != http.StatusForbidden || decision.Allowed {
			t.Fatalf("completed task authorized provider use: status=%d body=%#v", status, decision)
		}
	})

	t.Run("a lease nobody holds is denied and recorded", func(t *testing.T) {
		fixture := newProviderLeaseFixture(t, testUserID)
		body := providerLeaseRequest(fixture, "claude-test-model", 500)
		body["lease_token"] = "mat_" + uuid.NewString()

		status, decision := authorizeProviderOperation(t, fixture, body)
		if status != http.StatusForbidden || decision.Allowed {
			t.Fatalf("unknown lease authorized an operation: status=%d body=%#v", status, decision)
		}
		if decision.DecisionID == "" {
			t.Fatalf("a presented-but-unknown lease left no explain record: %#v", decision)
		}
	})
}

// TestProviderCapabilityLeaseIsClaimBound covers the two properties the unique
// claim index exists for: one dispatch mints exactly one lease, and a replayed
// mint cannot quietly hand out a second bearer for the same claim.
func TestProviderCapabilityLeaseIsClaimBound(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	fixture := newProviderLeaseFixture(t, testUserID)

	task, err := testHandler.Queries.GetAgentTask(ctx, parseUUID(fixture.taskID))
	if err != nil {
		t.Fatalf("load claimed task: %v", err)
	}
	replayed, err := testHandler.Queries.CreateTaskToken(ctx, db.CreateTaskTokenParams{
		ID:                dbid.NewV7(),
		TokenHash:         auth.HashToken("mat_" + uuid.NewString()),
		TaskID:            task.ID,
		AgentID:           task.AgentID,
		WorkspaceID:       parseUUID(testWorkspaceID),
		UserID:            parseUUID(testUserID),
		ExpiresAt:         task.DispatchedAt,
		Scope:             []byte(`[{"action":"credential.use","resource_type":"provider_identity","resource_id":"*"}]`),
		ClaimDispatchedAt: task.DispatchedAt,
		OnBehalfOfUserID:  task.OriginatorUserID,
		DeviceID:          task.RuntimeID,
		DelegationFence:   0,
	})
	if err == nil {
		t.Fatalf("replayed claim minted a second lease %s for one dispatch", uuidToString(replayed.ID))
	}

	if leases := dbfx.Count(t, `SELECT count(*) FROM task_token WHERE task_id = $1`, fixture.taskID); leases != 1 {
		t.Fatalf("task holds %d leases, want exactly one per dispatch", leases)
	}
	var scope []byte
	dbfx.QueryRow(t, `SELECT scope FROM task_token WHERE task_id = $1`, fixture.taskID).Scan(&scope)
	var capabilities []struct {
		Action       string `json:"action"`
		ResourceType string `json:"resource_type"`
		ResourceID   string `json:"resource_id"`
	}
	if err := json.Unmarshal(scope, &capabilities); err != nil {
		t.Fatalf("decode lease scope: %v", err)
	}
	credential := false
	for _, capability := range capabilities {
		if capability.Action == "credential.use" && capability.ResourceType == "provider_identity" &&
			capability.ResourceID == fixture.runtimeID {
			credential = true
		}
	}
	if !credential {
		t.Fatalf("minted lease scope %s does not carry credential.use for its own runtime", scope)
	}
}

// TestProviderGrantPermissions pins who may hand out someone else's provider
// credential, and what a non-owner's task can do with and without a grant.
func TestProviderGrantPermissions(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	otherUserID := dbfx.User(t, "Provider grant other", "provider-grant-"+uuid.NewString()+"@example.com")
	dbfx.Member(t, testWorkspaceID, otherUserID, "member")
	fixture := newProviderLeaseFixtureWithBootstrapGrant(t, otherUserID, true)
	// Claim issuance now performs the Rust-aligned provider pre-gate. Revoke
	// only the bootstrap grant after issuance so this test can still exercise
	// the independent operation-time "no grant" decision.
	dbfx.Exec(t, `UPDATE authorization_grant SET revoked_at = now(), revoked_by = $2 WHERE id = $1`, fixture.bootstrapGrantID, testUserID)

	createGrant := func(t *testing.T, asUserID string, body map[string]any) (int, map[string]any) {
		t.Helper()
		w := httptest.NewRecorder()
		request := newRequest("POST", "/api/provider-authorizations", body)
		request.Header.Set("X-User-ID", asUserID)
		testHandler.CreateProviderAuthorizationGrant(w, withProviderTestWorkspaceCtx(t, request, asUserID))
		decoded := map[string]any{}
		_ = json.NewDecoder(w.Body).Decode(&decoded)
		return w.Code, decoded
	}

	grantBody := map[string]any{
		"grantee_type": "user",
		"grantee_id":   otherUserID,
		"runtime_id":   fixture.runtimeID,
		"actions":      []string{"provider.invoke"},
		"models":       []string{"claude-test-model"},
		"max_tokens":   1000,
		"expires_at":   time.Now().Add(time.Hour).UTC().Format(time.RFC3339),
	}

	// Without a grant the runtime owner has authorized nothing, so another
	// member's task is refused even though its lease is perfectly live.
	status, decision := authorizeProviderOperation(t, fixture, providerLeaseRequest(fixture, "claude-test-model", 500))
	if status != http.StatusForbidden || decision.Allowed {
		t.Fatalf("ungranted member use was authorized: status=%d body=%#v", status, decision)
	}

	// Nor can the would-be grantee authorize themselves.
	if code, body := createGrant(t, otherUserID, grantBody); code != http.StatusForbidden {
		t.Fatalf("non-owner minted a grant over someone else's provider identity: status=%d body=%#v", code, body)
	}

	code, created := createGrant(t, testUserID, grantBody)
	if code != http.StatusCreated {
		t.Fatalf("runtime owner could not grant provider use: status=%d body=%#v", code, created)
	}
	grantID, _ := created["id"].(string)
	if grantID == "" {
		t.Fatalf("created grant carries no id: %#v", created)
	}

	// The ledger says who may spend whose credential, so a member sees only
	// the grants they made or are named by.
	listGrantIDs := func(t *testing.T, asUserID string) []string {
		t.Helper()
		w := httptest.NewRecorder()
		request := newRequest("GET", "/api/provider-authorizations", nil)
		request.Header.Set("X-User-ID", asUserID)
		testHandler.ListProviderAuthorizationGrants(w, withProviderTestWorkspaceCtx(t, request, asUserID))
		if w.Code != http.StatusOK {
			t.Fatalf("list grants as %s: status=%d body=%s", asUserID, w.Code, w.Body.String())
		}
		var listed struct {
			Grants []struct {
				ID string `json:"id"`
			} `json:"grants"`
		}
		if err := json.NewDecoder(w.Body).Decode(&listed); err != nil {
			t.Fatalf("decode grant list: %v", err)
		}
		ids := make([]string, 0, len(listed.Grants))
		for _, grant := range listed.Grants {
			ids = append(ids, grant.ID)
		}
		return ids
	}
	if !slices.Contains(listGrantIDs(t, otherUserID), grantID) {
		t.Fatalf("grantee cannot see the grant that names them (%s)", grantID)
	}
	bystanderID := dbfx.User(t, "Provider grant bystander", "provider-grant-bystander-"+uuid.NewString()+"@example.com")
	dbfx.Member(t, testWorkspaceID, bystanderID, "member")
	if slices.Contains(listGrantIDs(t, bystanderID), grantID) {
		t.Fatalf("an unrelated member can enumerate a grant they are not party to (%s)", grantID)
	}

	status, decision = authorizeProviderOperation(t, fixture, providerLeaseRequest(fixture, "claude-test-model", 500))
	if status != http.StatusOK || !decision.Allowed {
		t.Fatalf("granted member use was refused: status=%d body=%#v", status, decision)
	}
	if len(decision.MatchedGrantIDs) != 1 || decision.MatchedGrantIDs[0] != grantID {
		t.Fatalf("decision matched %v, want exactly the grant that authorized it (%s)", decision.MatchedGrantIDs, grantID)
	}

	// The grant names one model. Another model is not a narrower use of it.
	status, decision = authorizeProviderOperation(t, fixture, providerLeaseRequest(fixture, "claude-other-model", 500))
	if status != http.StatusForbidden || decision.Allowed {
		t.Fatalf("model outside the grant was authorized: status=%d body=%#v", status, decision)
	}

	// The budget is a ceiling across decisions, not per request: the first 500
	// of a 1000-token grant is fine, and the request that would take it past
	// 1000 is refused.
	if status, decision = authorizeProviderOperation(t, fixture, providerLeaseRequest(fixture, "claude-test-model", 600)); status != http.StatusForbidden {
		t.Fatalf("request over the remaining budget was authorized: status=%d body=%#v", status, decision)
	}

	// Revoking the grant stops the next decision, and only its creator can
	// revoke it.
	revokeAs := func(t *testing.T, asUserID string) int {
		t.Helper()
		w := httptest.NewRecorder()
		request := withURLParam(newRequest("DELETE", "/api/provider-authorizations/"+grantID, nil), "grantId", grantID)
		request.Header.Set("X-User-ID", asUserID)
		testHandler.RevokeProviderAuthorizationGrant(w, withProviderTestWorkspaceCtx(t, request, asUserID))
		return w.Code
	}
	if code := revokeAs(t, otherUserID); code != http.StatusNotFound {
		t.Fatalf("grantee revoked a grant they did not create: status=%d", code)
	}
	if code := revokeAs(t, testUserID); code != http.StatusNoContent {
		t.Fatalf("grant creator could not revoke: status=%d", code)
	}
	status, decision = authorizeProviderOperation(t, fixture, providerLeaseRequest(fixture, "claude-test-model", 100))
	if status != http.StatusForbidden || decision.Allowed {
		t.Fatalf("revoked grant still authorized provider use: status=%d body=%#v", status, decision)
	}
}

// withProviderTestWorkspaceCtx injects the workspace + member context the chi
// middleware chain sets, for whichever member the test is acting as.
func withProviderTestWorkspaceCtx(t *testing.T, request *http.Request, userID string) *http.Request {
	t.Helper()
	member, err := testHandler.Queries.GetMemberByUserAndWorkspace(context.Background(), db.GetMemberByUserAndWorkspaceParams{
		UserID:      parseUUID(userID),
		WorkspaceID: parseUUID(testWorkspaceID),
	})
	if err != nil {
		t.Fatalf("load member row for %s: %v", userID, err)
	}
	return request.WithContext(middleware.SetMemberContext(request.Context(), testWorkspaceID, member))
}
