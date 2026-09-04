package handler

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/ghsnapshot"
	"github.com/patchbay-ai/patchbay/server/internal/issuestatus"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

var githubPullRequestURL = regexp.MustCompile(`(?i)^https?://github\.com/([A-Za-z0-9][A-Za-z0-9._-]*)/([A-Za-z0-9][A-Za-z0-9._-]*)/pull/([0-9]+)(?:[/?#].*)?$`)

type issuePullRequestAttachRequest struct {
	URL            string  `json:"url"`
	Title          *string `json:"title"`
	State          *string `json:"state"`
	Branch         *string `json:"branch"`
	HeadRefName    *string `json:"head_ref_name"`
	HeadSHA        *string `json:"head_sha"`
	AuthorLogin    *string `json:"author_login"`
	CloseIntent    bool    `json:"close_intent"`
}

// parseGitHubPRURL is intentionally narrower than a generic URL parser. The
// provider identity is the canonical owner/repository/number tuple used by
// the mirror's idempotency key; no issue identifier can be extracted from the
// URL or from a provider object's text.
func parseGitHubPRURL(raw string) (string, string, int32, error) {
	matches := githubPullRequestURL.FindStringSubmatch(strings.TrimSpace(raw))
	if len(matches) != 4 {
		return "", "", 0, fmt.Errorf("not a GitHub pull request URL")
	}
	number, err := strconv.ParseInt(matches[3], 10, 32)
	if err != nil || number <= 0 {
		return "", "", 0, fmt.Errorf("invalid pull request number")
	}
	return strings.ToLower(matches[1]), strings.ToLower(matches[2]), int32(number), nil
}

func normalizePullRequestAttachState(raw string) (string, error) {
	state := strings.ToLower(strings.TrimSpace(raw))
	if state == "" {
		return "open", nil
	}
	switch state {
	case "open", "closed", "merged", "draft":
		return state, nil
	default:
		return "", errors.New("invalid pull request state")
	}
}

func optionalAttachText(value *string) string {
	if value == nil {
		return ""
	}
	return strings.TrimSpace(*value)
}

func attachTimestamp(value time.Time, fallback time.Time) pgtype.Timestamptz {
	if value.IsZero() {
		value = fallback
	}
	return pgtype.Timestamptz{Time: value.UTC(), Valid: true}
}

func (h *Handler) resolveWorkProductIssue(w http.ResponseWriter, r *http.Request, workspaceUUID pgtype.UUID) (db.Issue, bool) {
	if h.Queries == nil {
		writeError(w, http.StatusInternalServerError, "database unavailable")
		return db.Issue{}, false
	}
	raw := strings.TrimSpace(chi.URLParam(r, "id"))
	if issue, ok := h.resolveIssueByIdentifier(r.Context(), raw, uuidToString(workspaceUUID)); ok {
		return issue, true
	}
	issueID, err := parseStrictUUID(raw)
	if err != nil {
		writeErrorCode(w, http.StatusNotFound, "not_found", "issue not found")
		return db.Issue{}, false
	}
	issue, err := h.Queries.GetIssueInWorkspace(r.Context(), db.GetIssueInWorkspaceParams{
		ID:          issueID,
		WorkspaceID: workspaceUUID,
	})
	if err != nil {
		if isNotFound(err) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "issue not found")
			return db.Issue{}, false
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return db.Issue{}, false
	}
	return issue, true
}

func (h *Handler) githubPullRequestMetadataForAttach(ctx context.Context, q *db.Queries, workspaceID pgtype.UUID, owner, repo string, number int32) (int64, *ghsnapshot.PullRequestMetadata, error) {
	if h.PRRefresh == nil || !h.PRRefresh.Enabled() {
		return 0, nil, nil
	}
	installations, err := q.ListGitHubInstallationsByWorkspace(ctx, workspaceID)
	if err != nil {
		return 0, nil, err
	}
	for _, installation := range installations {
		metadata, lookupErr := h.PRRefresh.PullRequestMetadata(ctx, installation.InstallationID, owner, repo, number)
		if lookupErr == nil {
			return installation.InstallationID, &metadata, nil
		}
	}
	// A human can register a provider URL while the App API is unavailable;
	// the provider facts then remain request fallbacks. A task cannot use this
	// branch because task ownership validation below requires verified metadata.
	return 0, nil, nil
}

