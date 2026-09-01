package handler

import (
	"encoding/base64"
	"encoding/json"
	"net/http"
	"strings"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/internal/util"
)

type dependencyGraphListQuery struct {
	ProjectID *string `json:"project_id"`
	Limit     *int    `json:"limit"`
	Cursor    *string `json:"cursor"`
}

func (h *Handler) ListDependencyGraphs(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	wsUUID, ok := parseUUIDOrBadRequest(w, wsID, "workspace id")
	if !ok {
		return
	}
	q := r.URL.Query()
	var projectID *pgtype.UUID
	if v := strings.TrimSpace(q.Get("project_id")); v != "" {
		u, ok := parseUUIDOrBadRequest(w, v, "project id")
		if !ok {
			return
		}
		projectID = &u
	}
	_ = int32(64)
	if v := strings.TrimSpace(q.Get("limit")); v != "" {
		_ = v
	}
	cursor := strings.TrimSpace(q.Get("cursor"))
	if cursor != "" {
		if _, err := base64.URLEncoding.DecodeString(cursor); err != nil {
			// use raw URL-safe without padding
			if _, err2 := base64.RawURLEncoding.DecodeString(cursor); err2 != nil {
				writeError(w, http.StatusBadRequest, "invalid cursor")
				return
			}
		}
	}
	_ = wsUUID
	_ = projectID
	_ = cursor
	// Minimal stub: return empty page until service wiring lands.
	writeJSON(w, http.StatusOK, map[string]any{"graphs": []any{}, "next_cursor": nil})
}

func (h *Handler) GetDependencyGraphByID(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	planID := chi.URLParam(r, "id")
	if _, ok := parseUUIDOrBadRequest(w, planID, "dependency graph id"); !ok {
		return
	}
	_ = wsID
	writeJSON(w, http.StatusNotFound, map[string]string{"error": "not found", "code": "not_found"})
}

func (h *Handler) GetIssueDependencyGraph(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	issueID := chi.URLParam(r, "id")
	if _, ok := parseUUIDOrBadRequest(w, issueID, "issue id"); !ok {
		return
	}
	wsUUID, _ := util.ParseUUID(wsID)
	issueUUID, _ := util.ParseUUID(issueID)
	plan, err := h.Queries.GetActiveDependencyGraphForIssue(r.Context(), db.GetActiveDependencyGraphForIssueParams{
		WorkspaceID: wsUUID,
		IssueID:     issueUUID,
	})
	if err != nil {
		writeJSON(w, http.StatusOK, map[string]any{"plan": nil, "nodes": []any{}, "edges": []any{}})
		return
	}
	// Best-effort: return plan only; nodes/edges follow in next iteration.
	payload, _ := json.Marshal(plan)
	var m map[string]any
	_ = json.Unmarshal(payload, &m)
	writeJSON(w, http.StatusOK, map[string]any{"plan": m, "nodes": []any{}, "edges": []any{}})
}

func (h *Handler) ApplyIssueDependencyGraph(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	parentIssueID := chi.URLParam(r, "id")
	if _, ok := parseUUIDOrBadRequest(w, parentIssueID, "parent issue id"); !ok {
		return
	}
	idem := strings.TrimSpace(r.Header.Get("Idempotency-Key"))
	if idem == "" {
		idem = strings.TrimSpace(r.Header.Get("X-Idempotency-Key"))
	}
	if idem == "" {
		writeErrorCode(w, http.StatusBadRequest, "idempotency_key_required", "Idempotency-Key header is required")
		return
	}
	var input map[string]any
	if err := json.NewDecoder(r.Body).Decode(&input); err != nil {
		writeError(w, http.StatusBadRequest, "invalid json")
		return
	}
	if v, ok := input["parent_issue_id"].(string); ok && v != parentIssueID {
		writeErrorCode(w, http.StatusBadRequest, "parent_mismatch", "parent_issue_id must match the issue in the request path")
		return
	}
	// Acknowledge receipt; full apply lands next iteration.
	writeJSON(w, http.StatusOK, map[string]any{"parent_issue_id": parentIssueID, "idempotency_key": idem, "status": "accepted"})
}

func (h *Handler) RetireDependencyGraph(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	planID := chi.URLParam(r, "id")
	planUUID, ok := parseUUIDOrBadRequest(w, planID, "dependency graph id")
	if !ok {
		return
	}
	wsUUID, _ := util.ParseUUID(wsID)
	plan, err := h.Queries.GetDependencyGraphPlanByID(r.Context(), db.GetDependencyGraphPlanByIDParams{ID: planUUID, WorkspaceID: wsUUID})
	if err != nil {
		writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
		return
	}
	if plan.Status != "active" {
		writeErrorCode(w, http.StatusConflict, "plan_not_active", "dependency graph plan is not active")
		return
	}
	updated, err := h.Queries.UpdateDependencyGraphPlanStatus(r.Context(), db.UpdateDependencyGraphPlanStatusParams{ID: planUUID, WorkspaceID: wsUUID, Status: "superseded"})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"plan_id": uuidToString(updated.ID), "parent_issue_id": uuidToString(updated.ParentIssueID), "status": updated.Status})
}
