package handler

import (
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"path/filepath"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

const (
	workProductDefaultPageSize = 64
	workProductMaxPageSize     = 100
	workProductMaxPage         = 100000
)

const (
	workProductRelationSourceManual    = "manual_explicit"
	workProductRelationSourceTask      = "task_explicit"
	workProductRelationSourceDiscovery = "execution_branch_discovery"
)

func isExplicitWorkProductRelationSource(source string) bool {
	switch source {
	case workProductRelationSourceManual, workProductRelationSourceTask, workProductRelationSourceDiscovery:
		return true
	default:
		return false
	}
}

const (
	workProductColumns           = `id, workspace_id, kind, provider, external_identity, external_url, provider_record_type, provider_record_id, created_at, updated_at`
	workProductRelationColumns   = `id, workspace_id, work_product_id, issue_id, task_id, run_id, relation_key, relation_source, attached_by_type, attached_by_id, attached_at, close_intent, detached_at, detached_by_type, detached_by_id, detached_task_id, detached_run_id`
	workProductProvenanceColumns = `task_id, workspace_id, run_id, repo_identity, execution_workspace, head_branch, head_sha, head_state, started_at, finished_at, discovery_status, discovery_lease_id, discovery_match_count, discovery_reason, discovery_work_product_id, discovery_at, updated_at`
)

type workProductCreateRequest struct {
	Kind               string `json:"kind"`
	Provider           string `json:"provider"`
	ExternalIdentity   string `json:"external_identity"`
	ExternalURL        string `json:"external_url"`
	ProviderRecordType string `json:"provider_record_type"`
	ProviderRecordID   string `json:"provider_record_id"`
}

// WorkProductRelationRequest retains the old actor/anchor fields for clients
// that still send them, but the handler treats them as assertions. Actor and
// task/run identity is always derived from authenticated request context and
// the issue path; clients cannot use this body to impersonate another actor or
// cross a workspace boundary.
type workProductRelationRequest struct {
	WorkProductID  string `json:"work_product_id"`
	IssueID        string `json:"issue_id"`
	TaskID         string `json:"task_id"`
	RunID          string `json:"run_id"`
	RelationKey    string `json:"relation_key"`
	RelationSource string `json:"relation_source"`
	AttachedByType string `json:"attached_by_type"`
	AttachedByID   string `json:"attached_by_id"`
	CloseIntent    bool   `json:"close_intent"`
}

type workProductProvenanceRequest struct {
	RunID              string `json:"run_id"`
	RepoIdentity       string `json:"repo_identity"`
	ExecutionWorkspace string `json:"execution_workspace"`
	HeadBranch         string `json:"head_branch"`
	HeadSHA            string `json:"head_sha"`
	HeadState          string `json:"head_state"`
	DiscoveryStatus    string `json:"discovery_status"`
	DiscoveryReason    string `json:"discovery_reason"`
}

type workProductRelationActor struct {
	Type   string
	ID     pgtype.UUID
	TaskID pgtype.UUID
	RunID  pgtype.UUID
}

type workProductProvenanceValues struct {
	RepoIdentity       string
	ExecutionWorkspace string
	HeadBranch         *string
	HeadSHA            *string
	HeadState          string
}

type branchDiscoveryDecision struct {
	Status string
	Reason string
}

func decodeWorkProductJSON(r *http.Request, dst any) error {
	decoder := json.NewDecoder(r.Body)
	if err := decoder.Decode(dst); err != nil {
		return err
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		if err == nil {
			return errors.New("multiple JSON values")
		}
		return err
	}
	return nil
}

func workProductPage(r *http.Request) (int32, int32, error) {
	page := 1
	perPage := workProductDefaultPageSize
	if raw := strings.TrimSpace(r.URL.Query().Get("page")); raw != "" {
		parsed, err := strconv.Atoi(raw)
		if err != nil || parsed < 1 || parsed > workProductMaxPage {
			return 0, 0, errors.New("invalid page")
		}
		page = parsed
	}
	if raw := strings.TrimSpace(r.URL.Query().Get("per_page")); raw != "" {
		parsed, err := strconv.Atoi(raw)
		if err != nil || parsed < 1 || parsed > workProductMaxPageSize {
			return 0, 0, errors.New("invalid per_page")
		}
		perPage = parsed
	}
	offset := (page - 1) * perPage
	return int32(perPage), int32(offset), nil
}

func validWorkProductKind(kind string) bool {
	switch kind {
	case "pull_request", "branch", "commit", "preview", "artifact", "document":
		return true
	default:
		return false
	}
}

func validWorkProductProvider(provider string) bool {
	provider = strings.TrimSpace(provider)
	if provider == "" || len(provider) > 64 {
		return false
	}
	for _, b := range []byte(provider) {
		if b < 33 || b > 126 {
			return false
		}
	}
	return true
}

func validWorkProductExternalIdentity(identity string) bool {
	identity = strings.TrimSpace(identity)
	if identity == "" || utf8.RuneCountInString(identity) > 2048 {
		return false
	}
	for _, r := range identity {
		if r < 0x20 || r == 0x7f {
			return false
		}
	}
	return true
}

func workProductRelationKey(issueID, taskID, runID pgtype.UUID) string {
	issue := util.UUIDToString(issueID)
	if issue == "" {
		issue = "none"
	}
	task := util.UUIDToString(taskID)
	if task == "" {
		task = "manual"
	}
	run := util.UUIDToString(runID)
	if run == "" {
		run = "none"
	}
	return "issue:" + issue + ":task:" + task + ":run:" + run
}

func sameWorkProductUUID(left, right pgtype.UUID) bool {
	return left.Valid && right.Valid && util.UUIDToString(left) == util.UUIDToString(right)
}

func nullableWorkProductUUID(value pgtype.UUID) any {
	if !value.Valid {
		return nil
	}
	return value
}

func nullableWorkProductText(value string) any {
	if value == "" {
		return nil
	}
	return value
}

func textWorkProductValue(value pgtype.Text) string {
	if !value.Valid {
		return ""
	}
	return value.String
}

type workProductScanner interface {
	Scan(dest ...any) error
}

func scanWorkProduct(row workProductScanner) (db.WorkProduct, error) {
	var product db.WorkProduct
	err := row.Scan(
		&product.ID,
		&product.WorkspaceID,
		&product.Kind,
		&product.Provider,
		&product.ExternalIdentity,
		&product.ExternalUrl,
		&product.ProviderRecordType,
		&product.ProviderRecordID,
		&product.CreatedAt,
		&product.UpdatedAt,
	)
	return product, err
}

func scanWorkProductRelation(row workProductScanner) (db.WorkProductRelation, error) {
	var relation db.WorkProductRelation
	err := row.Scan(
		&relation.ID,
		&relation.WorkspaceID,
		&relation.WorkProductID,
		&relation.IssueID,
		&relation.TaskID,
		&relation.RunID,
		&relation.RelationKey,
		&relation.RelationSource,
		&relation.AttachedByType,
		&relation.AttachedByID,
		&relation.AttachedAt,
		&relation.CloseIntent,
		&relation.DetachedAt,
		&relation.DetachedByType,
		&relation.DetachedByID,
		&relation.DetachedTaskID,
		&relation.DetachedRunID,
	)
	return relation, err
}

func scanWorkProductProvenance(row workProductScanner) (db.AgentTaskExecutionProvenance, error) {
	var provenance db.AgentTaskExecutionProvenance
	err := row.Scan(
		&provenance.TaskID,
		&provenance.WorkspaceID,
		&provenance.RunID,
		&provenance.RepoIdentity,
		&provenance.ExecutionWorkspace,
		&provenance.HeadBranch,
		&provenance.HeadSha,
		&provenance.HeadState,
		&provenance.StartedAt,
		&provenance.FinishedAt,
		&provenance.DiscoveryStatus,
		&provenance.DiscoveryLeaseID,
		&provenance.DiscoveryMatchCount,
		&provenance.DiscoveryReason,
		&provenance.DiscoveryWorkProductID,
		&provenance.DiscoveryAt,
		&provenance.UpdatedAt,
	)
	return provenance, err
}

func (h *Handler) requireWorkProductDB(w http.ResponseWriter) (dbExecutor, bool) {
	if h.DB == nil {
		writeError(w, http.StatusInternalServerError, "database unavailable")
		return nil, false
	}
	return h.DB, true
}

func (h *Handler) workProductWorkspace(w http.ResponseWriter, r *http.Request) (string, pgtype.UUID, bool) {
	workspaceID := strings.TrimSpace(ctxWorkspaceID(r.Context()))
	if workspaceID == "" {
		workspaceID = strings.TrimSpace(h.resolveWorkspaceID(r))
	}
	if workspaceID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return "", pgtype.UUID{}, false
	}
	workspaceUUID, err := util.ParseUUID(workspaceID)
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid workspace_id")
		return "", pgtype.UUID{}, false
	}
	// The router already runs RequireWorkspaceMember. This fallback keeps a
	// direct handler call (and a future route rearrangement) tenant-safe too.
	if _, inContext := ctxMember(r.Context()); !inContext {
		if h.Queries == nil {
			writeError(w, http.StatusInternalServerError, "database unavailable")
			return "", pgtype.UUID{}, false
		}
		if _, ok := h.workspaceMember(w, r, workspaceID); !ok {
			return "", pgtype.UUID{}, false
		}
	}
	return workspaceID, workspaceUUID, true
}

