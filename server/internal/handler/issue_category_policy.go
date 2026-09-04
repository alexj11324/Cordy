package handler

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"strings"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/issuestatus"
	"github.com/patchbay-ai/patchbay/server/internal/logger"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

const issueCategoryPolicyUnavailable = "configured policy agent is unavailable"

// IssueCategoryPolicyResponse is the JSON contract for one workspace-wide
// execution/review default. Agent relationships are intentionally nullable:
// the database has no foreign keys and the write path validates them before
// storing a non-null value.
type IssueCategoryPolicyResponse struct {
	WorkspaceID             string  `json:"workspace_id"`
	Category                string  `json:"category"`
	DefaultExecutionAgentID *string `json:"default_execution_agent_id"`
	DefaultReviewerAgentID  *string `json:"default_reviewer_agent_id"`
	CreatedAt               string  `json:"created_at"`
	UpdatedAt               string  `json:"updated_at"`
}

type ListIssueCategoryPoliciesResponse struct {
	Policies []IssueCategoryPolicyResponse `json:"policies"`
}

type updateIssueCategoryPolicyRequest struct {
	DefaultExecutionAgentID *string `json:"default_execution_agent_id"`
	DefaultReviewerAgentID  *string `json:"default_reviewer_agent_id"`
}

func issueCategoryPolicyToResponse(policy db.WorkspaceIssueCategoryPolicy) IssueCategoryPolicyResponse {
	return IssueCategoryPolicyResponse{
		WorkspaceID:             uuidToString(policy.WorkspaceID),
		Category:                policy.Category,
		DefaultExecutionAgentID: uuidToPtr(policy.DefaultExecutionAgentID),
		DefaultReviewerAgentID:  uuidToPtr(policy.DefaultReviewerAgentID),
		CreatedAt:               timestampToString(policy.CreatedAt),
		UpdatedAt:               timestampToString(policy.UpdatedAt),
	}
}

// ListIssueCategoryPolicies returns the two workspace policy rows, scoped by
// the same membership guard used by the Rust route. The handler repeats the
// membership lookup so direct handler calls and the routed path have the same
// authorization contract.
func (h *Handler) ListIssueCategoryPolicies(w http.ResponseWriter, r *http.Request) {
	workspaceID := h.resolveWorkspaceID(r)
	wsUUID, ok := parseUUIDOrBadRequest(w, workspaceID, "workspace id")
	if !ok {
		return
	}
	if _, ok := h.requireWorkspaceMember(w, r, workspaceID, "workspace not found"); !ok {
		return
	}

	policies, err := h.Queries.ListWorkspaceIssueCategoryPolicies(r.Context(), wsUUID)
	if err != nil {
		slog.Warn("ListIssueCategoryPolicies failed", append(logger.RequestAttrs(r), "error", err)...)
		writeError(w, http.StatusInternalServerError, "failed to list issue category policies")
		return
	}

	response := ListIssueCategoryPoliciesResponse{
		Policies: make([]IssueCategoryPolicyResponse, len(policies)),
	}
	for i, policy := range policies {
		response.Policies[i] = issueCategoryPolicyToResponse(policy)
	}
	writeJSON(w, http.StatusOK, response)
}

func parseIssueCategoryPolicyAgentID(w http.ResponseWriter, raw *string, field string) (pgtype.UUID, bool) {
	if raw == nil || strings.TrimSpace(*raw) == "" {
		return pgtype.UUID{}, true
	}
	return parseUUIDOrBadRequest(w, strings.TrimSpace(*raw), field)
}

func validateIssueCategoryPolicyAgent(ctx context.Context, queries *db.Queries, agentID, workspaceID pgtype.UUID) error {
	agent, err := queries.GetAgentInWorkspace(ctx, db.GetAgentInWorkspaceParams{
		ID:          agentID,
		WorkspaceID: workspaceID,
	})
	if errors.Is(err, pgx.ErrNoRows) {
		return errors.New(issueCategoryPolicyUnavailable)
	}
	if err != nil {
		return err
	}
	if agent.ArchivedAt.Valid || !agent.RuntimeID.Valid {
		return errors.New(issueCategoryPolicyUnavailable)
	}
	return nil
}

