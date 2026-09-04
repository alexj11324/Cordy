package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"slices"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/auth"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
)

const ProviderAuthorizationPolicyVersion = auth.AuthorizationPolicyVersion

var (
	ErrProviderAuthorizationForbidden = errors.New("provider authorization forbidden")
	ErrProviderAuthorizationNotFound  = errors.New("provider authorization record not found")
)

type ProviderGrantInput struct {
	GranteeType    string
	GranteeID      pgtype.UUID
	RuntimeID      pgtype.UUID
	AllowedActions []string
	Models         []string
	MaxTokens      *int64
	ExpiresAt      time.Time
	TaskID         pgtype.UUID
	Effect         string
}

type ProviderLeaseValidation struct {
	WorkspaceID        pgtype.UUID
	LeaseID            pgtype.UUID
	TaskID             pgtype.UUID
	AgentID            pgtype.UUID
	RuntimeID          pgtype.UUID
	OnBehalfOfUserID   pgtype.UUID
	Provider           string
	Model              string
	RequestedMaxTokens int64
	// Preflight records a daemon claim-start check. It authorizes the provider
	// before the subprocess is spawned but intentionally does not reserve a
	// request budget; request-level reservations are made by non-preflight
	// validations through the atomic audit query.
	Preflight bool
	// DelegationChain is server-derived task lineage, never caller-provided.
	// It is kept on the input only between the evaluator and audit writer.
	DelegationChain []pgtype.UUID
}

// ProviderClaimValidation is the lease-free claim-time gate. Rust performs
// this provider/runtime authorization before it returns a task lease; Go must
// do the same so a task cannot be claimed and spawned before an owner deny or
// missing grant is observed.
type ProviderClaimValidation struct {
	WorkspaceID      pgtype.UUID
	TaskID           pgtype.UUID
	AgentID          pgtype.UUID
	RuntimeID        pgtype.UUID
	OnBehalfOfUserID pgtype.UUID
	Provider         string
	Model            string
}

type ProviderAuthorizationDecision struct {
	Allowed         bool          `json:"allowed"`
	Effect          string        `json:"decision"`
	Reason          string        `json:"reason"`
	DecisionID      pgtype.UUID   `json:"decision_id"`
	PolicyVersion   string        `json:"policy_version"`
	MatchedGrantIDs []pgtype.UUID `json:"matched_grant_ids"`
}

type ProviderAuthorizationService struct {
	Queries *db.Queries
}

func NewProviderAuthorizationService(queries *db.Queries) *ProviderAuthorizationService {
	return &ProviderAuthorizationService{Queries: queries}
}

type providerGrantConditions struct {
	Provider       string   `json:"provider"`
	ProviderAction string   `json:"provider_action"`
	DeviceID       string   `json:"device_id"`
	Models         []string `json:"models,omitempty"`
	MaxTokens      *int64   `json:"max_tokens,omitempty"`
	TaskID         string   `json:"task_id,omitempty"`
}

func uuidText(value pgtype.UUID) string {
	if !value.Valid {
		return ""
	}
	return fmt.Sprintf("%x-%x-%x-%x-%x", value.Bytes[0:4], value.Bytes[4:6], value.Bytes[6:8], value.Bytes[8:10], value.Bytes[10:16])
}

func validProviderGrantEffect(effect string) bool {
	return auth.ValidEffect(effect)
}

