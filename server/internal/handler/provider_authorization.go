package handler

import (
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/auth"
	"github.com/patchbay-ai/patchbay/server/internal/service"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// Provider authorization is the control plane over one runtime's stored
// provider credential. The credential itself never leaves the runtime; what
// these endpoints decide is who may cause it to be spent, on which models, and
// up to what token budget — and every decision is written to the append-only
// explain ledger so "why did that agent get to use my Claude subscription?"
// has an answer that is not a guess.

type providerGrantRequest struct {
	GranteeType string   `json:"grantee_type"`
	GranteeID   string   `json:"grantee_id"`
	RuntimeID   string   `json:"runtime_id"`
	Actions     []string `json:"actions"`
	Models      []string `json:"models"`
	MaxTokens   *int64   `json:"max_tokens"`
	ExpiresAt   string   `json:"expires_at"`
	TaskID      string   `json:"task_id"`
	Effect      string   `json:"effect"`
}

type providerGrantResponse struct {
	ID            string          `json:"id"`
	GranteeType   string          `json:"grantee_type"`
	GranteeID     string          `json:"grantee_id"`
	RuntimeID     string          `json:"runtime_id"`
	Action        string          `json:"action"`
	Effect        string          `json:"effect"`
	Conditions    json.RawMessage `json:"conditions"`
	ExpiresAt     *string         `json:"expires_at"`
	RevokedAt     *string         `json:"revoked_at"`
	CreatedBy     string          `json:"created_by"`
	CreatedAt     *string         `json:"created_at"`
	PolicyVersion string          `json:"policy_version"`
}

type providerDecisionResponse struct {
	Allowed         bool     `json:"allowed"`
	Decision        string   `json:"decision"`
	Reason          string   `json:"reason"`
	DecisionID      string   `json:"decision_id"`
	PolicyVersion   string   `json:"policy_version"`
	MatchedGrantIDs []string `json:"matched_grant_ids"`
}

type providerDecisionExplainResponse struct {
	providerDecisionResponse
	PrincipalType    string          `json:"principal_type"`
	PrincipalID      string          `json:"principal_id"`
	OnBehalfOfUserID string          `json:"on_behalf_of_user_id"`
	ViaAgentID       string          `json:"via_agent_id"`
	DeviceID         string          `json:"device_id"`
	Action           string          `json:"action"`
	ResourceType     string          `json:"resource_type"`
	ResourceID       string          `json:"resource_id"`
	Context          json.RawMessage `json:"context"`
	CreatedAt        *string         `json:"created_at"`
}

type providerAuthorizeRequest struct {
	// LeaseToken is the mat_ capability lease the daemon was handed when it
	// claimed the task. Presenting the bearer rather than an id is what makes
	// this a capability check: a caller that never held the lease cannot ask
	// for a decision about it, even knowing the task id.
	LeaseToken string `json:"lease_token"`
	// LeaseID names the same lease for a caller that holds the id instead of
	// the secret (the revocation UI, and the daemon once a future claim
	// response carries it). It is accepted because the decision itself is
	// bound to the task/agent/runtime/actor read server-side, not to how the
	// lease was named.
	LeaseID   string `json:"lease_id"`
	Provider  string `json:"provider"`
	Model     string `json:"model"`
	MaxTokens int64  `json:"max_tokens"`
}

func timestampPtr(ts pgtype.Timestamptz) *string {
	if !ts.Valid {
		return nil
	}
	formatted := ts.Time.UTC().Format(time.RFC3339)
	return &formatted
}

func providerGrantToResponse(grant db.AuthorizationGrant) providerGrantResponse {
	conditions := json.RawMessage(grant.Conditions)
	if len(conditions) == 0 {
		conditions = json.RawMessage(`{}`)
	}
	return providerGrantResponse{
		ID:            uuidToString(grant.ID),
		GranteeType:   grant.PrincipalType,
		GranteeID:     uuidToString(grant.PrincipalID),
		RuntimeID:     uuidToString(grant.ResourceID),
		Action:        grant.Action,
		Effect:        grant.Effect,
		Conditions:    conditions,
		ExpiresAt:     timestampPtr(grant.ExpiresAt),
		RevokedAt:     timestampPtr(grant.RevokedAt),
		CreatedBy:     uuidToString(grant.CreatedBy),
		CreatedAt:     timestampPtr(grant.CreatedAt),
		PolicyVersion: service.ProviderAuthorizationPolicyVersion,
	}
}

func decisionToResponse(decision service.ProviderAuthorizationDecision) providerDecisionResponse {
	ids := make([]string, 0, len(decision.MatchedGrantIDs))
	for _, id := range decision.MatchedGrantIDs {
		ids = append(ids, uuidToString(id))
	}
	return providerDecisionResponse{
		Allowed:         decision.Allowed,
		Decision:        decision.Effect,
		Reason:          decision.Reason,
		DecisionID:      uuidToString(decision.DecisionID),
		PolicyVersion:   decision.PolicyVersion,
		MatchedGrantIDs: ids,
	}
}

// providerAuthorizationActor resolves the workspace-scoped human behind a
// control-plane call. Grants over a provider identity are only ever made by a
// member of the workspace the runtime lives in, and the member row is what
// carries the role the owner-only paths check.
func (h *Handler) providerAuthorizationActor(w http.ResponseWriter, r *http.Request) (pgtype.UUID, db.Member, bool) {
	userID, ok := requireUserID(w, r)
	if !ok {
		return pgtype.UUID{}, db.Member{}, false
	}
	workspaceID := ctxWorkspaceID(r.Context())
	member, ok := h.workspaceMember(w, r, workspaceID)
	if !ok {
		return pgtype.UUID{}, db.Member{}, false
	}
	actorID, ok := parseUUIDOrBadRequest(w, userID, "user id")
	if !ok {
		return pgtype.UUID{}, db.Member{}, false
	}
	return actorID, member, true
}

func writeProviderAuthorizationError(w http.ResponseWriter, err error) {
	switch {
	case errors.Is(err, service.ErrProviderAuthorizationForbidden):
		writeError(w, http.StatusForbidden, "you do not control this provider identity")
	case errors.Is(err, service.ErrProviderAuthorizationNotFound):
		writeError(w, http.StatusNotFound, "provider authorization record not found")
	default:
		writeError(w, http.StatusBadRequest, err.Error())
	}
}

// CreateProviderAuthorizationGrant records one grant over a runtime's provider
// identity. Only the runtime owner can create one — the service checks that
// rather than the router, because "owns the credential" is not the same
// question as "is a workspace admin".
func (h *Handler) CreateProviderAuthorizationGrant(w http.ResponseWriter, r *http.Request) {
	actorID, member, ok := h.providerAuthorizationActor(w, r)
	if !ok {
		return
	}
	var request providerGrantRequest
	if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	granteeID, ok := parseUUIDOrBadRequest(w, request.GranteeID, "grantee_id")
	if !ok {
		return
	}
	runtimeID, ok := parseUUIDOrBadRequest(w, request.RuntimeID, "runtime_id")
	if !ok {
		return
	}
	expiresAt, err := time.Parse(time.RFC3339, request.ExpiresAt)
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid expires_at")
		return
	}
	input := service.ProviderGrantInput{
		GranteeType:    request.GranteeType,
		GranteeID:      granteeID,
		RuntimeID:      runtimeID,
		AllowedActions: request.Actions,
		Models:         request.Models,
		MaxTokens:      request.MaxTokens,
		ExpiresAt:      expiresAt,
		Effect:         request.Effect,
	}
	if request.TaskID != "" {
		taskID, taskOK := parseUUIDOrBadRequest(w, request.TaskID, "task_id")
		if !taskOK {
			return
		}
		input.TaskID = taskID
	}
	grant, err := h.ProviderAuthorization.CreateGrant(r.Context(), member.WorkspaceID, actorID, input)
	if err != nil {
		writeProviderAuthorizationError(w, err)
		return
	}
	writeJSON(w, http.StatusCreated, providerGrantToResponse(grant))
}

