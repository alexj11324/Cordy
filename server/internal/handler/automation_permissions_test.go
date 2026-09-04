package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

// createPlainMember adds a fresh member-role user to the test workspace and
// returns its user id. Used to exercise the automation write gate from the
// perspective of a member who is neither the creator nor a workspace admin.
func createPlainMember(t *testing.T, email string) string {
	t.Helper()
	ctx := context.Background()

	var userID string
	if err := testPool.QueryRow(ctx,
		`INSERT INTO "user" (name, email) VALUES ('AP Perm Member', $1) RETURNING id`,
		email,
	).Scan(&userID); err != nil {
		t.Fatalf("create user: %v", err)
	}
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM member WHERE workspace_id = $1 AND user_id = $2`, testWorkspaceID, userID)
		testPool.Exec(context.Background(), `DELETE FROM "user" WHERE id = $1`, userID)
	})

	if _, err := testPool.Exec(ctx,
		`INSERT INTO member (workspace_id, user_id, role) VALUES ($1, $2, 'member')`,
		testWorkspaceID, userID,
	); err != nil {
		t.Fatalf("add member: %v", err)
	}
	return userID
}

// createAutomationAs creates an automation via the API as the given user (empty
// userID = workspace owner) assigned to a fresh public agent, and returns its
// id. The caller-supplied title prefix keeps cleanup queries unambiguous.
func createAutomationAs(t *testing.T, userID, title string) string {
	t.Helper()
	agentID := createHandlerTestAgent(t, title+"-agent", nil)

	body := map[string]any{
		"title":          title,
		"executor_id":    agentID,
		"execution_mode": "create_issue",
	}
	w := httptest.NewRecorder()
	path := "/api/automations?workspace_id=" + testWorkspaceID
	var r *http.Request
	if userID == "" {
		r = newRequest("POST", path, body)
	} else {
		r = newRequestAs(userID, "POST", path, body)
	}
	testHandler.CreateAutomation(w, r)
	if w.Code != http.StatusCreated {
		t.Fatalf("CreateAutomation: expected 201, got %d: %s", w.Code, w.Body.String())
	}
	var ap AutomationResponse
	if err := json.NewDecoder(w.Body).Decode(&ap); err != nil {
		t.Fatalf("decode automation: %v", err)
	}
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM automation_run WHERE automation_id = $1`, ap.ID)
		testPool.Exec(context.Background(), `DELETE FROM automation_trigger WHERE automation_id = $1`, ap.ID)
		testPool.Exec(context.Background(), `DELETE FROM automation_collaborator WHERE automation_id = $1`, ap.ID)
		testPool.Exec(context.Background(), `DELETE FROM automation WHERE id = $1`, ap.ID)
	})
	return ap.ID
}

// grantAutomationAccess grants the target member write access via the API as the
// given caller (empty caller = workspace owner), asserting the expected status.
func grantAutomationAccess(t *testing.T, caller, apID, targetUserID string, wantStatus int) {
	t.Helper()
	w := httptest.NewRecorder()
	path := "/api/automations/" + apID + "/collaborators?workspace_id=" + testWorkspaceID
	body := map[string]any{"user_id": targetUserID}
	var r *http.Request
	if caller == "" {
		r = newRequest("POST", path, body)
	} else {
		r = newRequestAs(caller, "POST", path, body)
	}
	r = withURLParam(r, "id", apID)
	testHandler.AddAutomationCollaborator(w, r)
	if w.Code != wantStatus {
		t.Fatalf("AddAutomationCollaborator: expected %d, got %d: %s", wantStatus, w.Code, w.Body.String())
	}
}

// automationCanWrite fetches the detail as the given caller and returns the
// can_write flag the server stamped for them.
func automationCanWrite(t *testing.T, caller, apID string) bool {
	t.Helper()
	w := httptest.NewRecorder()
	path := "/api/automations/" + apID + "?workspace_id=" + testWorkspaceID
	var r *http.Request
	if caller == "" {
		r = newRequest("GET", path, nil)
	} else {
		r = newRequestAs(caller, "GET", path, nil)
	}
	r = withURLParam(r, "id", apID)
	testHandler.GetAutomation(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("GetAutomation: expected 200, got %d: %s", w.Code, w.Body.String())
	}
	var got struct {
		Automation AutomationResponse `json:"automation"`
	}
	if err := json.NewDecoder(w.Body).Decode(&got); err != nil {
		t.Fatalf("decode automation: %v", err)
	}
	if got.Automation.CanWrite == nil {
		t.Fatalf("expected can_write to be set on detail response")
	}
	return *got.Automation.CanWrite
}

