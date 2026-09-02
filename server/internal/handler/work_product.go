package handler

import (
	"encoding/json"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/internal/util"
)

func (h *Handler) ListWorkProducts(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	wsUUID, _ := util.ParseUUID(wsID)
	products, err := h.Queries.ListWorkProductsByWorkspace(r.Context(), db.ListWorkProductsByWorkspaceParams{WorkspaceID: wsUUID, Limit: 64, Offset: 0})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"products": products})
}

func (h *Handler) GetWorkProduct(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	id := chi.URLParam(r, "id")
	pid, ok := parseUUIDOrBadRequest(w, id, "work product id")
	if !ok {
		return
	}
	wsUUID, _ := util.ParseUUID(wsID)
	p, err := h.Queries.GetWorkProductByID(r.Context(), db.GetWorkProductByIDParams{ID: pid, WorkspaceID: wsUUID})
	if err != nil {
		writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
		return
	}
	writeJSON(w, http.StatusOK, p)
}

func (h *Handler) CreateWorkProduct(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	var body map[string]any
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeError(w, http.StatusBadRequest, "invalid json")
		return
	}
	kind, _ := body["kind"].(string)
	provider, _ := body["provider"].(string)
	extID, _ := body["external_identity"].(string)
	if kind == "" || provider == "" || extID == "" {
		writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_work_product", "kind/provider/external_identity required")
		return
	}
	wsUUID, _ := util.ParseUUID(wsID)
	p, err := h.Queries.CreateWorkProduct(r.Context(), db.CreateWorkProductParams{
		WorkspaceID:      wsUUID,
		Kind:             kind,
		Provider:         provider,
		ExternalIdentity: extID,
	})
	if err != nil {
		if isCheckViolation(err) || isUniqueViolation(err) {
			writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_work_product", err.Error())
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusCreated, p)
}

func (h *Handler) ListWorkProductRelationsByIssue(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	issueID := chi.URLParam(r, "id")
	iid, ok := parseUUIDOrBadRequest(w, issueID, "issue id")
	if !ok {
		return
	}
	wsUUID, _ := util.ParseUUID(wsID)
	rels, err := h.Queries.ListWorkProductRelationsByIssue(r.Context(), db.ListWorkProductRelationsByIssueParams{WorkspaceID: wsUUID, IssueID: iid})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"relations": rels})
}

func (h *Handler) CreateWorkProductRelation(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	var body map[string]any
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeError(w, http.StatusBadRequest, "invalid json")
		return
	}
	wpIDStr, _ := body["work_product_id"].(string)
	wpUUID, ok := parseUUIDOrBadRequest(w, wpIDStr, "work_product_id")
	if !ok {
		return
	}
	relKey, _ := body["relation_key"].(string)
	relSource, _ := body["relation_source"].(string)
	if relKey == "" || relSource == "" {
		writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_relation", "relation_key/relation_source required")
		return
	}
	attachedByType, _ := body["attached_by_type"].(string)
	attachedByIDStr, _ := body["attached_by_id"].(string)
	attachedByID, ok := parseUUIDOrBadRequest(w, attachedByIDStr, "attached_by_id")
	if !ok {
		return
	}
	wsUUID, _ := util.ParseUUID(wsID)
	arg := db.CreateWorkProductRelationParams{
		WorkspaceID:    wsUUID,
		WorkProductID:  wpUUID,
		RelationKey:    relKey,
		RelationSource: relSource,
		AttachedByType: attachedByType,
		AttachedByID:   attachedByID,
	}
	if v, ok := body["issue_id"].(string); ok && v != "" {
		u, _ := util.ParseUUID(v)
		arg.IssueID = u
	}
	if v, ok := body["task_id"].(string); ok && v != "" {
		u, _ := util.ParseUUID(v)
		arg.TaskID = u
	}
	if v, ok := body["run_id"].(string); ok && v != "" {
		u, _ := util.ParseUUID(v)
		arg.RunID = u
	}
	rel, err := h.Queries.CreateWorkProductRelation(r.Context(), arg)
	if err != nil {
		if isCheckViolation(err) || isUniqueViolation(err) {
			writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_relation", err.Error())
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusCreated, rel)
}

func (h *Handler) GetProvenanceByTask(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	taskID := chi.URLParam(r, "taskId")
	tid, ok := parseUUIDOrBadRequest(w, taskID, "task id")
	if !ok {
		return
	}
	wsUUID, _ := util.ParseUUID(wsID)
	p, err := h.Queries.GetProvenanceByTask(r.Context(), db.GetProvenanceByTaskParams{WorkspaceID: wsUUID, TaskID: tid})
	if err != nil {
		writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
		return
	}
	writeJSON(w, http.StatusOK, p)
}

func (h *Handler) UpsertProvenance(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	taskID := chi.URLParam(r, "taskId")
	tid, ok := parseUUIDOrBadRequest(w, taskID, "task id")
	if !ok {
		return
	}
	var body map[string]any
	_ = json.NewDecoder(r.Body).Decode(&body)
	wsUUID, _ := util.ParseUUID(wsID)
	arg := db.UpsertProvenanceParams{WorkspaceID: wsUUID, TaskID: tid}
	if v, ok := body["repo_identity"].(string); ok {
		arg.RepoIdentity = pgtype.Text{String: v, Valid: true}
	}
	if v, ok := body["execution_workspace"].(string); ok {
		arg.ExecutionWorkspace = pgtype.Text{String: v, Valid: true}
	}
	if v, ok := body["head_branch"].(string); ok {
		arg.HeadBranch = pgtype.Text{String: v, Valid: true}
	}
	if v, ok := body["head_sha"].(string); ok {
		arg.HeadSha = pgtype.Text{String: v, Valid: true}
	}
	if v, ok := body["head_state"].(string); ok {
		arg.HeadState = pgtype.Text{String: v, Valid: true}
	}
	if v, ok := body["discovery_status"].(string); ok {
		arg.DiscoveryStatus = pgtype.Text{String: v, Valid: true}
	}
	p, err := h.Queries.UpsertProvenance(r.Context(), arg)
	if err != nil {
		if isCheckViolation(err) {
			writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_provenance", err.Error())
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusOK, p)
}

func (h *Handler) ListProvenanceByWorkspace(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	wsUUID, _ := util.ParseUUID(wsID)
	ps, err := h.Queries.ListProvenanceByWorkspace(r.Context(), db.ListProvenanceByWorkspaceParams{WorkspaceID: wsUUID, Limit: 64, Offset: 0})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"provenance": ps})
}