// ListProviderAuthorizationGrants returns the grants the caller made plus the
// ones that name them. A member cannot enumerate grants they are not party to:
// the grant ledger describes who may spend someone else's credential, which is
// exactly the shape of information worth keeping narrow.
func (h *Handler) ListProviderAuthorizationGrants(w http.ResponseWriter, r *http.Request) {
	actorID, member, ok := h.providerAuthorizationActor(w, r)
	if !ok {
		return
	}
	grants, err := h.ProviderAuthorization.ListGrants(r.Context(), member.WorkspaceID, actorID)
	if err != nil {
		slog.Warn("list provider authorization grants failed", "error", err)
		writeError(w, http.StatusInternalServerError, "failed to load provider authorizations")
		return
	}
	out := make([]providerGrantResponse, 0, len(grants))
	for _, grant := range grants {
		out = append(out, providerGrantToResponse(grant))
	}
	writeJSON(w, http.StatusOK, map[string]any{"grants": out})
}

// RevokeProviderAuthorizationGrant retires a grant. Revocation takes effect on
// the next decision rather than on any lease already issued, so the lease
// revoke path exists alongside it.
func (h *Handler) RevokeProviderAuthorizationGrant(w http.ResponseWriter, r *http.Request) {
	actorID, member, ok := h.providerAuthorizationActor(w, r)
	if !ok {
		return
	}
	grantID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "grantId"), "grant id")
	if !ok {
		return
	}
	if err := h.ProviderAuthorization.RevokeGrant(r.Context(), member.WorkspaceID, actorID, grantID); err != nil {
		writeProviderAuthorizationError(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// ExplainProviderAuthorizationDecision replays one recorded decision.
func (h *Handler) ExplainProviderAuthorizationDecision(w http.ResponseWriter, r *http.Request) {
	actorID, member, ok := h.providerAuthorizationActor(w, r)
	if !ok {
		return
	}
	decisionID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "decisionId"), "decision id")
	if !ok {
		return
	}
	event, err := h.ProviderAuthorization.Explain(r.Context(), member.WorkspaceID, actorID, member.Role, decisionID)
	if err != nil {
		writeProviderAuthorizationError(w, err)
		return
	}
	ids := make([]string, 0, len(event.MatchedGrantIds))
	for _, id := range event.MatchedGrantIds {
		ids = append(ids, uuidToString(id))
	}
	context := json.RawMessage(event.Context)
	if len(context) == 0 {
		context = json.RawMessage(`{}`)
	}
	writeJSON(w, http.StatusOK, providerDecisionExplainResponse{
		providerDecisionResponse: providerDecisionResponse{
			Allowed:         event.Decision == "allow",
			Decision:        event.Decision,
			Reason:          event.Reason,
			DecisionID:      uuidToString(event.ID),
			PolicyVersion:   event.PolicyVersion,
			MatchedGrantIDs: ids,
		},
		PrincipalType:    event.PrincipalType,
		PrincipalID:      uuidToString(event.PrincipalID),
		OnBehalfOfUserID: uuidToString(event.OnBehalfOfUserID),
		ViaAgentID:       uuidToString(event.ViaAgentID),
		DeviceID:         uuidToString(event.DeviceID),
		Action:           event.Action,
		ResourceType:     event.ResourceType,
		ResourceID:       uuidToString(event.ResourceID),
		Context:          context,
		CreatedAt:        timestampPtr(event.CreatedAt),
	})
}