func (h *Handler) resolveWorkProductRelationActor(w http.ResponseWriter, r *http.Request, workspaceID string, workspaceUUID, issueUUID pgtype.UUID) (workProductRelationActor, bool) {
	// X-Actor-Source is stamped by Auth after stripping client input. A
	// client-supplied X-Agent-ID/X-Task-ID pair is therefore never enough to
	// impersonate an agent on this ownership-sensitive endpoint.
	actorSource := r.Header.Get("X-Actor-Source")
	if actorSource == "" {
		actorUUID, err := util.ParseUUID(strings.TrimSpace(requestUserID(r)))
		if err != nil {
			writeError(w, http.StatusUnauthorized, "user not authenticated")
			return workProductRelationActor{}, false
		}
		return workProductRelationActor{Type: "member", ID: actorUUID}, true
	}
	if actorSource != "task_token" {
		writeError(w, http.StatusForbidden, "a machine credential cannot attach work products as a member")
		return workProductRelationActor{}, false
	}

	actorUUID, err := util.ParseUUID(strings.TrimSpace(r.Header.Get("X-Agent-ID")))
	if err != nil {
		writeError(w, http.StatusForbidden, "task execution context is invalid")
		return workProductRelationActor{}, false
	}
	if h.Queries == nil {
		writeError(w, http.StatusInternalServerError, "database unavailable")
		return workProductRelationActor{}, false
	}

	taskID, ok := parseUUIDOrBadRequest(w, strings.TrimSpace(r.Header.Get("X-Task-ID")), "task id")
	if !ok {
		return workProductRelationActor{}, false
	}
	task, err := h.Queries.GetAgentTaskInWorkspace(r.Context(), db.GetAgentTaskInWorkspaceParams{
		ID:          taskID,
		WorkspaceID: workspaceUUID,
	})
	if err != nil {
		writeError(w, http.StatusForbidden, "task execution context is invalid")
		return workProductRelationActor{}, false
	}
	if !sameWorkProductUUID(task.AgentID, actorUUID) || !sameWorkProductUUID(task.IssueID, issueUUID) {
		writeError(w, http.StatusForbidden, "task may only attach products to its issue")
		return workProductRelationActor{}, false
	}
	actor := workProductRelationActor{Type: "agent", ID: actorUUID}
	actor.TaskID = taskID
	actor.RunID = task.AutomationRunID
	return actor, true
}