func (h *Handler) validateTaskExplicitGitHubProduct(ctx context.Context, q *db.Queries, workspaceID, taskID pgtype.UUID, repoIdentity string, metadata ghsnapshot.PullRequestMetadata) error {
	provenances, err := q.ListExecutionProvenanceByTask(ctx, db.ListExecutionProvenanceByTaskParams{
		WorkspaceID: workspaceID,
		TaskID:      taskID,
	})
	if err != nil || len(provenances) == 0 {
		return errors.New("execution provenance unavailable")
	}
	task, err := q.GetAgentTaskInWorkspace(ctx, db.GetAgentTaskInWorkspaceParams{ID: taskID, WorkspaceID: workspaceID})
	if err != nil {
		return errors.New("task unavailable")
	}
	workspace, err := q.GetWorkspace(ctx, workspaceID)
	if err != nil {
		return errors.New("workspace unavailable")
	}
	if !headRepositoryMatches(metadata.HeadRepoIdentity, repoIdentity) {
		return errors.New("provider head repository mismatch")
	}
	runtime := NewWorkProductDiscoveryRuntime(h)
	authorized, err := runtime.taskRepositoryAuthorized(ctx, task, workspace, repoIdentity)
	if err != nil {
		return errors.New("task repository authorization unavailable")
	}
	if !authorized {
		return errors.New("repository is not authorized for workspace")
	}
	for _, provenance := range provenances {
		if provenance.HeadState != "attached" || !headRepositoryMatches(provenance.RepoIdentity, repoIdentity) {
			continue
		}
		executionWorkspace := textWorkProductValue(provenance.ExecutionWorkspace)
		if executionWorkspace == "" || !taskExecutionWorkspaceMatches(textWorkProductValue(task.WorkDir), textWorkProductValue(task.DurableWorkDir), executionWorkspace) {
			continue
		}
		if !provenance.HeadBranch.Valid || provenance.HeadBranch.String == "" || provenance.HeadBranch.String != metadata.Branch {
			continue
		}
		if !provenance.HeadSha.Valid || provenance.HeadSha.String == "" || !strings.EqualFold(provenance.HeadSha.String, metadata.HeadSHA) {
			continue
		}
		return nil
	}
	return errors.New("provider repository, branch, or head does not match execution provenance")
}