// RevokeProviderCapabilityLease kills one live lease. This is the stop button:
// retiring a grant changes what the next decision says, but a task already
// holding a lease keeps it until the lease is revoked or its claim is fenced.
func (h *Handler) RevokeProviderCapabilityLease(w http.ResponseWriter, r *http.Request) {
	actorID, member, ok := h.providerAuthorizationActor(w, r)
	if !ok {
		return
	}
	leaseID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "leaseId"), "lease id")
	if !ok {
		return
	}
	reason := r.URL.Query().Get("reason")
	if err := h.ProviderAuthorization.RevokeLease(r.Context(), member.WorkspaceID, actorID, leaseID, member.Role, reason); err != nil {
		writeProviderAuthorizationError(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// AuthorizeProviderOperation is the daemon's pre-operation gate: nothing that
// spends a provider credential runs until this returns 200.
//
// Everything the decision is made from is read server-side from the task,
// runtime and lease rows — the daemon names the lease it holds and the model it
// is about to call, and nothing else it sends can widen the outcome. A denied
// or approval-pending decision comes back as 403 with the same body an allow
// carries, so a client that only looks at the status code still fails closed.
func (h *Handler) AuthorizeProviderOperation(w http.ResponseWriter, r *http.Request) {
	runtimeID := chi.URLParam(r, "runtimeId")
	taskID := chi.URLParam(r, "taskId")

	runtime, ok := h.requireDaemonRuntimeAccess(w, r, runtimeID)
	if !ok {
		return
	}
	task, taskWorkspaceID, ok := h.requireDaemonTaskAccessWithWorkspace(w, r, taskID)
	if !ok {
		return
	}
	if taskWorkspaceID != uuidToString(runtime.WorkspaceID) || task.RuntimeID != runtime.ID {
		writeError(w, http.StatusNotFound, "task not found")
		return
	}
	var request providerAuthorizeRequest
	if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	leaseID, ok := h.resolveProviderLeaseID(w, r, request)
	if !ok {
		return
	}
	if request.MaxTokens < 0 {
		writeError(w, http.StatusBadRequest, "invalid max_tokens")
		return
	}
	decision, err := h.ProviderAuthorization.ValidateLease(r.Context(), service.ProviderLeaseValidation{
		WorkspaceID:        runtime.WorkspaceID,
		LeaseID:            leaseID,
		TaskID:             task.ID,
		AgentID:            task.AgentID,
		RuntimeID:          runtime.ID,
		OnBehalfOfUserID:   task.OriginatorUserID,
		Provider:           request.Provider,
		Model:              request.Model,
		RequestedMaxTokens: request.MaxTokens,
	})
	if err != nil {
		slog.Warn("provider authorization decision failed",
			"task_id", taskID, "runtime_id", runtimeID, "error", err)
		writeError(w, http.StatusInternalServerError, "failed to evaluate provider authorization")
		return
	}
	status := http.StatusOK
	if !decision.Allowed {
		status = http.StatusForbidden
	}
	writeJSON(w, status, decisionToResponse(decision))
}

// resolveProviderLeaseID turns whichever form of the lease the caller
// presented into an id.
//
// An unrecognized bearer deliberately resolves to the zero id rather than to a
// 401: the decision path then denies it and writes that denial to the explain
// ledger, which is the record an operator needs when a runtime starts
// presenting leases the server has already forgotten. Returning early here
// would make exactly that case the one event nobody can look up.
func (h *Handler) resolveProviderLeaseID(w http.ResponseWriter, r *http.Request, request providerAuthorizeRequest) (pgtype.UUID, bool) {
	if token := strings.TrimSpace(request.LeaseToken); token != "" {
		lease, err := h.Queries.GetTaskTokenByHash(r.Context(), auth.HashToken(token))
		if err != nil {
			return pgtype.UUID{}, true
		}
		return lease.ID, true
	}
	if strings.TrimSpace(request.LeaseID) == "" {
		writeError(w, http.StatusBadRequest, "lease_token or lease_id is required")
		return pgtype.UUID{}, false
	}
	return parseUUIDOrBadRequest(w, request.LeaseID, "lease_id")
}