func (h *Handler) ListWorkProducts(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	limit, offset, err := workProductPage(r)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	executor, ok := h.requireWorkProductDB(w)
	if !ok {
		return
	}
	rows, err := executor.Query(r.Context(), `SELECT `+workProductColumns+` FROM work_product WHERE workspace_id = $1 ORDER BY updated_at DESC, id DESC LIMIT $2 OFFSET $3`, workspaceUUID, limit+1, offset)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	defer rows.Close()
	products := make([]db.WorkProduct, 0, int(limit))
	for rows.Next() {
		product, scanErr := scanWorkProduct(rows)
		if scanErr != nil {
			writeError(w, http.StatusInternalServerError, "database error")
			return
		}
		products = append(products, product)
	}
	if err := rows.Err(); err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	hasMore := len(products) > int(limit)
	if hasMore {
		products = products[:limit]
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"products": products,
		"page":     int(offset/limit) + 1,
		"per_page": limit,
		"has_more": hasMore,
	})
}

// ListUnassociatedWorkProducts mirrors provider objects that are not attached
// to an issue by one of the three canonical relation sources. Provider text is
// deliberately not inspected here: a caller must select a product and invoke
// an explicit attach endpoint.
func (h *Handler) ListUnassociatedWorkProducts(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok || h.Queries == nil {
		if ok {
			writeError(w, http.StatusInternalServerError, "database unavailable")
		}
		return
	}
	page := 1
	perPage := 20
	if raw := strings.TrimSpace(r.URL.Query().Get("page")); raw != "" {
		parsed, err := strconv.Atoi(raw)
		if err != nil || parsed < 1 || parsed > workProductMaxPage {
			writeError(w, http.StatusBadRequest, "invalid page")
			return
		}
		page = parsed
	}
	if raw := strings.TrimSpace(r.URL.Query().Get("per_page")); raw != "" {
		parsed, err := strconv.Atoi(raw)
		if err != nil || parsed < 1 || parsed > 50 {
			writeError(w, http.StatusBadRequest, "invalid per_page")
			return
		}
		perPage = parsed
	}
	search := strings.TrimSpace(r.URL.Query().Get("query"))
	if utf8.RuneCountInString(search) > 200 {
		writeError(w, http.StatusBadRequest, "query is too long")
		return
	}
	products, err := h.Queries.ListUnassociatedWorkProducts(r.Context(), db.ListUnassociatedWorkProductsParams{
		WorkspaceID: workspaceUUID,
		Kind:        "pull_request",
		Search:      search,
		Limit:       int32(perPage + 1),
		Offset:      int32((page - 1) * perPage),
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to list unassociated work products")
		return
	}
	nextPage := any(nil)
	if len(products) > perPage {
		nextPage = page + 1
		products = products[:perPage]
	}
	responses := make([]map[string]any, 0, len(products))
	for _, product := range products {
		responses = append(responses, workProductCatalogResponse(product, nil))
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"work_products": responses,
		"next_page":     nextPage,
	})
}

func (h *Handler) GetWorkProduct(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	pid, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "work product id")
	if !ok {
		return
	}
	executor, ok := h.requireWorkProductDB(w)
	if !ok {
		return
	}
	p, err := scanWorkProduct(executor.QueryRow(r.Context(), `SELECT `+workProductColumns+` FROM work_product WHERE id = $1 AND workspace_id = $2`, pid, workspaceUUID))
	if err != nil {
		if isNotFound(err) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusOK, p)
}