func (s *ProviderAuthorizationService) CreateGrant(ctx context.Context, workspaceID, actorID pgtype.UUID, input ProviderGrantInput) (db.AuthorizationGrant, error) {
	if !workspaceID.Valid || !actorID.Valid || !input.GranteeID.Valid || !input.RuntimeID.Valid {
		return db.AuthorizationGrant{}, fmt.Errorf("provider grant workspace, actor, grantee, and runtime are required")
	}
	input.GranteeType = strings.TrimSpace(input.GranteeType)
	input.Effect = strings.TrimSpace(input.Effect)
	if input.Effect == "" {
		input.Effect = "allow"
	}
	if !slices.Contains([]string{"user", "team", "agent_definition"}, input.GranteeType) {
		return db.AuthorizationGrant{}, fmt.Errorf("invalid provider grant grantee")
	}
	if !validProviderGrantEffect(input.Effect) {
		return db.AuthorizationGrant{}, fmt.Errorf("invalid provider grant effect")
	}
	if len(input.AllowedActions) != 1 || strings.TrimSpace(input.AllowedActions[0]) != "provider.invoke" {
		return db.AuthorizationGrant{}, fmt.Errorf("provider grants currently support only provider.invoke")
	}
	now := time.Now()
	if !input.ExpiresAt.After(now) || input.ExpiresAt.After(now.Add(30*24*time.Hour)) {
		return db.AuthorizationGrant{}, fmt.Errorf("provider grant expiry must be within the next 30 days")
	}
	if input.MaxTokens != nil && *input.MaxTokens <= 0 {
		return db.AuthorizationGrant{}, fmt.Errorf("provider token budget must be positive")
	}
	models := make([]string, 0, len(input.Models))
	for _, model := range input.Models {
		if model = strings.TrimSpace(model); model != "" {
			models = append(models, model)
		}
	}
	slices.Sort(models)
	models = slices.Compact(models)
	if len(models) == 0 && input.MaxTokens == nil {
		return db.AuthorizationGrant{}, fmt.Errorf("provider grant requires models or a token budget")
	}
	runtime, err := s.Queries.GetAgentRuntimeForWorkspace(ctx, db.GetAgentRuntimeForWorkspaceParams{ID: input.RuntimeID, WorkspaceID: workspaceID})
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return db.AuthorizationGrant{}, ErrProviderAuthorizationNotFound
		}
		return db.AuthorizationGrant{}, err
	}
	if !runtime.OwnerID.Valid || runtime.OwnerID != actorID {
		return db.AuthorizationGrant{}, ErrProviderAuthorizationForbidden
	}
	if runtime.Provider != "codex" && runtime.Provider != "claude" {
		return db.AuthorizationGrant{}, fmt.Errorf("provider credential control plane is unavailable for this runtime")
	}
	switch input.GranteeType {
	case "user":
		_, err = s.Queries.GetMemberByUserAndWorkspace(ctx, db.GetMemberByUserAndWorkspaceParams{UserID: input.GranteeID, WorkspaceID: workspaceID})
	case "team":
		_, err = s.Queries.GetTeamInWorkspace(ctx, db.GetTeamInWorkspaceParams{ID: input.GranteeID, WorkspaceID: workspaceID})
	case "agent_definition":
		_, err = s.Queries.GetAgentInWorkspace(ctx, db.GetAgentInWorkspaceParams{ID: input.GranteeID, WorkspaceID: workspaceID})
	}
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return db.AuthorizationGrant{}, fmt.Errorf("provider grant grantee not found")
		}
		return db.AuthorizationGrant{}, err
	}
	if input.TaskID.Valid {
		task, taskErr := s.Queries.GetAgentTask(ctx, input.TaskID)
		if taskErr != nil || task.RuntimeID != input.RuntimeID || !task.ID.Valid {
			return db.AuthorizationGrant{}, fmt.Errorf("provider grant task not found")
		}
		// The task row itself carries no workspace column, so containment is
		// proven through its Agent. Without this a grant could be pinned to a
		// task in another workspace and then matched by conditions.task_id.
		if _, agentErr := s.Queries.GetAgentInWorkspace(ctx, db.GetAgentInWorkspaceParams{ID: task.AgentID, WorkspaceID: workspaceID}); agentErr != nil {
			return db.AuthorizationGrant{}, fmt.Errorf("provider grant task not found")
		}
	}
	conditions := providerGrantConditions{
		Provider: runtime.Provider, ProviderAction: "provider.invoke",
		DeviceID: uuidText(runtime.ID), Models: models, MaxTokens: input.MaxTokens,
	}
	if input.TaskID.Valid {
		conditions.TaskID = uuidText(input.TaskID)
	}
	raw, err := json.Marshal(conditions)
	if err != nil {
		return db.AuthorizationGrant{}, err
	}
	return s.Queries.CreateProviderAuthorizationGrant(ctx, db.CreateProviderAuthorizationGrantParams{
		ID: dbid.NewV7(), WorkspaceID: workspaceID, PrincipalType: input.GranteeType,
		PrincipalID: input.GranteeID, ResourceID: input.RuntimeID, Effect: input.Effect,
		Conditions: raw, ExpiresAt: pgtype.Timestamptz{Time: input.ExpiresAt, Valid: true}, CreatedBy: actorID,
	})
}

