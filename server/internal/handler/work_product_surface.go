package handler

import (
	"context"
	"encoding/json"
	"log/slog"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/logger"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

// Work Product is the single contract for "something an issue or a task
// produced". Pull requests used to have a parallel one — their own link
// tables, their own `/pull-requests` endpoint, their own list component — and
// the two disagreed the moment either side was written without the other.
// These surfaces replace it: one list per anchor (issue, task), each row
// carrying the product, the relation that attached it, and, for a product that
// mirrors a provider pull request, the PR card rendered from the same mirror
// the old endpoint read.

const (
	// workProductAttachedActivity and workProductDetachedActivity are the
	// activity_log actions for relation lifecycle. Attaching a product is a
	// claim about what resolved an issue, and detaching retracts it; both are
	// judgement calls a human may later need to trace back to whoever made
	// them, which a soft-deleted row alone does not answer (it records that a
	// relation ended, not that anyone went looking).
	workProductAttachedActivity = "work_product_attached"
	workProductDetachedActivity = "work_product_detached"
)

// WorkProductRelationView is the relation as a Work Product surface presents
// it: why the product is attached and who attached it, without the internal
// relation_key or the detach columns, which are always empty on a live row.
type WorkProductRelationView struct {
	ID             string  `json:"id"`
	IssueID        *string `json:"issue_id,omitempty"`
	TaskID         *string `json:"task_id,omitempty"`
	RunID          *string `json:"run_id,omitempty"`
	RelationSource string  `json:"relation_source"`
	AttachedByType string  `json:"attached_by_type"`
	AttachedByID   *string `json:"attached_by_id"`
	AttachedAt     string  `json:"attached_at"`
	CloseIntent    bool    `json:"close_intent"`
}

// WorkProductView is one row of a Work Product surface. PullRequest is set
// only when the product mirrors a provider pull request; every other kind
// leaves it out rather than sending an empty card, so a client can branch on
// presence instead of on a sentinel.
type WorkProductView struct {
	ID                 string                     `json:"id"`
	WorkspaceID        string                     `json:"workspace_id"`
	Kind               string                     `json:"kind"`
	Provider           string                     `json:"provider"`
	ExternalIdentity   string                     `json:"external_identity"`
	ExternalURL        *string                    `json:"external_url"`
	ProviderRecordType *string                    `json:"provider_record_type"`
	ProviderRecordID   *string                    `json:"provider_record_id"`
	CreatedAt          string                     `json:"created_at"`
	UpdatedAt          string                     `json:"updated_at"`
	AssociationState   string                     `json:"association_state"`
	Relation           WorkProductRelationView    `json:"relation"`
	PullRequest        *GitHubPullRequestResponse `json:"pull_request,omitempty"`
}

// workProductRow is the shape both list queries return. sqlc generates a
// distinct row type per query even when the columns match, so the two are
// funnelled through this one struct and hydrated by a single code path.
type workProductRow struct {
	Product         db.WorkProduct
	RelationID      pgtype.UUID
	RelationIssueID pgtype.UUID
	RelationTaskID  pgtype.UUID
	RelationRunID   pgtype.UUID
	RelationSource  string
	AttachedByType  string
	AttachedByID    pgtype.UUID
	AttachedAt      pgtype.Timestamptz
	CloseIntent     bool
}

func workProductRowFromIssue(row db.ListWorkProductsByIssueRow) workProductRow {
	return workProductRow{
		Product: db.WorkProduct{
			ID:                 row.ID,
			WorkspaceID:        row.WorkspaceID,
			Kind:               row.Kind,
			Provider:           row.Provider,
			ExternalIdentity:   row.ExternalIdentity,
			ExternalUrl:        row.ExternalUrl,
			ProviderRecordType: row.ProviderRecordType,
			ProviderRecordID:   row.ProviderRecordID,
			CreatedAt:          row.CreatedAt,
			UpdatedAt:          row.UpdatedAt,
		},
		RelationID:      row.RelationID,
		RelationIssueID: row.RelationIssueID,
		RelationTaskID:  row.RelationTaskID,
		RelationRunID:   row.RelationRunID,
		RelationSource:  row.RelationSource,
		AttachedByType:  row.AttachedByType,
		AttachedByID:    row.AttachedByID,
		AttachedAt:      row.AttachedAt,
		CloseIntent:     row.CloseIntent,
	}
}

func workProductRowFromTask(row db.ListWorkProductsByTaskRow) workProductRow {
	return workProductRow{
		Product: db.WorkProduct{
			ID:                 row.ID,
			WorkspaceID:        row.WorkspaceID,
			Kind:               row.Kind,
			Provider:           row.Provider,
			ExternalIdentity:   row.ExternalIdentity,
			ExternalUrl:        row.ExternalUrl,
			ProviderRecordType: row.ProviderRecordType,
			ProviderRecordID:   row.ProviderRecordID,
			CreatedAt:          row.CreatedAt,
			UpdatedAt:          row.UpdatedAt,
		},
		RelationID:      row.RelationID,
		RelationIssueID: row.RelationIssueID,
		RelationTaskID:  row.RelationTaskID,
		RelationRunID:   row.RelationRunID,
		RelationSource:  row.RelationSource,
		AttachedByType:  row.AttachedByType,
		AttachedByID:    row.AttachedByID,
		AttachedAt:      row.AttachedAt,
		CloseIntent:     row.CloseIntent,
	}
}

func workProductUUIDPtr(value pgtype.UUID) *string {
	if !value.Valid {
		return nil
	}
	text := uuidToString(value)
	return &text
}

func workProductTextPtr(value pgtype.Text) *string {
	if !value.Valid {
		return nil
	}
	return &value.String
}

// workProductViewFromRow maps a list row onto the wire shape. Hydration of the
// pull-request card is deliberately separate: it costs a query per row, so a
// caller that does not need the card does not pay for it.
func workProductViewFromRow(row workProductRow) WorkProductView {
	return WorkProductView{
		ID:                 uuidToString(row.Product.ID),
		WorkspaceID:        uuidToString(row.Product.WorkspaceID),
		Kind:               row.Product.Kind,
		Provider:           row.Product.Provider,
		ExternalIdentity:   row.Product.ExternalIdentity,
		ExternalURL:        workProductTextPtr(row.Product.ExternalUrl),
		ProviderRecordType: workProductTextPtr(row.Product.ProviderRecordType),
		ProviderRecordID:   workProductUUIDPtr(row.Product.ProviderRecordID),
		CreatedAt:          timestampToString(row.Product.CreatedAt),
		UpdatedAt:          timestampToString(row.Product.UpdatedAt),
		AssociationState:   "associated",
		Relation: WorkProductRelationView{
			ID:             uuidToString(row.RelationID),
			IssueID:        workProductUUIDPtr(row.RelationIssueID),
			TaskID:         workProductUUIDPtr(row.RelationTaskID),
			RunID:          workProductUUIDPtr(row.RelationRunID),
			RelationSource: row.RelationSource,
			AttachedByType: row.AttachedByType,
			AttachedByID:   workProductUUIDPtr(row.AttachedByID),
			AttachedAt:     timestampToString(row.AttachedAt),
			CloseIntent:    row.CloseIntent,
		},
	}
}

func workProductRelationViewFromDB(relation db.WorkProductRelation) WorkProductRelationView {
	return WorkProductRelationView{
		ID:             uuidToString(relation.ID),
		IssueID:        workProductUUIDPtr(relation.IssueID),
		TaskID:         workProductUUIDPtr(relation.TaskID),
		RunID:          workProductUUIDPtr(relation.RunID),
		RelationSource: relation.RelationSource,
		AttachedByType: relation.AttachedByType,
		AttachedByID:   workProductUUIDPtr(relation.AttachedByID),
		AttachedAt:     timestampToString(relation.AttachedAt),
		CloseIntent:    relation.CloseIntent,
	}
}

// workProductCatalogResponse is shared by the explicit attach, unassociated
// catalog, and detach event contracts. The optional relation is deliberate:
// an unassociated provider mirror has no issue claim to expose.
func workProductCatalogResponse(product db.WorkProduct, relation *db.WorkProductRelation) map[string]any {
	var relationResponse any
	associationState := "unassociated"
	if relation != nil {
		relationResponse = workProductRelationViewFromDB(*relation)
		associationState = "associated"
	}
	return map[string]any{
		"id":                   uuidToString(product.ID),
		"workspace_id":         uuidToString(product.WorkspaceID),
		"kind":                 product.Kind,
		"provider":             product.Provider,
		"external_identity":    product.ExternalIdentity,
		"external_url":         workProductTextPtr(product.ExternalUrl),
		"provider_record_type": workProductTextPtr(product.ProviderRecordType),
		"provider_record_id":   workProductUUIDPtr(product.ProviderRecordID),
		"created_at":           timestampToString(product.CreatedAt),
		"updated_at":           timestampToString(product.UpdatedAt),
		"association_state":    associationState,
		"relation":             relationResponse,
	}
}

func workProductRelationResponse(relation db.WorkProductRelation) map[string]any {
	return map[string]any{
		"id":               uuidToString(relation.ID),
		"workspace_id":     uuidToString(relation.WorkspaceID),
		"work_product_id":  uuidToString(relation.WorkProductID),
		"issue_id":         workProductUUIDPtr(relation.IssueID),
		"task_id":          workProductUUIDPtr(relation.TaskID),
		"run_id":           workProductUUIDPtr(relation.RunID),
		"relation_key":     relation.RelationKey,
		"relation_source":  relation.RelationSource,
		"attached_by_type":  relation.AttachedByType,
		"attached_by_id":   workProductUUIDPtr(relation.AttachedByID),
		"attached_at":      timestampToString(relation.AttachedAt),
		"close_intent":     relation.CloseIntent,
		"detached_at":      timestampToPtr(relation.DetachedAt),
		"detached_by_type": workProductTextPtr(relation.DetachedByType),
		"detached_by_id":   workProductUUIDPtr(relation.DetachedByID),
		"detached_task_id": workProductUUIDPtr(relation.DetachedTaskID),
		"detached_run_id":  workProductUUIDPtr(relation.DetachedRunID),
	}
}

// hydratePullRequest fills in the PR card for a product that mirrors a
// provider pull request. A mirror row that has gone missing is not an error:
// the product still exists and still names the PR by URL, so the row is
// returned without a card rather than failing the whole list.
func (h *Handler) hydratePullRequest(r *http.Request, view *WorkProductView, row workProductRow) {
	if h.Queries == nil || !row.Product.ProviderRecordType.Valid || !row.Product.ProviderRecordID.Valid {
		return
	}
	switch row.Product.ProviderRecordType.String {
	case "github_pull_request":
		pr, err := h.Queries.GetGitHubPullRequestForWorkProduct(r.Context(), row.Product.ProviderRecordID)
		if err != nil {
			return
		}
		snapshotEnabled := h.PRRefresh != nil && h.PRRefresh.Enabled()
		card := githubWorkProductPullRequestToResponse(pr, snapshotEnabled)
		view.PullRequest = &card
		// Page-visit trigger (MUL-5265): a card whose snapshot is missing or
		// past the view TTL kicks an async refresh. Non-blocking — the
		// possibly stale card ships now and the fresh snapshot arrives over
		// the pull_request:updated realtime event.
		if h.PRRefresh != nil {
			h.PRRefresh.MaybeEnqueueOnView(
				pr.InstallationID, pr.RepoOwner, pr.RepoName, pr.PrNumber,
				pr.SnapshotFetchedAt.Time,
				pr.SnapshotFetchedAt.Valid &&
					pr.SnapshotHeadSha != "" &&
					pr.SnapshotHeadSha == pr.HeadSha,
			)
		}
	case "vcs_pull_request":
		pr, err := h.Queries.GetVCSPullRequestForWorkProduct(r.Context(), row.Product.ProviderRecordID)
		if err != nil {
			return
		}
		card := vcsWorkProductPullRequestToResponse(pr)
		view.PullRequest = &card
	}
}

func (h *Handler) workProductViews(r *http.Request, rows []workProductRow, limit int32) ([]WorkProductView, bool) {
	hasMore := len(rows) > int(limit)
	if hasMore {
		rows = rows[:limit]
	}
	views := make([]WorkProductView, 0, len(rows))
	for _, row := range rows {
		view := workProductViewFromRow(row)
		h.hydratePullRequest(r, &view, row)
		views = append(views, view)
	}
	return views, hasMore
}

func writeWorkProductPage(w http.ResponseWriter, views []WorkProductView, limit, offset int32, hasMore bool) {
	writeJSON(w, http.StatusOK, map[string]any{
		"work_products": views,
		"page":          int(offset/limit) + 1,
		"per_page":      limit,
		"has_more":      hasMore,
	})
}

// ListWorkProductsForIssue (GET /api/issues/{id}/work-products) is the issue's
// delivery list. It replaces the PR-only endpoint: pull requests now arrive as
// products whose relation says why they are attached, alongside every other
// kind of product the issue produced.
func (h *Handler) ListWorkProductsForIssue(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	if h.Queries == nil {
		writeError(w, http.StatusInternalServerError, "database unavailable")
		return
	}
	issue, ok := h.resolveWorkProductIssue(w, r, workspaceUUID)
	if !ok {
		return
	}
	rows, err := h.Queries.ListWorkProductsByIssue(r.Context(), db.ListWorkProductsByIssueParams{
		WorkspaceID: workspaceUUID,
		IssueID:     issue.ID,
		Limit:       workProductMaxPage,
		Offset:      0,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	converted := make([]workProductRow, 0, len(rows))
	for _, row := range rows {
		converted = append(converted, workProductRowFromIssue(row))
	}
	views, _ := h.workProductViews(r, converted, int32(len(converted)))
	writeJSON(w, http.StatusOK, map[string]any{"work_products": views})
}

// ListWorkProductsForTask (GET /api/tasks/{taskId}/work-products) answers
// "what did this run actually produce". Unlike the issue list it keeps every
// relation source, because a task's own discovery record is the point.
func (h *Handler) ListWorkProductsForTask(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	taskID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "taskId"), "task id")
	if !ok {
		return
	}
	if h.Queries == nil {
		writeError(w, http.StatusInternalServerError, "database unavailable")
		return
	}
	// The task id is caller-supplied, so it is resolved inside the workspace
	// before it reaches the list query. Without this a member of workspace A
	// could read workspace B's products by guessing a task id: the list query
	// filters products by workspace but the relation's task anchor would not
	// be checked against anything.
	if _, err := h.Queries.GetAgentTaskInWorkspace(r.Context(), db.GetAgentTaskInWorkspaceParams{
		ID:          taskID,
		WorkspaceID: workspaceUUID,
	}); err != nil {
		writeErrorCode(w, http.StatusNotFound, "not_found", "task not found")
		return
	}
	rows, err := h.Queries.ListWorkProductsByTask(r.Context(), db.ListWorkProductsByTaskParams{
		WorkspaceID: workspaceUUID,
		TaskID:      taskID,
		Limit:       workProductMaxPage,
		Offset:      0,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	converted := make([]workProductRow, 0, len(rows))
	for _, row := range rows {
		converted = append(converted, workProductRowFromTask(row))
	}
	views, _ := h.workProductViews(r, converted, int32(len(converted)))
	provenances, err := h.Queries.ListExecutionProvenanceByTask(r.Context(), db.ListExecutionProvenanceByTaskParams{
		WorkspaceID: workspaceUUID,
		TaskID:      taskID,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to load execution provenance")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"task_id":       uuidToString(taskID),
		"provenances":   provenances,
		"work_products": views,
	})
}

// DetachWorkProduct (DELETE /api/issues/{id}/work-products/{workProductId})
// retracts every live claim for the product on this issue that the
// authenticated actor may retract. The relation rows remain as an audit trail.
func (h *Handler) DetachWorkProduct(w http.ResponseWriter, r *http.Request) {
	workspaceID, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	productID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "workProductId"), "work product id")
	if !ok {
		return
	}
	issue, ok := h.resolveWorkProductIssue(w, r, workspaceUUID)
	if !ok {
		return
	}
	issueID := issue.ID
	actor, ok := h.resolveWorkProductRelationActor(w, r, workspaceID, workspaceUUID, issueID)
	if !ok {
		return
	}
	if h.Queries == nil {
		writeError(w, http.StatusInternalServerError, "database unavailable")
		return
	}
	// 'user' and 'agent' are the schema's actor vocabulary; the handler's own
	// word for a human is "member", so it is translated once, here.
	detachedByType := "user"
	if actor.Type == "agent" {
		detachedByType = "agent"
	}
	product, err := h.Queries.GetWorkProductByID(r.Context(), db.GetWorkProductByIDParams{ID: productID, WorkspaceID: workspaceUUID})
	if err != nil {
		if isNotFound(err) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "work product not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
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
		writeError(w, http.StatusNotFound, "issue not found")
		return
	}
	detached, err := h.Queries.WithTx(tx).DetachWorkProductRelationsForIssue(r.Context(), db.DetachWorkProductRelationsForIssueParams{
		WorkProductID:  productID,
		WorkspaceID:    workspaceUUID,
		IssueID:        issueID,
		DetachedByType: pgtype.Text{String: detachedByType, Valid: true},
		DetachedByID:   actor.ID,
		DetachedTaskID: actor.TaskID,
		DetachedRunID:  actor.RunID,
	})
	if err != nil {
		rollback()
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	if len(detached) == 0 {
		rollback()
		writeErrorCode(w, http.StatusNotFound, "not_found", "work product relation not found")
		return
	}
	if err := tx.Commit(r.Context()); err != nil {
		rollback()
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	for _, relation := range detached {
		h.recordWorkProductRelationActivity(r, workProductDetachedActivity, actor, relation)
	}
	payload := map[string]any{
		"work_product":     workProductCatalogResponse(product, nil),
		"linked_issue_ids": []string{},
		"detached":         true,
	}
	if actor.TaskID.Valid {
		h.publishTask(protocol.EventPullRequestUpdated, workspaceID, "agent", uuidToString(actor.ID), uuidToString(actor.TaskID), payload)
	} else {
		h.publish(protocol.EventPullRequestUpdated, workspaceID, "member", uuidToString(actor.ID), payload)
	}
	if issue, ok := h.loadWorkProductIssueForCompletion(r.Context(), issueID, workspaceUUID); ok {
		h.maybeCompleteWorkProductIssue(r.Context(), issue)
	}
	writeJSON(w, http.StatusOK, map[string]any{"detached": len(detached)})
}

// DetachWorkProductRelation preserves the relation-id handler used by older
// internal callers and fixtures. The canonical issue surface now detaches by
// work-product id so all live claims are retracted together, but removing this
// small adapter would break existing server-side callers without improving the
// new contract.
func (h *Handler) DetachWorkProductRelation(w http.ResponseWriter, r *http.Request) {
	workspaceID, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	issueID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "issue id")
	if !ok {
		return
	}
	relationID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "relationId"), "relation id")
	if !ok {
		return
	}
	actor, ok := h.resolveWorkProductRelationActor(w, r, workspaceID, workspaceUUID, issueID)
	if !ok {
		return
	}
	if h.Queries == nil {
		writeError(w, http.StatusInternalServerError, "database unavailable")
		return
	}
	if _, ok := h.requireWorkProductIssue(w, r, workspaceUUID, issueID); !ok {
		return
	}
	detachedByType := "user"
	if actor.Type == "agent" {
		detachedByType = "agent"
	}
	relation, err := h.Queries.DetachWorkProductRelationForIssue(r.Context(), db.DetachWorkProductRelationForIssueParams{
		ID:             relationID,
		WorkspaceID:    workspaceUUID,
		IssueID:        issueID,
		DetachedByType: pgtype.Text{String: detachedByType, Valid: true},
		DetachedByID:   actor.ID,
		DetachedTaskID: actor.TaskID,
		DetachedRunID:  actor.RunID,
	})
	if err != nil {
		if isNotFound(err) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "work product relation not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	h.recordWorkProductRelationActivity(r, workProductDetachedActivity, actor, relation)
	writeJSON(w, http.StatusOK, map[string]any{"relation": relation})
}

func (h *Handler) loadWorkProductIssueForCompletion(ctx context.Context, issueID, workspaceID pgtype.UUID) (db.Issue, bool) {
	if h.Queries == nil {
		return db.Issue{}, false
	}
	issue, err := h.Queries.GetIssueInWorkspace(ctx, db.GetIssueInWorkspaceParams{ID: issueID, WorkspaceID: workspaceID})
	return issue, err == nil
}

// requireWorkProductIssue resolves the path issue inside the workspace. Every
// issue-anchored Work Product surface goes through it so a caller cannot read
// or write another tenant's relations by supplying a foreign issue id.
func (h *Handler) requireWorkProductIssue(w http.ResponseWriter, r *http.Request, workspaceUUID, issueID pgtype.UUID) (pgtype.UUID, bool) {
	executor, ok := h.requireWorkProductDB(w)
	if !ok {
		return pgtype.UUID{}, false
	}
	var scoped pgtype.UUID
	if err := executor.QueryRow(r.Context(), `SELECT id FROM issue WHERE id = $1 AND workspace_id = $2`, issueID, workspaceUUID).Scan(&scoped); err != nil {
		if isNotFound(err) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "issue not found")
			return pgtype.UUID{}, false
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return pgtype.UUID{}, false
	}
	return scoped, true
}