func (h *Handler) CreateWorkProduct(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	var request workProductCreateRequest
	if err := decodeWorkProductJSON(r, &request); err != nil {
		writeError(w, http.StatusBadRequest, "invalid json")
		return
	}
	request.Kind = strings.TrimSpace(request.Kind)
	request.Provider = strings.TrimSpace(request.Provider)
	request.ExternalIdentity = strings.TrimSpace(request.ExternalIdentity)
	if !validWorkProductKind(request.Kind) || !validWorkProductProvider(request.Provider) || !validWorkProductExternalIdentity(request.ExternalIdentity) {
		writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_work_product", "invalid kind/provider/external_identity")
		return
	}
	var providerRecordID pgtype.UUID
	if strings.TrimSpace(request.ProviderRecordID) != "" {
		parsedID, parseErr := util.ParseUUID(strings.TrimSpace(request.ProviderRecordID))
		if parseErr != nil {
			writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_work_product", "invalid provider_record_id")
			return
		}
		providerRecordID = parsedID
	}
	externalURL := strings.TrimSpace(request.ExternalURL)
	providerRecordType := strings.TrimSpace(request.ProviderRecordType)
	executor, ok := h.requireWorkProductDB(w)
	if !ok {
		return
	}
	p, err := scanWorkProduct(executor.QueryRow(r.Context(), `
INSERT INTO work_product (workspace_id, kind, provider, external_identity, external_url, provider_record_type, provider_record_id)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (workspace_id, provider, external_identity) DO UPDATE SET
    external_url = COALESCE(EXCLUDED.external_url, work_product.external_url),
    provider_record_type = COALESCE(EXCLUDED.provider_record_type, work_product.provider_record_type),
    provider_record_id = COALESCE(EXCLUDED.provider_record_id, work_product.provider_record_id),
    updated_at = now()
RETURNING `+workProductColumns,
		workspaceUUID,
		request.Kind,
		request.Provider,
		request.ExternalIdentity,
		nullableWorkProductText(externalURL),
		nullableWorkProductText(providerRecordType),
		nullableWorkProductUUID(providerRecordID),
	))
	if err != nil {
		if isCheckViolation(err) || isUniqueViolation(err) {
			writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_work_product", "work product violates a database constraint")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	// The provider identity is the durable idempotency key. Repeating this
	// request converges the mirror and must not create another product.
	writeJSON(w, http.StatusOK, p)
}

func (h *Handler) ListWorkProductRelationsByIssue(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	iid, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "issue id")
	if !ok {
		return
	}
	limit, offset, err := workProductPage(r)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	executor, ok := h.requireWorkProductDB(w)
	if !ok {
		return
	}
	var issueScope pgtype.UUID
	if err := executor.QueryRow(r.Context(), `SELECT id FROM issue WHERE id = $1 AND workspace_id = $2`, iid, workspaceUUID).Scan(&issueScope); err != nil {
		if isNotFound(err) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "issue not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	// provider_reference relations record a bare mention of the issue in a
	// PR body. They are evidence somebody typed the identifier, not a claim
	// that the PR does the work, so they stay out of the issue's list for the
	// same reason they stay out of the close gate.
	rows, err := executor.Query(r.Context(), `SELECT `+workProductRelationColumns+` FROM work_product_relation WHERE workspace_id = $1 AND issue_id = $2 AND detached_at IS NULL AND relation_source IN ('manual_explicit', 'task_explicit', 'execution_branch_discovery', 'provider_discovery') ORDER BY attached_at DESC, id DESC LIMIT $3 OFFSET $4`, workspaceUUID, iid, limit+1, offset)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	defer rows.Close()
	relations := make([]db.WorkProductRelation, 0, int(limit))
	for rows.Next() {
		relation, scanErr := scanWorkProductRelation(rows)
		if scanErr != nil {
			writeError(w, http.StatusInternalServerError, "database error")
			return
		}
		relations = append(relations, relation)
	}
	if err := rows.Err(); err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	hasMore := len(relations) > int(limit)
	if hasMore {
		relations = relations[:limit]
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"relations": relations,
		"page":      int(offset/limit) + 1,
		"per_page":  limit,
		"has_more":  hasMore,
	})
}

func (h *Handler) CreateWorkProductRelation(w http.ResponseWriter, r *http.Request) {
	workspaceID, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	issueID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "issue id")
	if !ok {
		return
	}
	var request workProductRelationRequest
	if err := decodeWorkProductJSON(r, &request); err != nil {
		writeError(w, http.StatusBadRequest, "invalid json")
		return
	}
	productID, ok := parseUUIDOrBadRequest(w, request.WorkProductID, "work_product_id")
	if !ok {
		return
	}
	actor, ok := h.resolveWorkProductRelationActor(w, r, workspaceID, workspaceUUID, issueID)
	if !ok {
		return
	}
	if request.IssueID != "" {
		assertedIssue, parseOK := parseUUIDOrBadRequest(w, request.IssueID, "issue id")
		if !parseOK {
			return
		}
		if !sameWorkProductUUID(assertedIssue, issueID) {
			writeError(w, http.StatusForbidden, "relation issue does not match the request path")
			return
		}
	}
	expectedSource := "manual_explicit"
	expectedAttachedByType := "user"
	if actor.Type == "agent" {
		expectedSource = "task_explicit"
		expectedAttachedByType = "agent"
	}
	if request.RelationSource != "" && request.RelationSource != expectedSource {
		writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_relation", "relation source does not match the authenticated actor")
		return
	}
	if request.AttachedByType != "" && request.AttachedByType != expectedAttachedByType {
		writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_relation", "attached_by_type does not match the authenticated actor")
		return
	}
	if request.AttachedByID != "" {
		assertedActor, parseOK := parseUUIDOrBadRequest(w, request.AttachedByID, "attached_by_id")
		if !parseOK {
			return
		}
		if !sameWorkProductUUID(assertedActor, actor.ID) {
			writeError(w, http.StatusForbidden, "attached_by_id does not match the authenticated actor")
			return
		}
	}
	if request.TaskID != "" {
		assertedTask, parseOK := parseUUIDOrBadRequest(w, request.TaskID, "task id")
		if !parseOK {
			return
		}
		if !sameWorkProductUUID(assertedTask, actor.TaskID) {
			writeError(w, http.StatusForbidden, "relation task does not match the authenticated task")
			return
		}
	}
	if request.RunID != "" {
		assertedRun, parseOK := parseUUIDOrBadRequest(w, request.RunID, "run id")
		if !parseOK {
			return
		}
		if !sameWorkProductUUID(assertedRun, actor.RunID) {
			writeError(w, http.StatusForbidden, "relation run does not match the authenticated task run")
			return
		}
	}
	if actor.Type == "member" && (request.TaskID != "" || request.RunID != "") {
		writeError(w, http.StatusForbidden, "a member relation cannot carry task execution provenance")
		return
	}

	if h.TxStarter == nil {
		writeError(w, http.StatusInternalServerError, "database unavailable")
		return
	}
	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	rollback := func() { _ = tx.Rollback(r.Context()) }
	var lockedIssue pgtype.UUID
	if err := tx.QueryRow(r.Context(), `SELECT id FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE`, issueID, workspaceUUID).Scan(&lockedIssue); err != nil {
		rollback()
		if isNotFound(err) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "issue not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	if _, err := scanWorkProduct(tx.QueryRow(r.Context(), `SELECT `+workProductColumns+` FROM work_product WHERE id = $1 AND workspace_id = $2`, productID, workspaceUUID)); err != nil {
		rollback()
		if isNotFound(err) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "work product not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	relationKey := workProductRelationKey(issueID, actor.TaskID, actor.RunID)
	if actor.Type == "member" {
		// Manual confirmation converges an existing discovery/task relation for
		// this issue. The issue lock serializes this lookup with the insert.
		var existingKey string
		existingErr := tx.QueryRow(r.Context(), `SELECT relation_key FROM work_product_relation WHERE workspace_id = $1 AND work_product_id = $2 AND issue_id = $3 AND detached_at IS NULL ORDER BY attached_at ASC, id ASC LIMIT 1`, workspaceUUID, productID, issueID).Scan(&existingKey)
		if existingErr == nil {
			relationKey = existingKey
		} else if !isNotFound(existingErr) {
			rollback()
			writeError(w, http.StatusInternalServerError, "database error")
			return
		}
	}
	canonicalKey := workProductRelationKey(issueID, actor.TaskID, actor.RunID)
	if request.RelationKey != "" && request.RelationKey != relationKey && request.RelationKey != canonicalKey {
		rollback()
		writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_relation", "relation key is not server-derived")
		return
	}
	relation, err := scanWorkProductRelation(tx.QueryRow(r.Context(), `
WITH product_scope AS (
    SELECT id FROM work_product WHERE id = $2 AND workspace_id = $1
), issue_scope AS (
    SELECT id FROM issue WHERE id = $3 AND workspace_id = $1
)
INSERT INTO work_product_relation (
    workspace_id, work_product_id, issue_id, task_id, run_id, relation_key,
    relation_source, attached_by_type, attached_by_id, close_intent
)
SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10
FROM product_scope, issue_scope
WHERE ($4::uuid IS NULL OR EXISTS (
    SELECT 1
    FROM agent_task_queue task
    JOIN agent ON agent.id = task.agent_id
    WHERE task.id = $4::uuid
      AND agent.workspace_id = $1
      AND task.issue_id = $3::uuid
      AND task.agent_id = $9::uuid
))
  AND ($5::uuid IS NULL OR EXISTS (
    SELECT 1
    FROM agent_task_queue task
    JOIN agent ON agent.id = task.agent_id
    WHERE task.id = $4::uuid
      AND task.automation_run_id = $5::uuid
      AND agent.workspace_id = $1
))
ON CONFLICT (work_product_id, relation_key) WHERE detached_at IS NULL DO UPDATE SET
    relation_source = CASE
        WHEN work_product_relation.relation_source = 'execution_branch_discovery'
         AND EXCLUDED.relation_source IN ('task_explicit', 'manual_explicit')
        THEN EXCLUDED.relation_source
        ELSE work_product_relation.relation_source
    END,
    attached_by_type = CASE
        WHEN work_product_relation.relation_source = 'execution_branch_discovery'
         AND EXCLUDED.relation_source IN ('task_explicit', 'manual_explicit')
        THEN EXCLUDED.attached_by_type
        ELSE work_product_relation.attached_by_type
    END,
    attached_by_id = CASE
        WHEN work_product_relation.relation_source = 'execution_branch_discovery'
         AND EXCLUDED.relation_source IN ('task_explicit', 'manual_explicit')
        THEN EXCLUDED.attached_by_id
        ELSE work_product_relation.attached_by_id
    END,
    close_intent = work_product_relation.close_intent OR EXCLUDED.close_intent
RETURNING `+workProductRelationColumns,
		workspaceUUID,
		productID,
		issueID,
		nullableWorkProductUUID(actor.TaskID),
		nullableWorkProductUUID(actor.RunID),
		relationKey,
		expectedSource,
		expectedAttachedByType,
		actor.ID,
		request.CloseIntent,
	))
	if err != nil {
		rollback()
		if isNotFound(err) {
			writeError(w, http.StatusForbidden, "relation task or run is outside the workspace")
			return
		}
		if isCheckViolation(err) || isUniqueViolation(err) {
			writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_relation", "relation violates a database constraint")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	if err := tx.Commit(r.Context()); err != nil {
		rollback()
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	h.recordWorkProductRelationActivity(r, workProductAttachedActivity, actor, relation)
	writeJSON(w, http.StatusOK, relation)
}

func (h *Handler) GetProvenanceByTask(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	taskID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "taskId"), "task id")
	if !ok {
		return
	}
	executor, ok := h.requireWorkProductDB(w)
	if !ok {
		return
	}
	if h.Queries == nil {
		writeError(w, http.StatusInternalServerError, "database unavailable")
		return
	}
	if _, err := h.Queries.GetAgentTaskInWorkspace(r.Context(), db.GetAgentTaskInWorkspaceParams{ID: taskID, WorkspaceID: workspaceUUID}); err != nil {
		writeErrorCode(w, http.StatusNotFound, "not_found", "task not found")
		return
	}
	p, err := scanWorkProductProvenance(executor.QueryRow(r.Context(), `SELECT `+workProductProvenanceColumns+` FROM agent_task_execution_provenance WHERE workspace_id = $1 AND task_id = $2 ORDER BY updated_at DESC, repo_identity ASC, execution_workspace ASC LIMIT 1`, workspaceUUID, taskID))
	if err != nil {
		if isNotFound(err) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusOK, p)
}

func (h *Handler) UpsertProvenance(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	taskID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "taskId"), "task id")
	if !ok {
		return
	}
	task, ok := h.authorizeWorkProductProvenanceTask(w, r, workspaceUUID, taskID)
	if !ok {
		return
	}
	var request workProductProvenanceRequest
	if err := decodeWorkProductJSON(r, &request); err != nil {
		writeError(w, http.StatusBadRequest, "invalid json")
		return
	}
	values, err := normalizeWorkProductProvenance(request, task)
	if err != nil {
		writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_provenance", err.Error())
		return
	}
	runID := task.AutomationRunID
	if strings.TrimSpace(request.RunID) != "" {
		assertedRun, parseOK := parseUUIDOrBadRequest(w, request.RunID, "run id")
		if !parseOK {
			return
		}
		if !sameWorkProductUUID(assertedRun, runID) {
			writeError(w, http.StatusForbidden, "provenance run does not match the task run")
			return
		}
	}
	executor, ok := h.requireWorkProductDB(w)
	if !ok {
		return
	}
	p, err := scanWorkProductProvenance(executor.QueryRow(r.Context(), `
INSERT INTO agent_task_execution_provenance (
    workspace_id, task_id, run_id, repo_identity, execution_workspace,
    head_branch, head_sha, head_state, started_at, discovery_status
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now(), 'not_attempted')
ON CONFLICT (workspace_id, task_id, repo_identity, execution_workspace) DO UPDATE SET
    run_id = COALESCE(agent_task_execution_provenance.run_id, EXCLUDED.run_id),
    head_branch = CASE
        WHEN EXCLUDED.head_state <> 'unknown' THEN EXCLUDED.head_branch
        ELSE COALESCE(agent_task_execution_provenance.head_branch, EXCLUDED.head_branch)
    END,
    head_sha = CASE
        WHEN EXCLUDED.head_state <> 'unknown' THEN EXCLUDED.head_sha
        ELSE COALESCE(agent_task_execution_provenance.head_sha, EXCLUDED.head_sha)
    END,
    head_state = CASE
        WHEN EXCLUDED.head_state <> 'unknown' THEN EXCLUDED.head_state
        ELSE agent_task_execution_provenance.head_state
    END,
    started_at = COALESCE(agent_task_execution_provenance.started_at, EXCLUDED.started_at),
    updated_at = now()
RETURNING `+workProductProvenanceColumns,
		workspaceUUID,
		taskID,
		nullableWorkProductUUID(runID),
		values.RepoIdentity,
		values.ExecutionWorkspace,
		nullableWorkProductText(valueOrEmpty(values.HeadBranch)),
		nullableWorkProductText(valueOrEmpty(values.HeadSHA)),
		values.HeadState,
	))
	if err != nil {
		if isCheckViolation(err) || isUniqueViolation(err) {
			writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_provenance", "provenance violates a database constraint")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusOK, p)
}

func (h *Handler) authorizeWorkProductProvenanceTask(w http.ResponseWriter, r *http.Request, workspaceUUID, taskID pgtype.UUID) (db.AgentTaskQueue, bool) {
	if r.Header.Get("X-Actor-Source") != "task_token" {
		writeError(w, http.StatusForbidden, "execution provenance requires a task execution token")
		return db.AgentTaskQueue{}, false
	}
	claimedTask, ok := parseUUIDOrBadRequest(w, strings.TrimSpace(r.Header.Get("X-Task-ID")), "task id")
	if !ok {
		return db.AgentTaskQueue{}, false
	}
	if !sameWorkProductUUID(claimedTask, taskID) {
		writeError(w, http.StatusForbidden, "task token is bound to a different task")
		return db.AgentTaskQueue{}, false
	}
	claimedAgent, err := util.ParseUUID(strings.TrimSpace(r.Header.Get("X-Agent-ID")))
	if err != nil {
		writeError(w, http.StatusForbidden, "task execution context is invalid")
		return db.AgentTaskQueue{}, false
	}
	if h.Queries == nil {
		writeError(w, http.StatusInternalServerError, "database unavailable")
		return db.AgentTaskQueue{}, false
	}
	task, err := h.Queries.GetAgentTaskInWorkspace(r.Context(), db.GetAgentTaskInWorkspaceParams{ID: taskID, WorkspaceID: workspaceUUID})
	if err != nil || !sameWorkProductUUID(task.AgentID, claimedAgent) {
		writeError(w, http.StatusForbidden, "task execution context is invalid")
		return db.AgentTaskQueue{}, false
	}
	if task.Status != "running" {
		writeError(w, http.StatusConflict, "execution provenance requires a running task")
		return db.AgentTaskQueue{}, false
	}
	return task, true
}

func normalizeWorkProductFact(value string) (*string, error) {
	value = strings.TrimSpace(value)
	if value == "" {
		return nil, nil
	}
	for _, r := range value {
		if r == 0 || r < 0x20 || r == 0x7f {
			return nil, errors.New("execution fact contains a control character")
		}
	}
	return &value, nil
}

func normalizeWorkProductProvenance(request workProductProvenanceRequest, task db.AgentTaskQueue) (workProductProvenanceValues, error) {
	repoIdentity := strings.TrimSpace(request.RepoIdentity)
	if repoIdentity != "" {
		normalized, ok := normalizeWorkProductRepoIdentity(repoIdentity)
		if !ok {
			return workProductProvenanceValues{}, errors.New("invalid repository identity")
		}
		repoIdentity = normalized
	}
	executionWorkspace := strings.TrimSpace(request.ExecutionWorkspace)
	if executionWorkspace != "" {
		if strings.ContainsRune(executionWorkspace, 0) || !filepath.IsAbs(executionWorkspace) {
			return workProductProvenanceValues{}, errors.New("execution workspace must be absolute")
		}
		if !taskExecutionWorkspaceMatches(textWorkProductValue(task.WorkDir), textWorkProductValue(task.DurableWorkDir), executionWorkspace) {
			return workProductProvenanceValues{}, errors.New("execution workspace is not owned by task")
		}
	}
	headState := strings.TrimSpace(request.HeadState)
	if headState == "" {
		headState = "unknown"
	}
	if headState != "attached" && headState != "detached" && headState != "default" && headState != "unknown" {
		return workProductProvenanceValues{}, errors.New("invalid execution head state")
	}
	headBranch, err := normalizeWorkProductFact(request.HeadBranch)
	if err != nil {
		return workProductProvenanceValues{}, err
	}
	headSHA, err := normalizeWorkProductFact(request.HeadSHA)
	if err != nil {
		return workProductProvenanceValues{}, err
	}
	if headState == "attached" && (repoIdentity == "" || executionWorkspace == "" || headBranch == nil) {
		return workProductProvenanceValues{}, errors.New("attached execution requires repository, workspace, and branch")
	}
	if status := strings.TrimSpace(request.DiscoveryStatus); status != "" && status != "not_attempted" {
		return workProductProvenanceValues{}, errors.New("discovery status is server-managed")
	}
	if strings.TrimSpace(request.DiscoveryReason) != "" {
		return workProductProvenanceValues{}, errors.New("discovery reason is server-managed")
	}
	return workProductProvenanceValues{
		RepoIdentity:       repoIdentity,
		ExecutionWorkspace: executionWorkspace,
		HeadBranch:         headBranch,
		HeadSHA:            headSHA,
		HeadState:          headState,
	}, nil
}

func valueOrEmpty(value *string) string {
	if value == nil {
		return ""
	}
	return *value
}

func (h *Handler) ListProvenanceByWorkspace(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	limit, offset, err := workProductPage(r)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	executor, ok := h.requireWorkProductDB(w)
	if !ok {
		return
	}
	rows, err := executor.Query(r.Context(), `SELECT `+workProductProvenanceColumns+` FROM agent_task_execution_provenance WHERE workspace_id = $1 ORDER BY updated_at DESC, task_id DESC, repo_identity ASC, execution_workspace ASC LIMIT $2 OFFSET $3`, workspaceUUID, limit+1, offset)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	defer rows.Close()
	provenance := make([]db.AgentTaskExecutionProvenance, 0, int(limit))
	for rows.Next() {
		row, scanErr := scanWorkProductProvenance(rows)
		if scanErr != nil {
			writeError(w, http.StatusInternalServerError, "database error")
			return
		}
		provenance = append(provenance, row)
	}
	if err := rows.Err(); err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	hasMore := len(provenance) > int(limit)
	if hasMore {
		provenance = provenance[:limit]
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"provenance": provenance,
		"page":       int(offset/limit) + 1,
		"per_page":   limit,
		"has_more":   hasMore,
	})
}

