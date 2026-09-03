package handler

import (
	"context"
	"errors"
	"net/http"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/entitlement"
)

const freeHostedWorkspaceAllowance int64 = 2

const (
	hostedWorkspaceLimitCode       = "hosted_workspace_limit_reached"
	hostedWorkspaceLimitMessage    = "this account has reached its hosted workspace limit"
	hostedWorkspaceQuotaCode       = "hosted_workspace_quota_unavailable"
	hostedWorkspaceQuotaMessage    = "hosted workspace quota is temporarily unavailable"
	guestWorkspaceLimitMessage     = "guest workspace limit reached; formal login required"
	guestWorkspaceQuotaMessage     = "guest workspace quota unavailable"
	formalLoginRequiredMessage     = "formal login required"
)

type hostedWorkspacePolicy struct {
	action entitlement.Action
	limit  *int64
}

func unavailableHostedWorkspacePolicy() hostedWorkspacePolicy {
	return hostedWorkspacePolicy{action: entitlement.ActionOff}
}

// resolveHostedWorkspacePolicy reads Cloud before a database transaction is
// opened. One existing owned workspace supplies the account-scoped policy;
// the guaranteed two-workspace Free allowance needs no synthetic workspace.
func (h *Handler) resolveHostedWorkspacePolicy(ctx context.Context, userID pgtype.UUID) hostedWorkspacePolicy {
	if !isOfficialCloudDeployment() {
		return hostedWorkspacePolicy{action: entitlement.ActionObserve}
	}
	if h.DB == nil || h.Entitlements == nil {
		return unavailableHostedWorkspacePolicy()
	}
	var source pgtype.UUID
	err := h.DB.QueryRow(ctx, `
SELECT workspace_id
FROM member
WHERE user_id = $1 AND role = 'owner'
ORDER BY workspace_id
LIMIT 1`, userID).Scan(&source)
	if errors.Is(err, pgx.ErrNoRows) {
		return unavailableHostedWorkspacePolicy()
	}
	if err != nil || !source.Valid {
		return unavailableHostedWorkspacePolicy()
	}
	decision := h.Entitlements.Gate(ctx, uuid.UUID(source.Bytes), entitlement.GateHostedWorkspaceLimit)
	switch decision.Gate.Action {
	case entitlement.ActionObserve:
		return hostedWorkspacePolicy{action: entitlement.ActionObserve}
	case entitlement.ActionEnforce:
		if decision.Gate.Limit == nil {
			return hostedWorkspacePolicy{action: entitlement.ActionEnforce}
		}
		limit := int64(*decision.Gate.Limit)
		if limit < 0 {
			return unavailableHostedWorkspacePolicy()
		}
		return hostedWorkspacePolicy{action: entitlement.ActionEnforce, limit: &limit}
	default:
		return unavailableHostedWorkspacePolicy()
	}
}

func admitHostedWorkspaceOwnership(ownedCount int64, policy hostedWorkspacePolicy) (status int, code, message string) {
	if ownedCount < freeHostedWorkspaceAllowance {
		return 0, "", ""
	}
	switch policy.action {
	case entitlement.ActionObserve:
		return 0, "", ""
	case entitlement.ActionEnforce:
		if policy.limit == nil || ownedCount < *policy.limit {
			return 0, "", ""
		}
		return http.StatusForbidden, hostedWorkspaceLimitCode, hostedWorkspaceLimitMessage
	default:
		return http.StatusServiceUnavailable, hostedWorkspaceQuotaCode, hostedWorkspaceQuotaMessage
	}
}