// automationCanManageAccess fetches the detail as the given caller and returns
// the can_manage_access flag (narrower than can_write — collaborators lack it).
func automationCanManageAccess(t *testing.T, caller, apID string) bool {
	t.Helper()
	w := httptest.NewRecorder()
	path := "/api/automations/" + apID + "?workspace_id=" + testWorkspaceID
	var r *http.Request
	if caller == "" {
		r = newRequest("GET", path, nil)
	} else {
		r = newRequestAs(caller, "GET", path, nil)
	}
	r = withURLParam(r, "id", apID)
	testHandler.GetAutomation(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("GetAutomation: expected 200, got %d: %s", w.Code, w.Body.String())
	}
	var got struct {
		Automation AutomationResponse `json:"automation"`
	}
	if err := json.NewDecoder(w.Body).Decode(&got); err != nil {
		t.Fatalf("decode automation: %v", err)
	}
	if got.Automation.CanManageAccess == nil {
		t.Fatalf("expected can_manage_access to be set on detail response")
	}
	return *got.Automation.CanManageAccess
}

// TestAutomationCollaborator_GrantedMemberCanWrite verifies the full delegation
// flow: a non-creator member is blocked, becomes a writer once granted, and is
// blocked again after the grant is revoked (MUL-3807).
func TestAutomationCollaborator_GrantedMemberCanWrite(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	apID := createAutomationAs(t, "", "ap-collab-grant")
	member := createPlainMember(t, "ap-collab-grantee@patchbay.test")

	updateAs := func(caller string) int {
		w := httptest.NewRecorder()
		r := newRequestAs(caller, "PATCH", "/api/automations/"+apID+"?workspace_id="+testWorkspaceID, map[string]any{"title": "edited by " + caller})
		r = withURLParam(r, "id", apID)
		testHandler.UpdateAutomation(w, r)
		return w.Code
	}

	// Before grant: blocked, and can_write=false.
	if code := updateAs(member); code != http.StatusForbidden {
		t.Fatalf("pre-grant update: expected 403, got %d", code)
	}
	if automationCanWrite(t, member, apID) {
		t.Fatalf("pre-grant: expected can_write=false for member")
	}

	// Grant.
	grantAutomationAccess(t, "", apID, member, http.StatusCreated)

	// After grant: allowed, and can_write=true.
	if !automationCanWrite(t, member, apID) {
		t.Fatalf("post-grant: expected can_write=true for collaborator")
	}
	if code := updateAs(member); code != http.StatusOK {
		t.Fatalf("post-grant update: expected 200, got %d", code)
	}

	// Revoke.
	w := httptest.NewRecorder()
	r := newRequest("DELETE", "/api/automations/"+apID+"/collaborators/"+member+"?workspace_id="+testWorkspaceID, nil)
	r = withURLParams(r, "id", apID, "userId", member)
	testHandler.RemoveAutomationCollaborator(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("RemoveAutomationCollaborator: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	// After revoke: blocked again.
	if code := updateAs(member); code != http.StatusForbidden {
		t.Fatalf("post-revoke update: expected 403, got %d", code)
	}
}

// TestAutomationCollaborator_NonWriterCannotGrant verifies a member without write
// access cannot manage the access list, and granting a non-member is rejected.
func TestAutomationCollaborator_NonWriterCannotGrant(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	apID := createAutomationAs(t, "", "ap-collab-guard")
	stranger := createPlainMember(t, "ap-collab-stranger@patchbay.test")
	victim := createPlainMember(t, "ap-collab-victim@patchbay.test")

	// A non-writer cannot grant access to anyone.
	grantAutomationAccess(t, stranger, apID, victim, http.StatusForbidden)

	// Owner granting a non-member (random UUID) is rejected as bad input.
	grantAutomationAccess(t, "", apID, "00000000-0000-0000-0000-000000000000", http.StatusBadRequest)
}

// TestAutomationCollaborator_CannotManageAccessList verifies the privilege-
// escalation boundary: a granted collaborator keeps write/execute access but
// CANNOT manage the access list — they cannot grant access to others or revoke
// peers. Only the creator / owner / admin may manage access (MUL-3807).
func TestAutomationCollaborator_CannotManageAccessList(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	apID := createAutomationAs(t, "", "ap-collab-noescalate")
	carol := createPlainMember(t, "ap-collab-carol@patchbay.test")
	dave := createPlainMember(t, "ap-collab-dave@patchbay.test")
	bob := createPlainMember(t, "ap-collab-bob2@patchbay.test")

	// Owner grants two collaborators.
	grantAutomationAccess(t, "", apID, carol, http.StatusCreated)
	grantAutomationAccess(t, "", apID, dave, http.StatusCreated)

	// Collaborator carol keeps write access (can still edit the automation).
	w := httptest.NewRecorder()
	r := newRequestAs(carol, "PATCH", "/api/automations/"+apID+"?workspace_id="+testWorkspaceID, map[string]any{"title": "carol edit"})
	r = withURLParam(r, "id", apID)
	testHandler.UpdateAutomation(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("collaborator update: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	// ...but cannot grant access to a new member.
	grantAutomationAccess(t, carol, apID, bob, http.StatusForbidden)

	// ...and cannot revoke a peer collaborator.
	w = httptest.NewRecorder()
	r = newRequestAs(carol, "DELETE", "/api/automations/"+apID+"/collaborators/"+dave+"?workspace_id="+testWorkspaceID, nil)
	r = withURLParams(r, "id", apID, "userId", dave)
	testHandler.RemoveAutomationCollaborator(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatalf("collaborator revoke peer: expected 403, got %d: %s", w.Code, w.Body.String())
	}

	// can_manage_access: false for the collaborator, true for the owner.
	if automationCanManageAccess(t, carol, apID) {
		t.Fatalf("carol can_manage_access: expected false")
	}
	if !automationCanManageAccess(t, "", apID) {
		t.Fatalf("owner can_manage_access: expected true")
	}
}

// TestAutomationWrite_PlainMemberCannotMutateOthers verifies that a workspace
// member who is neither the creator nor an admin cannot edit, trigger, or
// delete an automation created by someone else (MUL-3807).
func TestAutomationWrite_PlainMemberCannotMutateOthers(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	apID := createAutomationAs(t, "", "ap-perm-owner-created")
	member := createPlainMember(t, "ap-perm-stranger@patchbay.test")

	// Update.
	w := httptest.NewRecorder()
	r := newRequestAs(member, "PATCH", "/api/automations/"+apID+"?workspace_id="+testWorkspaceID, map[string]any{"title": "hijacked"})
	r = withURLParam(r, "id", apID)
	testHandler.UpdateAutomation(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatalf("UpdateAutomation by stranger: expected 403, got %d: %s", w.Code, w.Body.String())
	}

	// Trigger.
	w = httptest.NewRecorder()
	r = newRequestAs(member, "POST", "/api/automations/"+apID+"/trigger?workspace_id="+testWorkspaceID, nil)
	r = withURLParam(r, "id", apID)
	testHandler.TriggerAutomation(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatalf("TriggerAutomation by stranger: expected 403, got %d: %s", w.Code, w.Body.String())
	}

	// Delete.
	w = httptest.NewRecorder()
	r = newRequestAs(member, "DELETE", "/api/automations/"+apID+"?workspace_id="+testWorkspaceID, nil)
	r = withURLParam(r, "id", apID)
	testHandler.DeleteAutomation(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatalf("DeleteAutomation by stranger: expected 403, got %d: %s", w.Code, w.Body.String())
	}
}

// TestAutomationWrite_CreatorCanMutateOwn verifies that the member who created
// an automation retains write access to it even without an admin role.
func TestAutomationWrite_CreatorCanMutateOwn(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	member := createPlainMember(t, "ap-perm-creator@patchbay.test")
	apID := createAutomationAs(t, member, "ap-perm-member-created")

	w := httptest.NewRecorder()
	r := newRequestAs(member, "PATCH", "/api/automations/"+apID+"?workspace_id="+testWorkspaceID, map[string]any{"title": "creator edit"})
	r = withURLParam(r, "id", apID)
	testHandler.UpdateAutomation(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("UpdateAutomation by creator: expected 200, got %d: %s", w.Code, w.Body.String())
	}
}

// TestAutomationWrite_AdminCanMutateMembersAutomation verifies that a workspace
// owner/admin can manage an automation created by a plain member.
func TestAutomationWrite_AdminCanMutateMembersAutomation(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	member := createPlainMember(t, "ap-perm-admin-target@patchbay.test")
	apID := createAutomationAs(t, member, "ap-perm-admin-target")

	// testUserID is the workspace owner.
	w := httptest.NewRecorder()
	r := newRequest("PATCH", "/api/automations/"+apID+"?workspace_id="+testWorkspaceID, map[string]any{"title": "admin edit"})
	r = withURLParam(r, "id", apID)
	testHandler.UpdateAutomation(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("UpdateAutomation by owner: expected 200, got %d: %s", w.Code, w.Body.String())
	}
}

// TestAutomationWrite_WebhookSecretRedactedForNonWriter verifies that the
// webhook token/path are returned to a writer (the owner) but stripped from
// the read response for a member who lacks write access — seeing the token is
// equivalent to being able to trigger the automation (MUL-3807).
func TestAutomationWrite_WebhookSecretRedactedForNonWriter(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	apID := createAutomationAs(t, "", "ap-perm-secret")
	stranger := createPlainMember(t, "ap-perm-secret-stranger@patchbay.test")

	// Owner adds a webhook trigger.
	w := httptest.NewRecorder()
	r := newRequest("POST", "/api/automations/"+apID+"/triggers?workspace_id="+testWorkspaceID, map[string]any{"kind": "webhook"})
	r = withURLParam(r, "id", apID)
	testHandler.CreateAutomationTrigger(w, r)
	if w.Code != http.StatusCreated {
		t.Fatalf("CreateAutomationTrigger: expected 201, got %d: %s", w.Code, w.Body.String())
	}

	type getResp struct {
		Triggers []AutomationTriggerResponse `json:"triggers"`
	}

	// Owner (writer) sees the secret.
	w = httptest.NewRecorder()
	r = withURLParam(newRequest("GET", "/api/automations/"+apID+"?workspace_id="+testWorkspaceID, nil), "id", apID)
	testHandler.GetAutomation(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("GetAutomation as owner: expected 200, got %d: %s", w.Code, w.Body.String())
	}
	var ownerView getResp
	if err := json.NewDecoder(w.Body).Decode(&ownerView); err != nil {
		t.Fatalf("decode owner view: %v", err)
	}
	if len(ownerView.Triggers) != 1 {
		t.Fatalf("owner view: expected 1 trigger, got %d", len(ownerView.Triggers))
	}
	if ownerView.Triggers[0].WebhookToken == nil || *ownerView.Triggers[0].WebhookToken == "" {
		t.Fatalf("owner view: expected webhook_token to be present")
	}
	if ownerView.Triggers[0].WebhookPath == nil {
		t.Fatalf("owner view: expected webhook_path to be present")
	}

	// Plain member (non-writer) sees the trigger but not the secret.
	w = httptest.NewRecorder()
	r = withURLParam(newRequestAs(stranger, "GET", "/api/automations/"+apID+"?workspace_id="+testWorkspaceID, nil), "id", apID)
	testHandler.GetAutomation(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("GetAutomation as stranger: expected 200, got %d: %s", w.Code, w.Body.String())
	}
	var strangerView getResp
	if err := json.NewDecoder(w.Body).Decode(&strangerView); err != nil {
		t.Fatalf("decode stranger view: %v", err)
	}
	if len(strangerView.Triggers) != 1 {
		t.Fatalf("stranger view: expected 1 trigger, got %d", len(strangerView.Triggers))
	}
	if strangerView.Triggers[0].Kind != "webhook" {
		t.Fatalf("stranger view: expected webhook trigger to remain visible, got kind %q", strangerView.Triggers[0].Kind)
	}
	if strangerView.Triggers[0].WebhookToken != nil {
		t.Fatalf("stranger view: webhook_token leaked: %v", *strangerView.Triggers[0].WebhookToken)
	}
	if strangerView.Triggers[0].WebhookPath != nil {
		t.Fatalf("stranger view: webhook_path leaked: %v", *strangerView.Triggers[0].WebhookPath)
	}
	if strangerView.Triggers[0].WebhookURL != nil {
		t.Fatalf("stranger view: webhook_url leaked: %v", *strangerView.Triggers[0].WebhookURL)
	}
}