// classifyBranchDiscovery is the durable discovery policy. A provider match
// is never enough by itself: the execution
// must have an attached head, no other persisted execution may claim the same
// workspace/repository/branch, and exactly one provider object must match.
func classifyBranchDiscovery(headState string, otherExecutionCount, matchCount int) branchDiscoveryDecision {
	if headState != "attached" {
		reason := "unknown_head_state"
		switch headState {
		case "default":
			reason = "default_branch"
		case "detached":
			reason = "detached_head"
		}
		return branchDiscoveryDecision{Status: "ineligible", Reason: reason}
	}
	if otherExecutionCount > 0 {
		return branchDiscoveryDecision{Status: "ambiguous", Reason: "branch_used_by_other_execution"}
	}
	switch matchCount {
	case 0:
		return branchDiscoveryDecision{Status: "unassociated"}
	case 1:
		return branchDiscoveryDecision{Status: "associated"}
	default:
		return branchDiscoveryDecision{Status: "ambiguous", Reason: "multiple_pull_requests_for_exact_head"}
	}
}

func taskExecutionWorkspaceMatches(workDir, durableWorkDir, executionWorkspace string) bool {
	if executionWorkspace == "" || strings.ContainsRune(executionWorkspace, 0) || !filepath.IsAbs(executionWorkspace) {
		return false
	}
	executionPath := filepath.Clean(executionWorkspace)
	for _, known := range []string{workDir, durableWorkDir} {
		known = strings.TrimSpace(known)
		if known == "" || strings.ContainsRune(known, 0) || !filepath.IsAbs(known) {
			continue
		}
		knownPath := filepath.Clean(known)
		if pathWithinWorkProductRoot(executionPath, knownPath) || pathWithinWorkProductRoot(knownPath, executionPath) {
			return true
		}
	}
	return false
}