func (s *ProviderAuthorizationService) ListGrants(ctx context.Context, workspaceID, actorID pgtype.UUID) ([]db.AuthorizationGrant, error) {
	return s.Queries.ListProviderAuthorizationGrants(ctx, db.ListProviderAuthorizationGrantsParams{WorkspaceID: workspaceID, ActorID: actorID})
}

func (s *ProviderAuthorizationService) RevokeGrant(ctx context.Context, workspaceID, actorID, grantID pgtype.UUID) error {
	rows, err := s.Queries.RevokeProviderAuthorizationGrant(ctx, db.RevokeProviderAuthorizationGrantParams{ID: grantID, WorkspaceID: workspaceID, ActorID: actorID})
	if err != nil {
		return err
	}
	if rows != 1 {
		return ErrProviderAuthorizationNotFound
	}
	return nil
}

func (s *ProviderAuthorizationService) Explain(ctx context.Context, workspaceID, actorID pgtype.UUID, workspaceRole string, decisionID pgtype.UUID) (db.AuthorizationAuditEvent, error) {
	event, err := s.Queries.GetAuthorizationDecision(ctx, db.GetAuthorizationDecisionParams{ID: decisionID, WorkspaceID: workspaceID})
	if err != nil {
		return db.AuthorizationAuditEvent{}, err
	}
	if workspaceRole != "owner" && event.PrincipalID != actorID && event.OnBehalfOfUserID != actorID {
		return db.AuthorizationAuditEvent{}, ErrProviderAuthorizationNotFound
	}
	return event, nil
}

func grantPrincipalMatches(grant db.AuthorizationGrant, originator, agentID, runtimeID pgtype.UUID, taskIDs map[pgtype.UUID]struct{}, teamIDs map[pgtype.UUID]struct{}) bool {
	switch grant.PrincipalType {
	case "user":
		return grant.PrincipalID.Valid && grant.PrincipalID == originator
	case "team":
		_, ok := teamIDs[grant.PrincipalID]
		return grant.PrincipalID.Valid && ok
	case "agent_definition":
		return grant.PrincipalID.Valid && grant.PrincipalID == agentID
	case "task_run":
		_, ok := taskIDs[grant.PrincipalID]
		return grant.PrincipalID.Valid && ok
	case "device_runtime":
		return grant.PrincipalID.Valid && grant.PrincipalID == runtimeID
	default:
		return false
	}
}

// decodeProviderGrantConditions is deliberately allow-listed. json.Unmarshal
// into a struct ignores unknown keys, which would make a future condition look
// accepted even though the evaluator never enforced it. Rust treats unknown
// conditions as non-matching; preserving that fail-closed rule is important for
// database rows written by a newer policy version.
func decodeProviderGrantConditions(raw []byte) (providerGrantConditions, bool) {
	var fields map[string]json.RawMessage
	if len(raw) == 0 || json.Unmarshal(raw, &fields) != nil || fields == nil {
		return providerGrantConditions{}, false
	}
	allowed := map[string]struct{}{
		"provider": {}, "provider_action": {}, "device_id": {}, "models": {},
		"max_tokens": {}, "task_id": {},
	}
	for key := range fields {
		if _, ok := allowed[key]; !ok {
			return providerGrantConditions{}, false
		}
	}
	var conditions providerGrantConditions
	decodeString := func(key string, target *string, required bool) bool {
		rawValue, present := fields[key]
		if !present {
			return !required
		}
		if json.Unmarshal(rawValue, target) != nil || (required && strings.TrimSpace(*target) == "") {
			return false
		}
		return true
	}
	if !decodeString("provider", &conditions.Provider, true) ||
		!decodeString("provider_action", &conditions.ProviderAction, true) ||
		!decodeString("device_id", &conditions.DeviceID, true) {
		return providerGrantConditions{}, false
	}
	if rawModels, present := fields["models"]; present {
		if json.Unmarshal(rawModels, &conditions.Models) != nil || len(conditions.Models) == 0 {
			return providerGrantConditions{}, false
		}
		seen := make(map[string]struct{}, len(conditions.Models))
		for _, model := range conditions.Models {
			if strings.TrimSpace(model) == "" {
				return providerGrantConditions{}, false
			}
			if _, duplicate := seen[model]; duplicate {
				return providerGrantConditions{}, false
			}
			seen[model] = struct{}{}
		}
	}
	if rawMax, present := fields["max_tokens"]; present {
		var maxTokens int64
		if json.Unmarshal(rawMax, &maxTokens) != nil || maxTokens <= 0 {
			return providerGrantConditions{}, false
		}
		conditions.MaxTokens = &maxTokens
	}
	if _, present := fields["task_id"]; present {
		if !decodeString("task_id", &conditions.TaskID, true) {
			return providerGrantConditions{}, false
		}
	}
	return conditions, true
}

