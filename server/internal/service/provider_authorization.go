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
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
)

const ProviderAuthorizationPolicyVersion = "phase1-go-2026-09-02"

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
	return effect == "allow" || effect == "deny" || effect == "require_approval"
}

func (s *ProviderAuthorizationService) CreateGrant(ctx context.Context, workspaceID, actorID pgtype.UUID, input ProviderGrantInput) (db.AuthorizationGrant, error) {
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
	if len(input.AllowedActions) != 1 || input.AllowedActions[0] != "provider.invoke" {
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
		_, err = s.Queries.GetTeamByAssignee(ctx, db.GetTeamByAssigneeParams{ID: input.GranteeID, WorkspaceID: workspaceID})
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
		if taskErr != nil || task.RuntimeID != input.RuntimeID {
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

func grantPrincipalMatches(grant db.AuthorizationGrant, originator, agentID pgtype.UUID, teamIDs map[pgtype.UUID]struct{}) bool {
	switch grant.PrincipalType {
	case "user":
		return !grant.PrincipalID.Valid || grant.PrincipalID == originator
	case "team":
		_, ok := teamIDs[grant.PrincipalID]
		return grant.PrincipalID.Valid && ok
	case "agent_definition":
		return !grant.PrincipalID.Valid || grant.PrincipalID == agentID
	case "task_run":
		return grant.Effect != "allow"
	default:
		return false
	}
}

func grantConditionsMatch(grant db.AuthorizationGrant, input ProviderLeaseValidation, delegated bool) (providerGrantConditions, bool) {
	var conditions providerGrantConditions
	if err := json.Unmarshal(grant.Conditions, &conditions); err != nil {
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
	if delegated && grant.Effect == "allow" && conditions.TaskID != uuidText(input.TaskID) {
		return conditions, false
	}
	return conditions, true
}

func (s *ProviderAuthorizationService) recordDecision(ctx context.Context, input ProviderLeaseValidation, effect, reason string, grantIDs []pgtype.UUID, reservation int64) (ProviderAuthorizationDecision, error) {
	decisionID := dbid.NewV7()
	// matched_grant_ids is NOT NULL: a decision that matched nothing records an
	// empty array, which is a different and load-bearing statement from "we do
	// not know what this matched".
	if grantIDs == nil {
		grantIDs = []pgtype.UUID{}
	}
	contextPayload := map[string]any{
		"task_id": uuidText(input.TaskID), "lease_id": uuidText(input.LeaseID),
		"provider_request_tokens":     reservation,
		"provider_budget_reservation": effect == "allow" && reservation > 0,
	}
	rawContext, _ := json.Marshal(contextPayload)
	event, err := s.Queries.CreateAuthorizationAuditEvent(ctx, db.CreateAuthorizationAuditEventParams{
		ID: decisionID, WorkspaceID: input.WorkspaceID, PrincipalType: "task_run", PrincipalID: input.TaskID,
		OnBehalfOfUserID: input.OnBehalfOfUserID, ViaAgentID: input.AgentID, DeviceID: input.RuntimeID,
		Action: "credential.use", ResourceType: "provider_identity", ResourceID: input.RuntimeID,
		Decision: effect, Reason: reason, MatchedGrantIds: grantIDs,
		PolicyVersion: ProviderAuthorizationPolicyVersion, Obligations: []byte(`[]`), DelegationChain: []byte(`[]`), Context: rawContext,
	})
	if err != nil {
		return ProviderAuthorizationDecision{}, err
	}
	return ProviderAuthorizationDecision{Allowed: effect == "allow", Effect: effect, Reason: reason, DecisionID: event.ID, PolicyVersion: event.PolicyVersion, MatchedGrantIDs: grantIDs}, nil
}

func (s *ProviderAuthorizationService) ValidateLease(ctx context.Context, input ProviderLeaseValidation) (ProviderAuthorizationDecision, error) {
	lease, err := s.Queries.GetValidTaskCapabilityLease(ctx, input.LeaseID)
	if err != nil && !errors.Is(err, pgx.ErrNoRows) {
		return ProviderAuthorizationDecision{}, err
	}
	if err != nil || !LeaseAuthorizesProviderUse(lease, input.TaskID, input.AgentID, input.WorkspaceID, input.RuntimeID, input.OnBehalfOfUserID) {
		return s.recordDecision(ctx, input, "deny", "capability lease is invalid, expired, revoked, or outside provider scope", nil, 0)
	}
	task, err := s.Queries.GetAgentTask(ctx, input.TaskID)
	if err != nil || task.AgentID != input.AgentID || task.RuntimeID != input.RuntimeID || task.OriginatorUserID != input.OnBehalfOfUserID {
		return s.recordDecision(ctx, input, "deny", "provider lease task identity mismatch", nil, 0)
	}
	agent, err := s.Queries.GetAgentInWorkspace(ctx, db.GetAgentInWorkspaceParams{ID: input.AgentID, WorkspaceID: input.WorkspaceID})
	if err != nil {
		return s.recordDecision(ctx, input, "deny", "provider lease workspace or agent mismatch", nil, 0)
	}
	runtime, err := s.Queries.GetAgentRuntimeForWorkspace(ctx, db.GetAgentRuntimeForWorkspaceParams{ID: input.RuntimeID, WorkspaceID: input.WorkspaceID})
	if err != nil || runtime.Provider != input.Provider || agent.RuntimeID != input.RuntimeID || !runtime.OwnerID.Valid {
		return s.recordDecision(ctx, input, "deny", "provider lease runtime or provider mismatch", nil, 0)
	}
	member, err := s.Queries.GetMemberByUserAndWorkspace(ctx, db.GetMemberByUserAndWorkspaceParams{UserID: input.OnBehalfOfUserID, WorkspaceID: input.WorkspaceID})
	if err != nil || !member.UserID.Valid {
		return s.recordDecision(ctx, input, "deny", "provider lease actor is not a current workspace member", nil, 0)
	}
	teams, err := s.Queries.ListTeamsByMember(ctx, db.ListTeamsByMemberParams{WorkspaceID: input.WorkspaceID, MemberType: "member", MemberID: input.OnBehalfOfUserID})
	if err != nil {
		return ProviderAuthorizationDecision{}, err
	}
	teamIDs := make(map[pgtype.UUID]struct{}, len(teams))
	for _, team := range teams {
		teamIDs[team.ID] = struct{}{}
	}
	grants, err := s.Queries.ListActiveProviderAuthorizationGrants(ctx, db.ListActiveProviderAuthorizationGrantsParams{WorkspaceID: input.WorkspaceID, RuntimeID: input.RuntimeID})
	if err != nil {
		return ProviderAuthorizationDecision{}, err
	}
	delegated := task.DelegatedFromTaskID.Valid
	matchedIDs := make([]pgtype.UUID, 0)
	matchedEffects := make([]string, 0)
	var budget int64
	var bounded bool
	for _, grant := range grants {
		if grant.CreatedBy != runtime.OwnerID || !grantPrincipalMatches(grant, input.OnBehalfOfUserID, input.AgentID, teamIDs) {
			continue
		}
		conditions, matches := grantConditionsMatch(grant, input, delegated)
		if !matches {
			continue
		}
		matchedIDs = append(matchedIDs, grant.ID)
		matchedEffects = append(matchedEffects, grant.Effect)
		if grant.Effect == "allow" && conditions.MaxTokens != nil {
			bounded = true
			if *conditions.MaxTokens > budget {
				budget = *conditions.MaxTokens
			}
		}
	}
	if slices.Contains(matchedEffects, "deny") {
		return s.recordDecision(ctx, input, "deny", "matched explicit deny grant", matchedIDs, 0)
	}
	if slices.Contains(matchedEffects, "require_approval") {
		return s.recordDecision(ctx, input, "require_approval", "approval is required before provider use", matchedIDs, 0)
	}
	ownerUse := runtime.OwnerID == input.OnBehalfOfUserID
	if !ownerUse && !slices.Contains(matchedEffects, "allow") {
		return s.recordDecision(ctx, input, "deny", "provider owner has not authorized this use", matchedIDs, 0)
	}
	reservation := input.RequestedMaxTokens
	if !ownerUse && bounded && reservation == 0 {
		// A caller that names no ceiling is charged the whole grant: the
		// alternative is reserving zero, which lets an unbounded request spend
		// a bounded grant forever without the budget ever moving.
		reservation = budget
	}
	// The budget is checked before the allow is written, not after. Recording
	// the allow first and then denying leaves an allow event in the ledger for
	// an operation that never ran, and — because the sum reads the ledger — that
	// phantom row is then charged against every later request.
	if !ownerUse && bounded {
		reserved, sumErr := s.Queries.SumProviderAuthorizationReservations(ctx, db.SumProviderAuthorizationReservationsParams{WorkspaceID: input.WorkspaceID, RuntimeID: input.RuntimeID, GrantIds: matchedIDs})
		if sumErr != nil {
			return ProviderAuthorizationDecision{}, sumErr
		}
		if reserved+reservation > budget {
			return s.recordDecision(ctx, input, "deny", "provider token budget exhausted", matchedIDs, 0)
		}
	}
	return s.recordDecision(ctx, input, "allow", "active capability lease and provider grant allow action", matchedIDs, reservation)
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
