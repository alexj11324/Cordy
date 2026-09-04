package handler

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"strconv"
	"strings"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/vcs"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

// ── Response mappers ────────────────────────────────────────────────────────

// vcsPullRequestToResponse maps a stored VCS PR onto the shared PR response
// shape for single-PR webhook broadcasts (no aggregated check counts; the
// frontend re-queries the issue's PR list for fresh counts).
func vcsPullRequestToResponse(p db.VcsPullRequest) GitHubPullRequestResponse {
	return GitHubPullRequestResponse{
		ID:               uuidToString(p.ID),
		Provider:         p.Provider,
		WorkspaceID:      uuidToString(p.WorkspaceID),
		RepoOwner:        p.RepoOwner,
		RepoName:         p.RepoName,
		Number:           p.PrNumber,
		Title:            p.Title,
		State:            p.State,
		HtmlURL:          p.HtmlUrl,
		Branch:           textToPtr(p.Branch),
		AuthorLogin:      textToPtr(p.AuthorLogin),
		AuthorAvatarURL:  textToPtr(p.AuthorAvatarUrl),
		MergedAt:         timestampToPtr(p.MergedAt),
		ClosedAt:         timestampToPtr(p.ClosedAt),
		PRCreatedAt:      timestampToString(p.PrCreatedAt),
		PRUpdatedAt:      timestampToString(p.PrUpdatedAt),
		MergeableState:   nil,
		ChecksConclusion: nil,
		Additions:        p.Additions,
		Deletions:        p.Deletions,
		ChangedFiles:     p.ChangedFiles,
	}
}

func vcsWorkProductPullRequestToResponse(p db.GetVCSPullRequestForWorkProductRow) GitHubPullRequestResponse {
	return GitHubPullRequestResponse{
		ID:               uuidToString(p.ID),
		Provider:         p.Provider,
		WorkspaceID:      uuidToString(p.WorkspaceID),
		RepoOwner:        p.RepoOwner,
		RepoName:         p.RepoName,
		Number:           p.PrNumber,
		Title:            p.Title,
		State:            p.State,
		HtmlURL:          p.HtmlUrl,
		Branch:           textToPtr(p.Branch),
		AuthorLogin:      textToPtr(p.AuthorLogin),
		AuthorAvatarURL:  textToPtr(p.AuthorAvatarUrl),
		MergedAt:         timestampToPtr(p.MergedAt),
		ClosedAt:         timestampToPtr(p.ClosedAt),
		PRCreatedAt:      timestampToString(p.PrCreatedAt),
		PRUpdatedAt:      timestampToString(p.PrUpdatedAt),
		MergeableState:   nil,
		ChecksConclusion: aggregateChecksConclusion(p.ChecksFailed, p.ChecksPassed, p.ChecksPending, p.ChecksTotal),
		ChecksTotal:      p.ChecksTotal,
		ChecksPassed:     p.ChecksPassed,
		ChecksFailed:     p.ChecksFailed,
		ChecksPending:    p.ChecksPending,
		ChecksRunning:    p.ChecksPending,
		FailedCheckNames: []string{},
		Additions:        p.Additions,
		Deletions:        p.Deletions,
		ChangedFiles:     p.ChangedFiles,
	}
}

// ── Webhook ─────────────────────────────────────────────────────────────────

// HandleVCSWebhook (POST /api/webhooks/vcs/{connectionId}) authenticates and
// mirrors webhooks from any token-based Git provider. The connection id in the path
// selects the workspace, the provider, and the decryption secret; the provider
// adapter handles the provider-specific signature scheme, event header, and
// payload shape, returning normalized events to the shared mirror logic below.
func (h *Handler) HandleVCSWebhook(w http.ResponseWriter, r *http.Request) {
	// Where the integration is off (the managed cloud) the endpoint behaves as
	// if it does not exist — a bare 404 that reveals nothing about config, the
	// same response a genuinely unknown connection id gets below.
	if !h.isVCSAvailable() {
		writeError(w, http.StatusNotFound, "unknown connection")
		return
	}
	if !h.isVCSConfigured() {
		writeError(w, http.StatusServiceUnavailable, "vcs webhooks not configured")
		return
	}
	connUUID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "connectionId"), "connection id")
	if !ok {
		return
	}
	body, err := io.ReadAll(io.LimitReader(r.Body, 10<<20)) // 10 MiB cap
	if err != nil {
		writeError(w, http.StatusBadRequest, "read body failed")
		return
	}

	conn, err := h.Queries.GetVCSConnectionByID(r.Context(), connUUID)
	if err != nil {
		if !errors.Is(err, pgx.ErrNoRows) {
			slog.Warn("vcs: lookup connection failed", "err", err)
		}
		writeError(w, http.StatusNotFound, "unknown connection")
		return
	}
	provider, ok := vcs.For(conn.Provider)
	if !ok {
		slog.Error("vcs: connection has unknown provider", "provider", conn.Provider)
		writeError(w, http.StatusInternalServerError, "unknown provider")
		return
	}

	secret, err := h.openVCSSecret(conn.WebhookSecretEncrypted)
	if err != nil {
		slog.Error("vcs: decrypt webhook secret failed", "err", err)
		writeError(w, http.StatusInternalServerError, "secret error")
		return
	}
	if !provider.VerifySignature(secret, r.Header, body) {
		writeError(w, http.StatusUnauthorized, "invalid signature")
		return
	}

	switch provider.EventKind(r.Header) {
	case vcs.EventPullRequest:
		if pr, err := provider.ParsePullRequest(body); err != nil {
			slog.Warn("vcs: bad pull_request payload", "provider", conn.Provider, "err", err)
		} else {
			h.mirrorVCSPullRequest(r.Context(), conn, pr)
		}
	case vcs.EventCIStatus:
		if st, err := provider.ParseCIStatus(body); err != nil {
			slog.Warn("vcs: bad status payload", "provider", conn.Provider, "err", err)
		} else {
			h.mirrorVCSCIStatus(r.Context(), conn, st)
		}
	default:
		// Acknowledge unmodelled events so the provider doesn't flag the hook.
	}
	w.WriteHeader(http.StatusAccepted)
}