func grantConditionsMatch(grant db.AuthorizationGrant, input ProviderLeaseValidation, delegated bool) (providerGrantConditions, bool) {
	conditions, ok := decodeProviderGrantConditions(grant.Conditions)
	if !ok {
		return conditions, false
	}
	if conditions.Provider != input.Provider || conditions.ProviderAction != "provider.invoke" || conditions.DeviceID != uuidText(input.RuntimeID) {
		return conditions, false
	}
	if len(conditions.Models) > 0 && !slices.Contains(conditions.Models, input.Model) {
		return conditions, false
	}
	if conditions.MaxTokens != nil && input.RequestedMaxTokens > *conditions.MaxTokens {
		return conditions, false
	}
	if conditions.TaskID != "" && conditions.TaskID != uuidText(input.TaskID) {
		return conditions, false
	}
	if delegated && grant.Effect == auth.EffectAllow && conditions.TaskID != uuidText(input.TaskID) {
		return conditions, false
	}
	return conditions, true
}

// ProviderUsesCredentialBroker is the explicit compatibility boundary for the
// provider-identity control plane. The Rust authorization endpoint only brokers
// Codex and Claude credentials; other Go runtimes use their own local provider
// login and have no provider_identity grant contract to evaluate.
func ProviderUsesCredentialBroker(provider string) bool {
	return provider == "codex" || provider == "claude"
}

type providerGrantEvaluation struct {
	GrantIDs          []pgtype.UUID
	Effects           []string
	Budget            int64
	Bounded           bool
	HasUnboundedAllow bool
}

func (s *ProviderAuthorizationService) taskLineage(ctx context.Context, task db.AgentTaskQueue) (map[pgtype.UUID]struct{}, []pgtype.UUID, error) {
	ids := make(map[pgtype.UUID]struct{})
	ordered := make([]pgtype.UUID, 0, auth.MaxDelegationDepth+1)
	current := task
	for depth := 0; ; depth++ {
		if depth > auth.MaxDelegationDepth || !current.ID.Valid {
			return nil, nil, auth.ErrAuthorizationDelegationDepth
		}
		if _, exists := ids[current.ID]; exists {
			return nil, nil, auth.ErrAuthorizationDelegationCycle
		}
		ids[current.ID] = struct{}{}
		ordered = append(ordered, current.ID)
		if !current.DelegatedFromTaskID.Valid {
			break
		}
		parent, err := s.Queries.GetAgentTask(ctx, current.DelegatedFromTaskID)
		if err != nil {
			return nil, nil, fmt.Errorf("load delegated provider task: %w", err)
		}
		if parent.AgentID != task.AgentID || parent.RuntimeID != task.RuntimeID || parent.OriginatorUserID != task.OriginatorUserID {
			return nil, nil, auth.ErrAuthorizationLeaseMismatch
		}
		current = parent
	}
	return ids, ordered, nil
}