func pathWithinWorkProductRoot(candidate, root string) bool {
	if candidate == root {
		return true
	}
	relative, err := filepath.Rel(root, candidate)
	return err == nil && relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator)) && !filepath.IsAbs(relative)
}

func headRepositoryMatches(headRepoIdentity, expected string) bool {
	actual, ok := normalizeWorkProductRepoIdentity(headRepoIdentity)
	if !ok {
		return false
	}
	expected, ok = normalizeWorkProductRepoIdentity(expected)
	return ok && actual == expected
}

func workspaceContainsRepo(repos []byte, candidate string) bool {
	expected, ok := normalizeWorkProductRepoIdentity(candidate)
	if !ok {
		return false
	}
	var entries []struct {
		URL string `json:"url"`
	}
	if err := json.Unmarshal(repos, &entries); err != nil {
		return false
	}
	for _, entry := range entries {
		if identity, valid := normalizeWorkProductRepoIdentity(entry.URL); valid && identity == expected {
			return true
		}
	}
	return false
}

// normalizeWorkProductRepoIdentity accepts transport forms only. It does not
// inspect issue titles, PR bodies, branch names, or task identifiers.
func normalizeWorkProductRepoIdentity(raw string) (string, bool) {
	value := strings.TrimSpace(strings.TrimRight(raw, "/"))
	switch {
	case strings.HasPrefix(value, "https://github.com/"):
		value = strings.TrimPrefix(value, "https://github.com/")
	case strings.HasPrefix(value, "http://github.com/"):
		value = strings.TrimPrefix(value, "http://github.com/")
	case strings.HasPrefix(value, "git@github.com:"):
		value = strings.TrimPrefix(value, "git@github.com:")
	case strings.HasPrefix(value, "ssh://git@github.com/"):
		value = strings.TrimPrefix(value, "ssh://git@github.com/")
	}
	value = strings.TrimSuffix(strings.TrimRight(value, "/"), ".git")
	parts := strings.Split(value, "/")
	if len(parts) != 2 || parts[0] == "" || parts[1] == "" {
		return "", false
	}
	for _, part := range parts {
		for i := 0; i < len(part); i++ {
			if !validWorkProductRepoByte(part[i]) {
				return "", false
			}
		}
	}
	return strings.ToLower(parts[0]) + "/" + strings.ToLower(parts[1]), true
}