func (h *Handler) mirrorVCSPullRequest(ctx context.Context, conn db.VcsConnection, ev vcs.PullRequestEvent) {
	if ev.RepoOwner == "" || ev.RepoName == "" || ev.Number == 0 {
		slog.Warn("vcs: pull_request missing repo identity", "provider", conn.Provider)
		return
	}

	if h.Queries == nil || h.TxStarter == nil {
		return
	}
	tx, err := h.TxStarter.Begin(ctx)
	if err != nil {
		slog.Warn("vcs: begin pull request mirror transaction failed", "err", err)
		return
	}
	rollback := func() { _ = tx.Rollback(ctx) }
	queries := h.Queries.WithTx(tx)
	if _, err := tx.Exec(ctx, `SELECT id FROM vcs_connection WHERE id = $1 AND workspace_id = $2 FOR UPDATE`, conn.ID, conn.WorkspaceID); err != nil {
		rollback()
		return
	}
	pr, err := queries.UpsertVCSPullRequest(ctx, db.UpsertVCSPullRequestParams{
		WorkspaceID:     conn.WorkspaceID,
		ConnectionID:    conn.ID,
		Provider:        conn.Provider,
		RepoOwner:       ev.RepoOwner,
		RepoName:        ev.RepoName,
		PrNumber:        ev.Number,
		Title:           ev.Title,
		State:           ev.State,
		HtmlUrl:         ev.HTMLURL,
		Branch:          ptrToText(strPtrOrNil(ev.Branch)),
		AuthorLogin:     ptrToText(strPtrOrNil(ev.AuthorLogin)),
		AuthorAvatarUrl: ptrToText(strPtrOrNil(ev.AuthorAvatarURL)),
		MergedAt:        parseGHTime(ev.MergedAt),
		ClosedAt:        parseGHTime(ev.ClosedAt),
		PrCreatedAt:     parseGHTimeRequired(ev.CreatedAt),
		PrUpdatedAt:     parseGHTimeRequired(ev.UpdatedAt),
		Additions:       ev.Additions,
		Deletions:       ev.Deletions,
		ChangedFiles:    ev.ChangedFiles,
		HeadSha:         ev.HeadSHA,
	})
	if err != nil {
		rollback()
		slog.Warn("vcs: upsert pr failed", "err", err)
		return
	}

	// UpsertVCSPullRequest keeps newer metadata on stale redelivery. Do not emit
	// an older provider snapshot or re-run any association side effect.
	evUpdatedAt := parseGHTimeRequired(ev.UpdatedAt)
	if pr.PrUpdatedAt.Valid && evUpdatedAt.Valid && pr.PrUpdatedAt.Time.After(evUpdatedAt.Time) {
		rollback()
		return
	}
	product, err := queries.CreateWorkProduct(ctx, db.CreateWorkProductParams{
		WorkspaceID:        conn.WorkspaceID,
		Kind:               "pull_request",
		Provider:           conn.Provider,
		ExternalIdentity:   uuidToString(conn.ID) + ":" + strings.ToLower(ev.RepoOwner) + "/" + strings.ToLower(ev.RepoName) + "#" + strconv.Itoa(int(ev.Number)),
		ExternalUrl:        pgtype.Text{String: ev.HTMLURL, Valid: ev.HTMLURL != ""},
		ProviderRecordType: pgtype.Text{String: "vcs_pull_request", Valid: true},
		ProviderRecordID:   pr.ID,
	})
	if err != nil {
		rollback()
		slog.Warn("vcs: upsert work product failed", "err", err)
		return
	}
	reevalIssues := make([]db.Issue, 0)
	idents := extractIdentifiers(ev.Title, ev.Body, ev.Branch)
	closingIdents := map[string]struct{}{}
	for _, identifier := range extractClosingIdentifiers(ev.Title, ev.Body) {
		closingIdents[identifier] = struct{}{}
	}
	qualifyingIdents := map[string]struct{}{}
	for _, identifier := range extractIdentifiers(ev.Title, ev.Branch) {
		qualifyingIdents[identifier] = struct{}{}
	}
	for identifier := range closingIdents {
		qualifyingIdents[identifier] = struct{}{}
	}
	preserveCloseIntent := !ev.Terminal() && (ev.State == "merged" || ev.State == "closed")
	prefix := h.getIssuePrefix(ctx, conn.WorkspaceID)
	for _, identifier := range idents {
		issue, ok := h.lookupIssueByIdentifier(ctx, conn.WorkspaceID, prefix, identifier)
		if !ok {
			continue
		}
		_, declared := closingIdents[identifier]
		_, qualifies := qualifyingIdents[identifier]
		if err := queries.LinkIssueToVCSPullRequest(ctx, db.LinkIssueToVCSPullRequestParams{
			IssueID:             issue.ID,
			PullRequestID:       pr.ID,
			CloseIntent:         declared && !preserveCloseIntent,
			MentionOnly:         !qualifies,
			PreserveCloseIntent: preserveCloseIntent,
		}); err != nil {
			rollback()
			slog.Warn("vcs: link failed", "err", err)
			return
		}
		reevalIssues = append(reevalIssues, issue)
	}
	issueIDs, err := queries.ListIssueIDsForWorkProduct(ctx, db.ListIssueIDsForWorkProductParams{WorkspaceID: conn.WorkspaceID, WorkProductID: product.ID})
	if err != nil {
		rollback()
		slog.Warn("vcs: list linked issues failed", "err", err)
		return
	}
	if err := tx.Commit(ctx); err != nil {
		rollback()
		slog.Warn("vcs: commit pull request mirror failed", "err", err)
		return
	}
	linkedIssueIDs := make([]string, 0, len(issueIDs))
	for _, issueID := range issueIDs {
		linkedIssueIDs = append(linkedIssueIDs, uuidToString(issueID))
	}
	if ev.State == "merged" || ev.State == "closed" {
		for _, issue := range reevalIssues {
			h.maybeCompleteWorkProductIssue(ctx, issue)
		}
	}
	workspaceID := uuidToString(conn.WorkspaceID)
	resp := vcsPullRequestToResponse(pr)
	h.publish(protocol.EventPullRequestUpdated, workspaceID, "system", "", map[string]any{
		"pull_request":     resp,
		"linked_issue_ids": linkedIssueIDs,
	})
}