func (s *ProviderAuthorizationService) matchProviderGrants(ctx context.Context, input ProviderLeaseValidation, task db.AgentTaskQueue, runtime db.AgentRuntime, teamIDs map[pgtype.UUID]struct{}, taskIDs map[pgtype.UUID]struct{}) (providerGrantEvaluation, error) {
	grants, err := s.Queries.ListActiveProviderAuthorizationGrants(ctx, db.ListActiveProviderAuthorizationGrantsParams{WorkspaceID: input.WorkspaceID, RuntimeID: input.RuntimeID})
	if err != nil {
		return providerGrantEvaluation{}, err
	}
	evaluation := providerGrantEvaluation{GrantIDs: make([]pgtype.UUID, 0), Effects: make([]string, 0)}
	delegated := task.DelegatedFromTaskID.Valid
	for _, grant := range grants {
		if !auth.ValidEffect(grant.Effect) || grant.CreatedBy != runtime.OwnerID ||
			!grantPrincipalMatches(grant, input.OnBehalfOfUserID, input.AgentID, input.RuntimeID, taskIDs, teamIDs) {
			continue
		}
		conditions, matches := grantConditionsMatch(grant, input, delegated)
		if !matches {
			continue
		}
		evaluation.GrantIDs = append(evaluation.GrantIDs, grant.ID)
		evaluation.Effects = append(evaluation.Effects, grant.Effect)
		if grant.Effect != auth.EffectAllow {
			continue
		}
		if conditions.MaxTokens == nil {
			evaluation.HasUnboundedAllow = true
			continue
		}
		evaluation.Bounded = true
		if *conditions.MaxTokens > evaluation.Budget {
			evaluation.Budget = *conditions.MaxTokens
		}
	}
	return evaluation, nil
}

func (s *ProviderAuthorizationService) recordDecision(ctx context.Context, input ProviderLeaseValidation, effect, reason string, grantIDs []pgtype.UUID, reservation int64) (ProviderAuthorizationDecision, error) {
	return s.recordDecisionBudget(ctx, input, effect, reason, grantIDs, reservation, 0, false)
}

func (s *ProviderAuthorizationService) recordDecisionBudget(ctx context.Context, input ProviderLeaseValidation, effect, reason string, grantIDs []pgtype.UUID, reservation, budget int64, enforceBudget bool) (ProviderAuthorizationDecision, error) {
	decisionID := dbid.NewV7()
	if grantIDs == nil {
		grantIDs = []pgtype.UUID{}
	}
	delegationChain := make([]string, 0, len(input.DelegationChain))
	for _, id := range input.DelegationChain {
		if id.Valid {
			delegationChain = append(delegationChain, uuidText(id))
		}
	}
	chainJSON, err := json.Marshal(delegationChain)
	if err != nil {
		return ProviderAuthorizationDecision{}, fmt.Errorf("marshal provider delegation chain: %w", err)
	}
	obligations := []any{}
	if effect == auth.EffectRequireApproval {
		obligations = append(obligations, map[string]any{"type": "approval", "required": true})
	}
	obligationsJSON, err := json.Marshal(obligations)
	if err != nil {
		return ProviderAuthorizationDecision{}, fmt.Errorf("marshal provider obligations: %w", err)
	}
	contextPayload := map[string]any{
		"task_id":                     uuidText(input.TaskID),
		"lease_id":                    uuidText(input.LeaseID),
		"provider":                    input.Provider,
		"model":                       input.Model,
		"requested_max_tokens":        input.RequestedMaxTokens,
		"provider_request_tokens":     reservation,
		"provider_budget_reservation": effect == auth.EffectAllow && reservation > 0,
		"provider_preflight":          input.Preflight,
	}
	rawContext, err := json.Marshal(contextPayload)
	if err != nil {
		return ProviderAuthorizationDecision{}, fmt.Errorf("marshal provider audit context: %w", err)
	}
	event, err := s.Queries.CreateProviderAuthorizationDecision(ctx, db.CreateProviderAuthorizationDecisionParams{
		BudgetLockKey:         uuidText(input.WorkspaceID) + ":" + uuidText(input.RuntimeID),
		WorkspaceID:           input.WorkspaceID,
		ResourceID:            input.RuntimeID,
		MatchedGrantIds:       grantIDs,
		EnforceBudget:         enforceBudget,
		Decision:              effect,
		BudgetLimit:           budget,
		Reservation:           reservation,
		BudgetExhaustedReason: "provider token budget exhausted",
		Reason:                reason,
		ID:                    decisionID,
		PrincipalType:         auth.PrincipalTaskRun,
		PrincipalID:           input.TaskID,
		OnBehalfOfUserID:      input.OnBehalfOfUserID,
		ViaAgentID:            input.AgentID,
		DeviceID:              input.RuntimeID,
		Action:                auth.ActionCredentialUse,
		ResourceType:          auth.ResourceProviderIdentity,
		PolicyVersion:         ProviderAuthorizationPolicyVersion,
		Obligations:           obligationsJSON,
		DelegationChain:       chainJSON,
		Context:               rawContext,
	})
	if err != nil {
		return ProviderAuthorizationDecision{}, err
	}
	return ProviderAuthorizationDecision{Allowed: event.Decision == auth.EffectAllow, Effect: event.Decision, Reason: event.Reason, DecisionID: event.ID, PolicyVersion: event.PolicyVersion, MatchedGrantIDs: grantIDs}, nil
}