// UpdateIssueCategoryPolicy validates and atomically stores one workspace
// policy. It mirrors the Rust service boundary: both agent lookups and the
// upsert share one transaction, and the realtime event is published only after
// commit succeeds.
func (h *Handler) UpdateIssueCategoryPolicy(w http.ResponseWriter, r *http.Request) {
	var request *updateIssueCategoryPolicyRequest
	decoder := json.NewDecoder(r.Body)
	if err := decoder.Decode(&request); err != nil || request == nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if err := rejectTrailingJSON(decoder); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}

	workspaceID := h.resolveWorkspaceID(r)
	wsUUID, ok := parseUUIDOrBadRequest(w, workspaceID, "workspace id")
	if !ok {
		return
	}
	member, ok := h.requireWorkspaceRole(w, r, workspaceID, "workspace not found", "owner", "admin")
	if !ok {
		return
	}

	category := chi.URLParam(r, "category")
	if category != issuestatus.InProgress && category != issuestatus.InReview {
		writeError(w, http.StatusBadRequest, "unsupported issue category policy")
		return
	}
	executionID, ok := parseIssueCategoryPolicyAgentID(w, request.DefaultExecutionAgentID, "default_execution_agent_id")
	if !ok {
		return
	}
	reviewerID, ok := parseIssueCategoryPolicyAgentID(w, request.DefaultReviewerAgentID, "default_reviewer_agent_id")
	if !ok {
		return
	}
	if !executionID.Valid {
		writeError(w, http.StatusBadRequest, "default_execution_agent_id is required")
		return
	}
	if category == issuestatus.InReview && !reviewerID.Valid {
		writeError(w, http.StatusBadRequest, "default_reviewer_agent_id is required for in_review")
		return
	}

	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		slog.Warn("UpdateIssueCategoryPolicy begin failed", append(logger.RequestAttrs(r), "error", err)...)
		writeError(w, http.StatusInternalServerError, "failed to update issue category policy")
		return
	}
	defer tx.Rollback(r.Context())
	qtx := h.Queries.WithTx(tx)

	if reviewerID.Valid && uuidToString(executionID) == uuidToString(reviewerID) {
		writeError(w, http.StatusBadRequest, "execution and review agents must differ")
		return
	}
	for _, agentID := range []pgtype.UUID{executionID, reviewerID} {
		if !agentID.Valid {
			continue
		}
		if err := validateIssueCategoryPolicyAgent(r.Context(), qtx, agentID, wsUUID); err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
	}

	policy, err := qtx.UpsertWorkspaceIssueCategoryPolicy(r.Context(), db.UpsertWorkspaceIssueCategoryPolicyParams{
		WorkspaceID:             wsUUID,
		Category:                category,
		DefaultExecutionAgentID: executionID,
		DefaultReviewerAgentID:  reviewerID,
	})
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := tx.Commit(r.Context()); err != nil {
		slog.Warn("UpdateIssueCategoryPolicy commit failed", append(logger.RequestAttrs(r), "error", err)...)
		writeError(w, http.StatusInternalServerError, "failed to update issue category policy")
		return
	}

	h.publish(protocol.EventIssueCategoryPolicyChanged, uuidToString(wsUUID), "member", uuidToString(member.UserID), map[string]any{
		"category": category,
	})
	writeJSON(w, http.StatusOK, issueCategoryPolicyToResponse(policy))
}

func rejectTrailingJSON(decoder *json.Decoder) error {
	var trailing any
	err := decoder.Decode(&trailing)
	if errors.Is(err, io.EOF) {
		return nil
	}
	if err == nil {
		return errors.New("multiple JSON values")
	}
	return err
}