// recordWorkProductRelationActivity writes the attach/detach trail. Unlike the
// agent-env reveal audit this is best-effort: the relation change is already
// committed and already self-describing on the row, so failing the request
// after the fact would report an error for work that did happen. The write
// failure is logged loudly instead.
func (h *Handler) recordWorkProductRelationActivity(r *http.Request, action string, actor workProductRelationActor, relation db.WorkProductRelation) {
	if h.Queries == nil {
		return
	}
	details, err := json.Marshal(map[string]any{
		"relation_id":     uuidToString(relation.ID),
		"work_product_id": uuidToString(relation.WorkProductID),
		"relation_source": relation.RelationSource,
		"close_intent":    relation.CloseIntent,
		"task_id":         workProductUUIDPtr(actor.TaskID),
		"run_id":          workProductUUIDPtr(actor.RunID),
	})
	if err != nil {
		return
	}
	// actor.Type is already the activity_log vocabulary ("member" / "agent"),
	// which differs from the relation table's ("user" / "agent") by one word.
	if _, err := h.Queries.CreateActivity(r.Context(), db.CreateActivityParams{
		ID:          dbid.NewV7(),
		WorkspaceID: relation.WorkspaceID,
		IssueID:     relation.IssueID,
		ActorType:   pgtype.Text{String: actor.Type, Valid: actor.Type != ""},
		ActorID:     actor.ID,
		Action:      action,
		Details:     details,
	}); err != nil {
		slog.Error("work product relation audit write failed",
			append(logger.RequestAttrs(r), "error", err, "action", action,
				"relation_id", uuidToString(relation.ID))...)
	}
}