func (s *ProviderAuthorizationService) authorizeProviderRequest(ctx context.Context, input ProviderLeaseValidation, task db.AgentTaskQueue, leaseValid bool) (ProviderAuthorizationDecision, error) {
	if input.RequestedMaxTokens < 0 {
		return s.recordDecision(ctx, input, auth.EffectDeny, "requested provider token budget is invalid", nil, 0)
	}
	if !ProviderUsesCredentialBroker(input.Provider) {
		return s.recordDecision(ctx, input, auth.EffectDeny, "provider credential control plane is unavailable for this provider", nil, 0)
	}
	if !leaseValid {
		return s.recordDecision(ctx, input, auth.EffectDeny, "capability lease is invalid, expired, revoked, or outside provider scope", nil, 0)
	}
	if task.AgentID != input.AgentID || task.RuntimeID != input.RuntimeID || task.OriginatorUserID != input.OnBehalfOfUserID {
		return s.recordDecision(ctx, input, auth.EffectDeny, "provider lease task identity mismatch", nil, 0)
	}
	agent, err := s.Queries.GetAgentInWorkspace(ctx, db.GetAgentInWorkspaceParams{ID: input.AgentID, WorkspaceID: input.WorkspaceID})
	if err != nil || agent.ArchivedAt.Valid || agent.RuntimeID != input.RuntimeID {
		return s.recordDecision(ctx, input, auth.EffectDeny, "provider lease workspace or agent mismatch", nil, 0)
	}
	runtime, err := s.Queries.GetAgentRuntimeForWorkspace(ctx, db.GetAgentRuntimeForWorkspaceParams{ID: input.RuntimeID, WorkspaceID: input.WorkspaceID})
	if err != nil || runtime.Provider != input.Provider || !runtime.OwnerID.Valid {
		return s.recordDecision(ctx, input, auth.EffectDeny, "provider lease runtime or provider mismatch", nil, 0)
	}
	member, err := s.Queries.GetMemberByUserAndWorkspace(ctx, db.GetMemberByUserAndWorkspaceParams{UserID: input.OnBehalfOfUserID, WorkspaceID: input.WorkspaceID})
	if err != nil || !member.UserID.Valid {
		return s.recordDecision(ctx, input, auth.EffectDeny, "provider lease actor is not a current workspace member", nil, 0)
	}
	taskIDs, chain, err := s.taskLineage(ctx, task)
	if err != nil {
		return s.recordDecision(ctx, input, auth.EffectDeny, "provider task delegation chain is invalid", nil, 0)
	}
	input.DelegationChain = chain
	teams, err := s.Queries.ListTeamsByMember(ctx, db.ListTeamsByMemberParams{WorkspaceID: input.WorkspaceID, MemberType: "member", MemberID: input.OnBehalfOfUserID})
	if err != nil {
		return ProviderAuthorizationDecision{}, err
	}
	teamIDs := make(map[pgtype.UUID]struct{}, len(teams))
	for _, team := range teams {
		teamIDs[team.ID] = struct{}{}
	}
	evaluation, err := s.matchProviderGrants(ctx, input, task, runtime, teamIDs, taskIDs)
	if err != nil {
		return ProviderAuthorizationDecision{}, err
	}
	if slices.Contains(evaluation.Effects, auth.EffectDeny) {
		return s.recordDecision(ctx, input, auth.EffectDeny, "matched explicit deny grant", evaluation.GrantIDs, 0)
	}
	if slices.Contains(evaluation.Effects, auth.EffectRequireApproval) {
		return s.recordDecision(ctx, input, auth.EffectRequireApproval, "approval is required before provider use", evaluation.GrantIDs, 0)
	}
	ownerUse := runtime.OwnerID == input.OnBehalfOfUserID
	if !ownerUse && !slices.Contains(evaluation.Effects, auth.EffectAllow) {
		return s.recordDecision(ctx, input, auth.EffectDeny, "provider owner has not authorized this use", evaluation.GrantIDs, 0)
	}
	reservation := int64(0)
	if !ownerUse && !evaluation.HasUnboundedAllow && !input.Preflight {
		reservation = input.RequestedMaxTokens
	}
	if !ownerUse && evaluation.Bounded && !evaluation.HasUnboundedAllow && reservation == 0 && !input.Preflight {
		reservation = evaluation.Budget
	}
	enforceBudget := !ownerUse && evaluation.Bounded && !evaluation.HasUnboundedAllow && !input.Preflight
	return s.recordDecisionBudget(ctx, input, auth.EffectAllow, "active capability lease and provider grant allow action", evaluation.GrantIDs, reservation, evaluation.Budget, enforceBudget)
}