func validWorkProductRepoByte(value byte) bool {
	return (value >= 'a' && value <= 'z') || (value >= 'A' && value <= 'Z') || (value >= '0' && value <= '9') || value == '.' || value == '-' || value == '_' || value == '~'
}

// markWorkProductDiscoveryPending is the durable handoff used by terminal
// task code. It changes only not_attempted rows; a restart can safely replay a
// pending row, while a final discovery result is never reopened.
func (h *Handler) markWorkProductDiscoveryPending(r *http.Request, workspaceID, taskID pgtype.UUID) error {
	if h.DB == nil {
		return errors.New("database unavailable")
	}
	_, err := h.DB.Exec(r.Context(), `UPDATE agent_task_execution_provenance SET discovery_status = 'pending', discovery_match_count = 0, discovery_reason = NULL, discovery_work_product_id = NULL, discovery_at = NULL, updated_at = now() WHERE workspace_id = $1 AND task_id = $2 AND discovery_status = 'not_attempted'`, workspaceID, taskID)
	return err
}

func (h *Handler) claimWorkProductDiscovery(r *http.Request, provenance db.AgentTaskExecutionProvenance) (db.AgentTaskExecutionProvenance, bool, error) {
	if h.DB == nil {
		return db.AgentTaskExecutionProvenance{}, false, errors.New("database unavailable")
	}
	claimed, err := scanWorkProductProvenance(h.DB.QueryRow(r.Context(), `UPDATE agent_task_execution_provenance SET discovery_status = 'in_progress', discovery_at = now(), updated_at = now() WHERE workspace_id = $1 AND task_id = $2 AND repo_identity = $3 AND execution_workspace = $4 AND (discovery_status = 'pending' OR (discovery_status = 'in_progress' AND updated_at < now() - interval '5 minutes')) RETURNING `+workProductProvenanceColumns, provenance.WorkspaceID, provenance.TaskID, provenance.RepoIdentity, provenance.ExecutionWorkspace))
	if err != nil {
		if isNotFound(err) {
			return db.AgentTaskExecutionProvenance{}, false, nil
		}
		return db.AgentTaskExecutionProvenance{}, false, err
	}
	return claimed, true, nil
}

func (h *Handler) recordWorkProductDiscovery(r *http.Request, provenance db.AgentTaskExecutionProvenance, status string, matchCount int32, reason string, productID pgtype.UUID) error {
	if status != "unassociated" && status != "ambiguous" && status != "associated" && status != "ineligible" {
		return errors.New("invalid discovery status")
	}
	if matchCount < 0 {
		return errors.New("invalid discovery match count")
	}
	if h.DB == nil {
		return errors.New("database unavailable")
	}
	_, err := h.DB.Exec(r.Context(), `UPDATE agent_task_execution_provenance SET discovery_status = $3, discovery_match_count = $4, discovery_reason = $5, discovery_work_product_id = $6, discovery_at = now(), updated_at = now() WHERE workspace_id = $1 AND task_id = $2 AND repo_identity = $7 AND execution_workspace = $8 AND discovery_status IN ('not_attempted', 'pending', 'in_progress')`, provenance.WorkspaceID, provenance.TaskID, status, matchCount, nullableWorkProductText(reason), nullableWorkProductUUID(productID), provenance.RepoIdentity, provenance.ExecutionWorkspace)
	return err
}
