package handler

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/auth"
	"github.com/patchbay-ai/patchbay/server/internal/service"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// Provider authorization is the control plane over one runtime's stored
// provider credential. The credential itself never leaves the runtime; what
// these endpoints decide is who may cause it to be spent, on which models, and
// up to what token budget — and every decision is written to the append-only
// explain ledger so "why did that agent get to use my Claude subscription?"
// has an answer that is not a guess.

type providerGrantRequest struct {
	GranteeType    string   `json:"grantee_type"`
	GranteeID      string   `json:"grantee_id"`
	RuntimeID      string   `json:"runtime_id"`
	Actions        []string `json:"actions"`
	AllowedActions []string `json:"allowed_actions"`
	Models         []string `json:"models"`
	MaxTokens      *int64   `json:"max_tokens"`
	ExpiresAt      string   `json:"expires_at"`
	TaskID         string   `json:"task_id"`
	Effect         string   `json:"effect"`
}

func (request providerGrantRequest) allowedActionList() []string {
	if len(request.AllowedActions) > 0 {
		return request.AllowedActions
	}
	return request.Actions
}

type providerGrantResponse struct {
	ID            string          `json:"id"`
	WorkspaceID   string          `json:"workspace_id"`
	GranteeType   string          `json:"grantee_type"`
	GranteeID     string          `json:"grantee_id"`
	RuntimeID     string          `json:"runtime_id"`
	Provider      string          `json:"provider"`
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
	Obligations      json.RawMessage `json:"obligations"`
	DelegationChain  json.RawMessage `json:"delegation_chain"`
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
	RuntimeID string `json:"runtime_id"`
	Provider  string `json:"provider"`
	Model     string `json:"model"`
	MaxTokens int64  `json:"max_tokens"`
	// RequestedMaxTokens is the canonical Rust field name. MaxTokens remains a
	// compatibility alias for the existing Go daemon client.
	RequestedMaxTokens *int64 `json:"requested_max_tokens"`
	// Preflight is used by the daemon at task start. It writes an allow decision
	// without reserving a provider budget; later operation validations reserve
	// atomically in the audit ledger.
	Preflight bool `json:"preflight,omitempty"`
}