func (h *Handler) AttachIssuePullRequest(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	issue, ok := h.resolveWorkProductIssue(w, r, workspaceUUID)
	if !ok {
		return
	}
	var request issuePullRequestAttachRequest
	if err := decodeWorkProductJSON(r, &request); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	owner, repo, number, err := parseGitHubPRURL(request.URL)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	requestedState, err := normalizePullRequestAttachState(optionalAttachText(request.State))
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	workspaceID := uuidToString(workspaceUUID)
	actor, ok := h.resolveWorkProductRelationActor(w, r, workspaceID, workspaceUUID, issue.ID)
	if !ok {
		return
	}
	if h.TxStarter == nil || h.Queries == nil {
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
	if err := tx.QueryRow(r.Context(), `SELECT id FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE`, issue.ID, workspaceUUID).Scan(&lockedIssue); err != nil {
		rollback()
		if isNotFound(err) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "issue not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	if actor.TaskID.Valid {
		if _, err := tx.Exec(r.Context(), `SELECT pg_advisory_xact_lock(hashtextextended($1::text || ':' || $2::text, 0))`, workspaceUUID, actor.TaskID); err != nil {
			rollback()
			writeError(w, http.StatusInternalServerError, "database error")
			return
		}
	}
	queries := h.Queries.WithTx(tx)
	installationID, metadata, err := h.githubPullRequestMetadataForAttach(r.Context(), queries, workspaceUUID, owner, repo, number)
	if err != nil {
		rollback()
		writeError(w, http.StatusInternalServerError, "failed to load provider metadata")
		return
	}
	if actor.TaskID.Valid {
		if metadata == nil {
			rollback()
			writeError(w, http.StatusForbidden, "task attach requires provider metadata for execution ownership")
			return
		}
		if err := h.validateTaskExplicitGitHubProduct(r.Context(), queries, workspaceUUID, actor.TaskID, owner+"/"+repo, *metadata); err != nil {
			rollback()
			writeError(w, http.StatusForbidden, "pull request is not owned by this task execution")
			return
		}
	}

	now := time.Now().UTC()
	title := owner + "/" + repo + "#" + strconv.Itoa(int(number))
	state := requestedState
	branch := optionalAttachText(request.Branch)
	if branch == "" {
		branch = optionalAttachText(request.HeadRefName)
	}
	headSHA := optionalAttachText(request.HeadSHA)
	authorLogin := optionalAttachText(request.AuthorLogin)
	createdAt := now
	updatedAt := now
	var mergedAt, closedAt pgtype.Timestamptz
	additions, deletions, changedFiles := int32(0), int32(0), int32(0)
	if metadata != nil {
		if metadata.Title != "" {
			title = metadata.Title
		}
		state = metadata.State
		branch = metadata.Branch
		headSHA = metadata.HeadSHA
		authorLogin = metadata.AuthorLogin
		createdAt = metadata.CreatedAt
		updatedAt = metadata.UpdatedAt
		mergedAt = workProductOptionalTimestamp(metadata.MergedAt)
		closedAt = workProductOptionalTimestamp(metadata.ClosedAt)
		additions, deletions, changedFiles = metadata.Additions, metadata.Deletions, metadata.ChangedFiles
	}
	canonicalURL := "https://github.com/" + owner + "/" + repo + "/pull/" + strconv.Itoa(int(number))
	wasNew := true
	if _, err := queries.GetGitHubPullRequest(r.Context(), db.GetGitHubPullRequestParams{
		WorkspaceID: workspaceUUID,
		RepoOwner:   owner,
		RepoName:    repo,
		PrNumber:    number,
	}); err == nil {
		wasNew = false
	} else if !isNotFound(err) {
		rollback()
		writeError(w, http.StatusInternalServerError, "failed to read pull request")
		return
	}
	pullRequest, err := queries.UpsertGitHubPullRequest(r.Context(), db.UpsertGitHubPullRequestParams{
		WorkspaceID:         workspaceUUID,
		InstallationID:      installationID,
		RepoOwner:           owner,
		RepoName:            repo,
		PrNumber:            number,
		Title:               title,
		State:               state,
		HtmlUrl:             canonicalURL,
		PrCreatedAt:         attachTimestamp(createdAt, now),
		PrUpdatedAt:         attachTimestamp(updatedAt, now),
		HeadSha:             headSHA,
		Additions:           additions,
		Deletions:           deletions,
		ChangedFiles:        changedFiles,
		Branch:              pgtype.Text{String: branch, Valid: branch != ""},
		AuthorLogin:         pgtype.Text{String: authorLogin, Valid: authorLogin != ""},
		AuthorAvatarUrl:     pgtype.Text{String: metadataString(metadata, func(m ghsnapshot.PullRequestMetadata) string { return m.AuthorAvatarURL }), Valid: metadata != nil && metadata.AuthorAvatarURL != ""},
		MergedAt:            mergedAt,
		ClosedAt:            closedAt,
		ClearMergeableState: pgtype.Bool{Bool: false, Valid: true},
	})
	if err != nil {
		rollback()
		writeError(w, http.StatusInternalServerError, "failed to attach pull request")
		return
	}
	product, err := queries.CreateWorkProduct(r.Context(), db.CreateWorkProductParams{
		WorkspaceID:        workspaceUUID,
		Kind:               "pull_request",
		Provider:           "github",
		ExternalIdentity:   strings.ToLower(owner+"/"+repo) + "#" + strconv.Itoa(int(number)),
		ExternalUrl:        pgtype.Text{String: canonicalURL, Valid: true},
		ProviderRecordType: pgtype.Text{String: "github_pull_request", Valid: true},
		ProviderRecordID:   pullRequest.ID,
	})
	if err != nil {
		rollback()
		writeError(w, http.StatusInternalServerError, "failed to register work product")
		return
	}
	attachedByType := "user"
	relationSource := workProductRelationSourceManual
	if actor.TaskID.Valid {
		attachedByType = "agent"
		relationSource = workProductRelationSourceTask
	}
	relation, err := queries.CreateWorkProductRelation(r.Context(), db.CreateWorkProductRelationParams{
		WorkspaceID:    workspaceUUID,
		WorkProductID:  product.ID,
		RelationKey:    workProductRelationKey(issue.ID, actor.TaskID, actor.RunID),
		RelationSource: relationSource,
		AttachedByType: attachedByType,
		AttachedByID:   actor.ID,
		IssueID:        issue.ID,
		TaskID:         actor.TaskID,
		RunID:          actor.RunID,
		CloseIntent:    request.CloseIntent,
	})
	if err != nil {
		rollback()
		writeError(w, http.StatusInternalServerError, "failed to attach work product")
		return
	}
	if err := tx.Commit(r.Context()); err != nil {
		rollback()
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	response := githubPullRequestToResponse(pullRequest, h.PRRefresh != nil && h.PRRefresh.Enabled())
	payload := map[string]any{
		"pull_request":     response,
		"linked_issue_ids": []string{uuidToString(issue.ID)},
		"work_product":     workProductCatalogResponse(product, &relation),
		"relation":         workProductRelationResponse(relation),
	}
	if actor.TaskID.Valid {
		h.publishTask(protocol.EventPullRequestUpdated, workspaceID, "agent", uuidToString(actor.ID), uuidToString(actor.TaskID), payload)
	} else {
		h.publish(protocol.EventPullRequestUpdated, workspaceID, "member", uuidToString(actor.ID), payload)
	}
	if request.CloseIntent && metadata != nil && metadata.State == "merged" {
		h.maybeCompleteWorkProductIssue(r.Context(), issue)
	}
	status := http.StatusOK
	if wasNew {
		status = http.StatusCreated
	}
	writeJSON(w, status, map[string]any{
		"pull_request": response,
		"work_product": workProductCatalogResponse(product, &relation),
		"relation":     workProductRelationResponse(relation),
	})
}

func metadataString(metadata *ghsnapshot.PullRequestMetadata, value func(ghsnapshot.PullRequestMetadata) string) string {
	if metadata == nil {
		return ""
	}
	return value(*metadata)
}

func (h *Handler) ListIssuePullRequests(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	issue, ok := h.resolveWorkProductIssue(w, r, workspaceUUID)
	if !ok {
		return
	}
	if h.Queries == nil {
		writeError(w, http.StatusInternalServerError, "database unavailable")
		return
	}
	rows, err := h.Queries.ListWorkProductsByIssue(r.Context(), db.ListWorkProductsByIssueParams{
		WorkspaceID: workspaceUUID,
		IssueID:     issue.ID,
		Limit:       workProductMaxPage,
		Offset:      0,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to list pull requests")
		return
	}
	snapshotEnabled := h.PRRefresh != nil && h.PRRefresh.Enabled()
	pullRequests := make([]GitHubPullRequestResponse, 0, len(rows))
	for _, row := range rows {
		if !row.ProviderRecordType.Valid || !row.ProviderRecordID.Valid {
			continue
		}
		switch row.ProviderRecordType.String {
		case "github_pull_request":
			pr, getErr := h.Queries.GetGitHubPullRequestForWorkProduct(r.Context(), row.ProviderRecordID)
			if getErr != nil {
				continue
			}
			if h.PRRefresh != nil {
				h.PRRefresh.MaybeEnqueueOnView(pr.InstallationID, pr.RepoOwner, pr.RepoName, pr.PrNumber, pr.SnapshotFetchedAt.Time, pr.SnapshotFetchedAt.Valid && pr.SnapshotHeadSha != "" && pr.SnapshotHeadSha == pr.HeadSha)
			}
			pullRequests = append(pullRequests, githubWorkProductPullRequestToResponse(pr, snapshotEnabled))
		case "vcs_pull_request":
			pr, getErr := h.Queries.GetVCSPullRequestForWorkProduct(r.Context(), row.ProviderRecordID)
			if getErr == nil {
				pullRequests = append(pullRequests, vcsWorkProductPullRequestToResponse(pr))
			}
		}
	}
	sort.SliceStable(pullRequests, func(i, j int) bool {
		return pullRequests[i].PRCreatedAt > pullRequests[j].PRCreatedAt
	})
	writeJSON(w, http.StatusOK, map[string]any{"pull_requests": pullRequests})
}

func (h *Handler) AttachExistingWorkProduct(w http.ResponseWriter, r *http.Request) {
	workspaceID, workspaceUUID, ok := h.workProductWorkspace(w, r)
	if !ok {
		return
	}
	issue, ok := h.resolveWorkProductIssue(w, r, workspaceUUID)
	if !ok {
		return
	}
	var request struct {
		WorkProductID string `json:"work_product_id"`
		CloseIntent   bool   `json:"close_intent"`
	}
	if err := decodeWorkProductJSON(r, &request); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	productID, ok := parseUUIDOrBadRequest(w, request.WorkProductID, "work product id")
	if !ok {
		return
	}
	actor, ok := h.resolveWorkProductRelationActor(w, r, workspaceID, workspaceUUID, issue.ID)
	if !ok {
		return
	}
	if h.TxStarter == nil || h.Queries == nil {
		writeError(w, http.StatusInternalServerError, "database unavailable")
		return
	}
	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	rollback := func() { _ = tx.Rollback(r.Context()) }
	var lockedProduct pgtype.UUID
	if err := tx.QueryRow(r.Context(), `SELECT id FROM work_product WHERE id = $1 AND workspace_id = $2 FOR UPDATE`, productID, workspaceUUID).Scan(&lockedProduct); err != nil {
		rollback()
		if isNotFound(err) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "work product not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	if actor.TaskID.Valid {
		if _, err := tx.Exec(r.Context(), `SELECT pg_advisory_xact_lock(hashtextextended($1::text || ':' || $2::text, 0))`, workspaceUUID, actor.TaskID); err != nil {
			rollback()
			writeError(w, http.StatusInternalServerError, "database error")
			return
		}
		relations, listErr := h.Queries.WithTx(tx).ListWorkProductRelationsByTask(r.Context(), db.ListWorkProductRelationsByTaskParams{WorkspaceID: workspaceUUID, TaskID: actor.TaskID})
		if listErr != nil {
			rollback()
			writeError(w, http.StatusInternalServerError, "failed to verify task work product")
			return
		}
		owned := false
		for _, relation := range relations {
			if sameWorkProductUUID(relation.WorkProductID, productID) && sameWorkProductUUID(relation.IssueID, issue.ID) && sameWorkProductUUID(relation.TaskID, actor.TaskID) && sameWorkProductUUID(relation.RunID, actor.RunID) && isExplicitWorkProductRelationSource(relation.RelationSource) {
				owned = true
				break
			}
		}
		if !owned {
			rollback()
			writeError(w, http.StatusForbidden, "task execution cannot attach an unowned work product")
			return
		}
	}
	queries := h.Queries.WithTx(tx)
	attachedByType := "user"
	relationSource := workProductRelationSourceManual
	if actor.TaskID.Valid {
		attachedByType = "agent"
		relationSource = workProductRelationSourceTask
	}
	relation, err := queries.CreateWorkProductRelation(r.Context(), db.CreateWorkProductRelationParams{
		WorkspaceID:    workspaceUUID,
		WorkProductID:  productID,
		RelationKey:    workProductRelationKey(issue.ID, actor.TaskID, actor.RunID),
		RelationSource: relationSource,
		AttachedByType: attachedByType,
		AttachedByID:   actor.ID,
		IssueID:        issue.ID,
		TaskID:         actor.TaskID,
		RunID:          actor.RunID,
		CloseIntent:    request.CloseIntent,
	})
	if err != nil {
		rollback()
		writeError(w, http.StatusInternalServerError, "failed to attach work product")
		return
	}
	product, err := queries.GetWorkProductByID(r.Context(), db.GetWorkProductByIDParams{ID: productID, WorkspaceID: workspaceUUID})
	if err != nil {
		rollback()
		writeError(w, http.StatusInternalServerError, "failed to load work product")
		return
	}
	if err := tx.Commit(r.Context()); err != nil {
		rollback()
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	h.recordWorkProductRelationActivity(r, workProductAttachedActivity, actor, relation)
	payload := map[string]any{
		"work_product":     workProductCatalogResponse(product, &relation),
		"relation":         workProductRelationResponse(relation),
		"linked_issue_ids": []string{uuidToString(issue.ID)},
	}
	if actor.TaskID.Valid {
		h.publishTask(protocol.EventPullRequestUpdated, workspaceID, "agent", uuidToString(actor.ID), uuidToString(actor.TaskID), payload)
	} else {
		h.publish(protocol.EventPullRequestUpdated, workspaceID, "member", uuidToString(actor.ID), payload)
	}
	if request.CloseIntent {
		h.maybeCompleteWorkProductIssue(r.Context(), issue)
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"work_product": workProductCatalogResponse(product, &relation),
		"relation":     workProductRelationResponse(relation),
	})
}

func (h *Handler) maybeCompleteWorkProductIssue(ctx context.Context, issue db.Issue) {
	if h.Queries == nil || issuestatus.Effective(ctx, h.Queries, issue.WorkspaceID, issue.Status) == "done" || issuestatus.Effective(ctx, h.Queries, issue.WorkspaceID, issue.Status) == "cancelled" {
		return
	}
	counts, err := h.Queries.GetIssueCombinedPullRequestCloseAggregate(ctx, issue.ID)
	if err != nil || counts.OpenCount != 0 || counts.MergedWithCloseIntentCount == 0 {
		return
	}
	h.advanceIssueToDone(ctx, issue, uuidToString(issue.WorkspaceID))
}