func (s *ProviderAuthorizationService) ValidateLease(ctx context.Context, input ProviderLeaseValidation) (ProviderAuthorizationDecision, error) {
	lease, err := s.Queries.GetValidTaskCapabilityLease(ctx, input.LeaseID)
	leaseValid := err == nil && LeaseAuthorizesProviderUse(lease, input.TaskID, input.AgentID, input.WorkspaceID, input.RuntimeID, input.OnBehalfOfUserID)
	if err != nil && !errors.Is(err, pgx.ErrNoRows) {
		return ProviderAuthorizationDecision{}, err
	}
	task, taskErr := s.Queries.GetAgentTask(ctx, input.TaskID)
	if taskErr != nil {
		return s.recordDecision(ctx, input, auth.EffectDeny, "provider lease task identity mismatch", nil, 0)
	}
	return s.authorizeProviderRequest(ctx, input, task, leaseValid)
}

func (s *ProviderAuthorizationService) AuthorizeTaskClaim(ctx context.Context, input ProviderClaimValidation) (ProviderAuthorizationDecision, error) {
	validation := ProviderLeaseValidation{
		WorkspaceID: input.WorkspaceID, TaskID: input.TaskID, AgentID: input.AgentID,
		RuntimeID: input.RuntimeID, OnBehalfOfUserID: input.OnBehalfOfUserID,
		Provider: input.Provider, Model: input.Model, Preflight: true,
	}
	task, err := s.Queries.GetAgentTask(ctx, input.TaskID)
	if err != nil {
		return s.recordDecision(ctx, validation, auth.EffectDeny, "provider claim task identity mismatch", nil, 0)
	}
	return s.authorizeProviderRequest(ctx, validation, task, true)
}

func (s *ProviderAuthorizationService) RevokeLease(ctx context.Context, workspaceID, actorID, leaseID pgtype.UUID, role, reason string) error {
	if role != "owner" {
		return ErrProviderAuthorizationForbidden
	}
	if _, err := s.Queries.GetTaskCapabilityLease(ctx, db.GetTaskCapabilityLeaseParams{ID: leaseID, WorkspaceID: workspaceID}); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return ErrProviderAuthorizationNotFound
		}
		return err
	}
	reason = strings.TrimSpace(reason)
	if reason == "" {
		reason = "revoked_by_workspace_owner"
	}
	// Zero rows means the lease was already revoked. Revocation is terminal and
	// the immutability trigger refuses to revive it, so the caller's intent
	// already holds; reporting "not found" for a second revoke would invite a
	// retry loop against a lease that can never change again.
	if _, err := s.Queries.RevokeTaskToken(ctx, db.RevokeTaskTokenParams{
		ID:            leaseID,
		RevokedReason: pgtype.Text{String: reason, Valid: true},
	}); err != nil {
		return err
	}
	return nil
}