func (h *Handler) mirrorVCSCIStatus(ctx context.Context, conn db.VcsConnection, ev vcs.CIStatusEvent) {
	if ev.SHA == "" || ev.State == "" {
		return
	}
	if h.Queries == nil || h.TxStarter == nil {
		return
	}
	// Use the provider's own event timestamp so UpsertVCSCommitStatus's
	// monotonic guard has something real to compare — writing time.Now() here
	// made the guard always true, so an out-of-order redelivery could regress a
	// status. Falls back to now() only when the payload carried no timestamp.
	tx, err := h.TxStarter.Begin(ctx)
	if err != nil {
		slog.Warn("vcs: begin status mirror transaction failed", "err", err)
		return
	}
	rollback := func() { _ = tx.Rollback(ctx) }
	queries := h.Queries.WithTx(tx)
	if err := queries.UpsertVCSCommitStatus(ctx, db.UpsertVCSCommitStatusParams{
		ConnectionID: conn.ID,
		Sha:          ev.SHA,
		Context:      ev.Context,
		State:        ev.State,
		TargetUrl:    ptrToText(strPtrOrNil(ev.TargetURL)),
		Description:  ptrToText(strPtrOrNil(ev.Description)),
		UpdatedAt:    parseGHTimeRequired(ev.UpdatedAt),
	}); err != nil {
		rollback()
		slog.Warn("vcs: upsert commit status failed", "err", err)
		return
	}

	issueIDs, err := queries.ListIssueIDsForVCSPRHead(ctx, db.ListIssueIDsForVCSPRHeadParams{
		ConnectionID: conn.ID,
		HeadSha:      ev.SHA,
	})
	if err != nil {
		rollback()
		slog.Warn("vcs: lookup issues for status failed", "err", err)
		return
	}
	if err := tx.Commit(ctx); err != nil {
		rollback()
		slog.Warn("vcs: commit status mirror failed", "err", err)
		return
	}
	workspaceID := uuidToString(conn.WorkspaceID)
	for _, issueID := range issueIDs {
		h.publish(protocol.EventPullRequestUpdated, workspaceID, "system", "", map[string]any{
			"issue_id": uuidToString(issueID),
		})
	}
}