func (request providerAuthorizeRequest) requestedMaxTokens() int64 {
	if request.RequestedMaxTokens != nil {
		return *request.RequestedMaxTokens
	}
	return request.MaxTokens
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
		WorkspaceID:   uuidToString(grant.WorkspaceID),
		GranteeType:   grant.PrincipalType,
		GranteeID:     uuidToString(grant.PrincipalID),
		RuntimeID:     uuidToString(grant.ResourceID),
		Provider:      providerFromGrantConditions(conditions),
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

func providerFromGrantConditions(conditions json.RawMessage) string {
	var fields map[string]json.RawMessage
	if json.Unmarshal(conditions, &fields) != nil {
		return ""
	}
	var provider string
	if json.Unmarshal(fields["provider"], &provider) != nil {
		return ""
	}
	return provider
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
	case errors.Is(err, pgx.ErrNoRows):
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
		AllowedActions: request.allowedActionList(),
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
		Obligations:      json.RawMessage(event.Obligations),
		DelegationChain:  json.RawMessage(event.DelegationChain),
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

type taskCapabilityRequestContext struct {
	Task  db.AgentTaskQueue
	Lease db.GetValidTaskCapabilityLeaseRow
}

// resolveTaskCapabilityContext is the Go adapter for Rust's
// TaskAuthorizationContext. The task token middleware has already authenticated
// the bearer; this second lookup binds task-scoped consumers to the current
// claim fence and obtains the lease id without accepting a client-provided id.
func (h *Handler) resolveTaskCapabilityContext(ctx context.Context, r *http.Request, workspaceID pgtype.UUID) (taskCapabilityRequestContext, bool, error) {
	if r.Header.Get("X-Actor-Source") != "task_token" {
		return taskCapabilityRequestContext{}, false, nil
	}
	taskID, err := util.ParseUUID(strings.TrimSpace(r.Header.Get("X-Task-ID")))
	if err != nil {
		return taskCapabilityRequestContext{}, true, fmt.Errorf("invalid task capability task id: %w", err)
	}
	agentID, err := util.ParseUUID(strings.TrimSpace(r.Header.Get("X-Agent-ID")))
	if err != nil {
		return taskCapabilityRequestContext{}, true, fmt.Errorf("invalid task capability agent id: %w", err)
	}
	task, err := h.Queries.GetAgentTaskInWorkspace(ctx, db.GetAgentTaskInWorkspaceParams{ID: taskID, WorkspaceID: workspaceID})
	if err != nil {
		return taskCapabilityRequestContext{}, true, fmt.Errorf("load task capability task: %w", err)
	}
	if task.AgentID != agentID || !task.RuntimeID.Valid || !task.OriginatorUserID.Valid {
		return taskCapabilityRequestContext{}, true, service.ErrProviderAuthorizationForbidden
	}
	current, err := h.Queries.GetCurrentTaskCapabilityLease(ctx, db.GetCurrentTaskCapabilityLeaseParams{
		TaskID: task.ID, WorkspaceID: workspaceID, ClaimDispatchedAt: task.DispatchedAt,
	})
	if err != nil {
		return taskCapabilityRequestContext{}, true, fmt.Errorf("load current task capability lease: %w", err)
	}
	lease, err := h.Queries.GetValidTaskCapabilityLease(ctx, current.ID)
	if err != nil {
		return taskCapabilityRequestContext{}, true, fmt.Errorf("validate current task capability lease: %w", err)
	}
	if lease.TaskID != task.ID || lease.AgentID != task.AgentID || lease.WorkspaceID != workspaceID ||
		lease.DeviceID != task.RuntimeID || lease.OnBehalfOfUserID != task.OriginatorUserID {
		return taskCapabilityRequestContext{}, true, service.ErrProviderAuthorizationForbidden
	}
	return taskCapabilityRequestContext{Task: task, Lease: lease}, true, nil
}

// taskProjectResourceAllows is the Go adapter for Rust's
// task_project_resource_allows. Human requests retain the workspace middleware
// semantics; a task-token request must additionally name its own bound issue
// and hold the exact action/resource capability in the current lease.
func (h *Handler) taskProjectResourceAllows(w http.ResponseWriter, r *http.Request, issueID pgtype.UUID, requireBoundIssue bool, action string) bool {
	if r.Header.Get("X-Actor-Source") != "task_token" {
		return true
	}
	capability, present, err := h.resolveTaskCapabilityContext(r.Context(), r, parseUUID(ctxWorkspaceID(r.Context())))
	if !present || err != nil || (requireBoundIssue && (!capability.Task.IssueID.Valid || capability.Task.IssueID != issueID)) ||
		!service.TaskLeaseAllows(capability.Lease.Scope, action, auth.ResourceProject, uuidToString(issueID)) {
		writeError(w, http.StatusForbidden, "task capability does not allow this issue operation")
		return false
	}
	return true
}

// taskLeaseAllows is the adapter for Rust's task_lease_allows helper used by
// user-facing task message/cancellation routes. URL task identity is compared
// with the middleware-stamped current task before the lease scope is consulted;
// "$task" is deliberately an exact relative task resource, never a wildcard.
func (h *Handler) taskLeaseAllows(w http.ResponseWriter, r *http.Request, workspaceID, taskID pgtype.UUID, action string) bool {
	if r.Header.Get("X-Actor-Source") != "task_token" {
		return true
	}
	capability, present, err := h.resolveTaskCapabilityContext(r.Context(), r, workspaceID)
	if !present || err != nil || capability.Task.ID != taskID ||
		!service.TaskLeaseAllows(capability.Lease.Scope, action, auth.ResourceTaskRun, "$task") {
		writeError(w, http.StatusForbidden, "task capability does not allow this task operation")
		return false
	}
	return true
}

// authorizeProviderTaskClaim is called after the full claim payload has been
// built but before a task token is minted. A deny/approval result terminally
// settles the dispatched task so the daemon cannot retry the same unauthorized
// work forever; a database failure is returned to the claim caller and no lease
// is issued.
func (h *Handler) authorizeProviderTaskClaim(ctx context.Context, task db.AgentTaskQueue, runtime db.AgentRuntime) (bool, error) {
	if !service.ProviderUsesCredentialBroker(runtime.Provider) {
		return true, nil
	}
	agent, err := h.Queries.GetAgentInWorkspace(ctx, db.GetAgentInWorkspaceParams{ID: task.AgentID, WorkspaceID: runtime.WorkspaceID})
	if err != nil {
		return false, fmt.Errorf("load provider claim agent: %w", err)
	}
	model := ""
	if agent.Model.Valid {
		model = strings.TrimSpace(agent.Model.String)
	}
	decision, err := h.ProviderAuthorization.AuthorizeTaskClaim(ctx, service.ProviderClaimValidation{
		WorkspaceID: runtime.WorkspaceID, TaskID: task.ID, AgentID: task.AgentID,
		RuntimeID: runtime.ID, OnBehalfOfUserID: task.OriginatorUserID,
		Provider: runtime.Provider, Model: model,
	})
	if err != nil {
		return false, err
	}
	if decision.Allowed {
		return true, nil
	}
	if _, err := h.TaskService.CancelTaskWithReason(ctx, task.ID, decision.Reason, "authorization_denied"); err != nil {
		return false, fmt.Errorf("settle provider authorization denial: %w", err)
	}
	return false, nil
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
	if request.requestedMaxTokens() < 0 {
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
		RequestedMaxTokens: request.requestedMaxTokens(),
		Preflight:          request.Preflight,
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

// ValidateProviderLease is the canonical Rust-compatible task-token route.
// Unlike the daemon compatibility route above, task/runtime identity comes from
// server-stamped headers and the request carries only runtime/provider/model and
// requested_max_tokens.
func (h *Handler) ValidateProviderLease(w http.ResponseWriter, r *http.Request) {
	workspaceID, err := util.ParseUUID(ctxWorkspaceID(r.Context()))
	if err != nil {
		writeError(w, http.StatusForbidden, "provider lease workspace is required")
		return
	}
	capability, present, err := h.resolveTaskCapabilityContext(r.Context(), r, workspaceID)
	if !present || err != nil {
		writeError(w, http.StatusForbidden, "task capability lease required")
		return
	}
	var request providerAuthorizeRequest
	if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	runtimeID, ok := parseUUIDOrBadRequest(w, request.RuntimeID, "runtime_id")
	if !ok {
		return
	}
	if runtimeID != capability.Task.RuntimeID {
		writeError(w, http.StatusForbidden, "provider lease identity mismatch")
		return
	}
	maxTokens := request.requestedMaxTokens()
	if maxTokens < 0 {
		writeError(w, http.StatusBadRequest, "invalid requested_max_tokens")
		return
	}
	decision, err := h.ProviderAuthorization.ValidateLease(r.Context(), service.ProviderLeaseValidation{
		WorkspaceID: workspaceID,
		LeaseID: capability.Lease.ID, TaskID: capability.Task.ID,
		AgentID: capability.Task.AgentID, RuntimeID: capability.Task.RuntimeID,
		OnBehalfOfUserID: capability.Task.OriginatorUserID,
		Provider: request.Provider, Model: request.Model,
		RequestedMaxTokens: maxTokens, Preflight: request.Preflight,
	})
	if err != nil {
		slog.Warn("canonical provider authorization decision failed", "task_id", uuidToString(capability.Task.ID), "error", err)
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
