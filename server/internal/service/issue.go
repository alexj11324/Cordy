package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/orvilo-ai/orvilo/server/internal/analytics"
	"github.com/orvilo-ai/orvilo/server/internal/dispatch"
	"github.com/orvilo-ai/orvilo/server/internal/entitlement"
	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/issueguard"
	"github.com/orvilo-ai/orvilo/server/internal/issueposition"
	"github.com/orvilo-ai/orvilo/server/internal/issuestatus"
	obsmetrics "github.com/orvilo-ai/orvilo/server/internal/metrics"
	"github.com/orvilo-ai/orvilo/server/internal/util"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/dbid"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

// IssueService is the single service-layer entry point for creating issues.
// Both the HTTP `POST /issues` handler and the future Lark `/issue` command
// call into Create so that duplicate guard, issue numbering, attachment
// linking, broadcast, analytics, and agent/team enqueue stay aligned. The
// service deliberately does NOT depend on http.Request — callers parse
// their own transport and pass a fully-resolved IssueCreateParams.
type IssueService struct {
	Queries   *db.Queries
	TxStarter TxStarter
	Bus       *events.Bus
	Analytics analytics.Client
	// Metrics is the shared business-metrics collector. Wired by
	// cmd/server/router.go after construction; nil in tests / self-hosted
	// without the metrics listener — obsmetrics.RecordEvent treats a nil
	// Metrics as "PostHog only", so leaving it unset is safe.
	Metrics     *obsmetrics.BusinessMetrics
	TaskService *TaskService
	// Entitlements supplies Cloud's effective issue-count instruction. Nil is
	// the self-hosted unlimited path.
	Entitlements entitlement.Provider
}

func NewIssueService(q *db.Queries, tx TxStarter, bus *events.Bus, ac analytics.Client, ts *TaskService) *IssueService {
	return &IssueService{
		Queries:     q,
		TxStarter:   tx,
		Bus:         bus,
		Analytics:   ac,
		TaskService: ts,
	}
}

// IssueCreateParams carries the already-validated, already-resolved inputs
// to IssueService.Create. The handler owns the parsing step that turns its
// request payload into this struct; the service stays transport-agnostic.
type IssueCreateParams struct {
	WorkspaceID      pgtype.UUID
	Title            string
	Description      pgtype.Text
	Status           string
	Priority         string
	ExecutorType     pgtype.Text
	ExecutorID       pgtype.UUID
	OwnerType        pgtype.Text
	OwnerID          pgtype.UUID
	ReviewerType     pgtype.Text
	ReviewerID       pgtype.UUID
	ReviewSubmission []byte
	CreatorType      string // "agent" or "member"
	CreatorID        pgtype.UUID
	ParentIssueID    pgtype.UUID
	ProjectID        pgtype.UUID
	StartDate        pgtype.Date
	DueDate          pgtype.Date
	OriginType       pgtype.Text
	OriginID         pgtype.UUID
	AttachmentIDs    []pgtype.UUID
	// LabelIDs are the issue-scoped labels to attach to the new issue. They
	// are validated and written inside the create transaction (see Create),
	// so the issue is never committed with a partial or wrong label set. An
	// unknown or non-issue label id fails the whole create with
	// ErrIssueLabelNotFound rather than being silently dropped.
	LabelIDs       []pgtype.UUID
	AllowDuplicate bool
	// Stage groups this issue into an ordered barrier group under its parent
	// (NULL = unstaged). See issue_child_done.go for the staged-barrier wake.
	Stage pgtype.Int4
	// SourceContext is set only by the comment-scoped manual create endpoint.
	// Its immutable snapshot and cloned attachment rows commit in the same
	// transaction as the new issue.
	SourceContext *SourceContextCapture
}

// IssueCreateOpts groups optional knobs for IssueService.Create. Most
// callers leave it zero-valued.
type IssueCreateOpts struct {
	// BroadcastPayload, if non-nil, is invoked after the issue row is
	// created and attachments are linked. Its return value is sent as
	// the EventIssueCreated payload via the event bus. The HTTP handler
	// uses this hook to inject its IssueResponse without forcing this
	// package to depend on handler-layer types. If nil, the service
	// emits a minimal `{"issue_id": <uuid>}` payload — enough for cache
	// invalidation, but front-ends that expect the full response shape
	// must provide BroadcastPayload. The labels argument is the authoritative
	// snapshot attached in the create transaction, so the emitted payload can
	// carry the new issue's labels and every online client renders it already
	// labeled instead of blank until a refetch.
	BroadcastPayload func(issue db.Issue, attachments []db.Attachment, labels []db.IssueLabel) map[string]any

	// ActorID overrides the actor ID used for broadcast + analytics
	// when it differs from the creator on the row. Agent-created issues
	// use the agent UUID here (the creator_id column is the daemon
	// owner). Empty falls back to CreatorID.
	ActorID string

	// AnalyticsAgentID is the agent associated with the issue for
	// analytics purposes (executor agent or, for agent-created issues,
	// the creator agent). Resolved by the caller because it depends on
	// transport context.
	AnalyticsAgentID string

	// Platform tags the IssueCreated analytics + business-metrics event
	// with the client surface the request came in on (web / desktop /
	// daemon / lark / automation). Derived from middleware's client
	// metadata at the handler layer.
	Platform string

	// ExecutorRunFireAt creates the automatic executor task in a
	// durable deferred state. Channel /issue uses this while detached media is
	// still resolving, then promotes the returned task after attachment binding.
	// Zero preserves the ordinary immediate enqueue path.
	ExecutorRunFireAt time.Time
	// ConsumeTaskLeaseID is the server-resolved one-shot capability used by
	// quick-create. It is revoked in the same transaction as the issue insert;
	// a replay therefore cannot create a second issue, while a failed insert
	// rolls the consumption back with the rest of the transaction.
	ConsumeTaskLeaseID     pgtype.UUID
	ConsumeTaskLeaseTaskID pgtype.UUID
}

// ErrActiveDuplicate signals that the duplicate guard found an active
// issue with the same (workspace, project, parent, title) tuple and
// AllowDuplicate was false. The IssueCreateResult.DuplicateIssue field is
// populated when this error is returned so callers can render the
// conflict (HTTP 409, Lark card, etc.).
var ErrActiveDuplicate = errors.New("active duplicate issue exists")

// ErrParentIssueNotFound signals that the supplied ParentIssueID does
// not exist in the issue's workspace. The service refuses to create
// orphaned or cross-workspace child issues; callers translate this into
// their transport's 400 / Lark card error.
var ErrParentIssueNotFound = errors.New("parent issue not found in this workspace")

// ErrProjectNotFound signals that the supplied ProjectID does not exist
// in the issue's workspace. Cross-workspace project IDs are rejected
// here so every create entry (HTTP `POST /issues`, Lark `/issue`, future
// MCP / API key callers) enforces the same workspace boundary without
// having to remember it. Callers translate this into 400.
var ErrProjectNotFound = errors.New("project not found in this workspace")

// ErrIssueLabelNotFound signals that one of the supplied LabelIDs does not
// exist in the issue's workspace or is not an issue-scoped label. The whole
// create is rejected so a new issue is never born with a partial or wrong
// label set. Callers translate this into their transport's 400.
var ErrIssueLabelNotFound = errors.New("issue label not found in this workspace")

// ErrIssueStatusUnavailable signals that the requested custom status was
// archived between the caller's pre-flight validation and the create
// transaction. Callers translate this into a 409 — the request was valid when
// it arrived, so retrying against the refreshed catalog is the remedy.
var ErrIssueStatusUnavailable = errors.New("issue status is no longer available")

// ErrCapabilityConsumed means the task-scoped one-shot lease was already
// revoked/expired or did not belong to the requested task/workspace. Callers
// translate this to a conflict rather than retrying an operation that can never
// be admitted by the same lease again.
var ErrCapabilityConsumed = errors.New("task capability lease was already consumed")

var ErrSourceContextAlreadyAttached = errors.New("source context is already attached")

// IssueCreateResult is the typed return from IssueService.Create.
//
//   - On the happy path: Issue is the new row, Attachments lists the
//     linked attachments (may be empty), DuplicateIssue is nil.
//   - On ErrActiveDuplicate: DuplicateIssue is the row that blocked the
//     create; Issue and Attachments are zero.
type IssueCreateResult struct {
	Issue       db.Issue
	Attachments []db.Attachment
	// ExecutorTaskID is populated when Create enqueues the automatic task for
	// an agent executor, including a task deferred by ExecutorRunFireAt.
	ExecutorTaskID pgtype.UUID
	// Labels is the authoritative set of labels attached to the new issue in
	// the create transaction (empty when none were requested). Callers echo it
	// on the create response + issue:created event so every client renders the
	// new issue already labeled and a new client can detect that the backend
	// understood label_ids (see the create handler's compatibility contract).
	Labels         []db.IssueLabel
	DuplicateIssue *db.Issue
}

// Create prepares optional external task inputs, then commits the issue, labels,
// source context, attachment links and automatic executor task together. A
// required task write failure rolls the issue creation back. Events and daemon
// wakeups are published only after commit, in issue-created then task-queued
// order. Media-gated tasks remain deferred until their existing promotion path.
//
// Validation that lives in the service (parent existence, project
// workspace membership, parent → project back-fill) is enforced here so
// every create entry — HTTP `POST /issues`, Lark `/issue`, future
// MCP/API-key callers — shares the same workspace boundary semantics.
// Caller-owned validation is limited to transport-shaped checks: title
// required, RFC3339 date format, and role-pair sanity.
func (s *IssueService) Create(ctx context.Context, p IssueCreateParams, opts IssueCreateOpts) (IssueCreateResult, error) {
	issueCountPolicy := ResolveIssueCountPolicy(ctx, s.Entitlements, p.WorkspaceID)
	issueID := dbid.NewV7()
	tx, err := s.TxStarter.Begin(ctx)
	if err != nil {
		return IssueCreateResult{}, fmt.Errorf("begin tx: %w", err)
	}
	defer tx.Rollback(ctx)
	qtx := s.Queries.WithTx(tx)
	if _, err := qtx.LockWorkspaceForIssueCreate(ctx, p.WorkspaceID); err != nil {
		return IssueCreateResult{}, fmt.Errorf("lock workspace for issue create: %w", err)
	}

	if opts.ConsumeTaskLeaseID.Valid {
		if !opts.ConsumeTaskLeaseTaskID.Valid {
			return IssueCreateResult{}, ErrCapabilityConsumed
		}
		rows, err := qtx.ConsumeTaskCapabilityLease(ctx, db.ConsumeTaskCapabilityLeaseParams{
			ID:            opts.ConsumeTaskLeaseID,
			WorkspaceID:   p.WorkspaceID,
			TaskID:        opts.ConsumeTaskLeaseTaskID,
			RevokedReason: pgtype.Text{String: "quick_create_consumed", Valid: true},
		})
		if err != nil {
			return IssueCreateResult{}, fmt.Errorf("consume task capability lease: %w", err)
		}
		if rows != 1 {
			return IssueCreateResult{}, ErrCapabilityConsumed
		}
	}

	// A create landing on a CUSTOM status takes the shared catalog lock AND
	// re-resolves the status inside this transaction. The caller validated the
	// status before the transaction opened, which is early enough to return a
	// clean 400 but too early to be safe: an archive can commit in between.
	// Re-checking under the lock is what makes the status provably active at
	// the moment the row is written. Built-in statuses skip both — they can
	// never be archived, so the common path is unchanged. (MUL-6243)
	if !issuestatus.IsBuiltIn(p.Status) {
		if err := qtx.LockIssueStatusCatalogShared(ctx, p.WorkspaceID); err != nil {
			return IssueCreateResult{}, err
		}
		if _, err := issuestatus.Resolve(ctx, qtx, p.WorkspaceID, p.Status); err != nil {
			if errors.Is(err, issuestatus.ErrUnknownStatus) {
				return IssueCreateResult{}, ErrIssueStatusUnavailable
			}
			return IssueCreateResult{}, err
		}
	}

	// Match issue edits and attachment deletion: catalog, attachments, then
	// issue rows. Capturing source context already locks the source issue;
	// waiting for attachment rows after that would invert the edit order.
	if len(p.AttachmentIDs) > 0 {
		if _, err := qtx.LockAttachmentsForIssueLink(ctx, db.LockAttachmentsForIssueLinkParams{
			WorkspaceID: p.WorkspaceID, AttachmentIds: p.AttachmentIDs,
		}); err != nil {
			return IssueCreateResult{}, fmt.Errorf("lock issue attachments: %w", err)
		}
	}

	if p.SourceContext != nil {
		if _, err := qtx.LockIssueForDescriptionUpdate(ctx, db.LockIssueForDescriptionUpdateParams{
			ID: p.SourceContext.SourceIssueID, WorkspaceID: p.WorkspaceID,
		}); err != nil {
			if errors.Is(err, pgx.ErrNoRows) {
				return IssueCreateResult{}, ErrSourceIssueDeleted
			}
			return IssueCreateResult{}, fmt.Errorf("lock source issue: %w", err)
		}
		locked, err := qtx.LockCommentAncestorPath(ctx, db.LockCommentAncestorPathParams{
			CommentID: p.SourceContext.AnchorCommentID, WorkspaceID: p.WorkspaceID,
			IssueID: p.SourceContext.SourceIssueID,
		})
		if err != nil {
			return IssueCreateResult{}, fmt.Errorf("lock anchor comment thread: %w", err)
		}
		if len(locked) == 0 {
			return IssueCreateResult{}, ErrAnchorCommentDeleted
		}
		current, err := BuildSourceContext(ctx, qtx, p.WorkspaceID, p.SourceContext.AnchorCommentID)
		if err != nil {
			return IssueCreateResult{}, err
		}
		if current.Digest != p.SourceContext.Digest {
			return IssueCreateResult{}, ErrSourceContextChanged
		}
	}

	// Resolve and validate parent / project before reading from the
	// duplicate guard so a forged parent or project ID is rejected
	// before we touch the issue counter. Both checks scope by
	// WorkspaceID — there is no path from this service to a row in a
	// foreign workspace.
	projectID := p.ProjectID
	if p.ParentIssueID.Valid {
		parent, err := qtx.GetIssueInWorkspace(ctx, db.GetIssueInWorkspaceParams{
			ID:          p.ParentIssueID,
			WorkspaceID: p.WorkspaceID,
		})
		if err != nil || !parent.ID.Valid {
			return IssueCreateResult{}, ErrParentIssueNotFound
		}
		// Back-fill project from parent when the caller did not pin
		// one explicitly. Matches the long-standing HTTP behavior: a
		// sub-issue inherits its parent's project unless overridden.
		if !projectID.Valid {
			projectID = parent.ProjectID
		}
	}
	if projectID.Valid {
		if _, err := qtx.GetProjectInWorkspace(ctx, db.GetProjectInWorkspaceParams{
			ID:          projectID,
			WorkspaceID: p.WorkspaceID,
		}); err != nil {
			return IssueCreateResult{}, ErrProjectNotFound
		}
	}

	// Validate labels before we increment the issue counter so a stale or
	// wrong-scope selection fails the create cheaply. The de-duplicated rows
	// are attached to the issue below, inside this same transaction, and
	// echoed back as the authoritative snapshot in the result.
	labels, err := validateIssueLabels(ctx, qtx, p.WorkspaceID, p.LabelIDs)
	if err != nil {
		return IssueCreateResult{}, err
	}

	duplicate, found, err := issueguard.LockAndFindActiveDuplicate(ctx, qtx, p.WorkspaceID, projectID, p.ParentIssueID, p.Title, p.AllowDuplicate)
	if err != nil {
		return IssueCreateResult{}, fmt.Errorf("duplicate guard: %w", err)
	}
	if found {
		dup := duplicate
		return IssueCreateResult{DuplicateIssue: &dup}, ErrActiveDuplicate
	}

	issueNumber, err := AllocateIssueNumber(ctx, qtx, p.WorkspaceID, issueCountPolicy)
	if err != nil {
		return IssueCreateResult{}, fmt.Errorf("allocate issue number: %w", err)
	}

	// New issues sort to the top of their (workspace, status) column for
	// manual ordering. Computed inside the tx, after IncrementIssueCounter
	// has already taken the workspace row lock, so two concurrent creates
	// in the same workspace see each other's positions and don't both
	// land on the same min-1 slot. Concurrent manual reorder via
	// UpdateIssue(position) does NOT take this lock, so a create racing
	// a reorder is still allowed to collide on position — manual ordering
	// is best-effort and the UI tolerates equal positions by falling back
	// to the secondary ORDER BY key.
	newPosition, err := issueposition.NextTopPosition(ctx, tx, p.WorkspaceID, p.Status)
	if err != nil {
		return IssueCreateResult{}, fmt.Errorf("next top position: %w", err)
	}

	var issue db.Issue
	if p.OriginType.Valid {
		issue, err = qtx.CreateIssueWithOrigin(ctx, db.CreateIssueWithOriginParams{
			ReviewSubmission: p.ReviewSubmission,
			ID:               issueID,
			WorkspaceID:      p.WorkspaceID,
			Title:            p.Title,
			Description:      p.Description,
			Status:           p.Status,
			Priority:         p.Priority,
			ExecutorType:     p.ExecutorType,
			ExecutorID:       p.ExecutorID,
			OwnerType:        p.OwnerType,
			OwnerID:          p.OwnerID,
			ReviewerType:     p.ReviewerType,
			ReviewerID:       p.ReviewerID,
			CreatorType:      p.CreatorType,
			CreatorID:        p.CreatorID,
			ParentIssueID:    p.ParentIssueID,
			Position:         newPosition,
			StartDate:        p.StartDate,
			DueDate:          p.DueDate,
			Number:           issueNumber,
			ProjectID:        projectID,
			OriginType:       p.OriginType,
			OriginID:         p.OriginID,
			Stage:            p.Stage,
		})
	} else {
		issue, err = qtx.CreateIssue(ctx, db.CreateIssueParams{
			ReviewSubmission: p.ReviewSubmission,
			ID:               issueID,
			WorkspaceID:      p.WorkspaceID,
			Title:            p.Title,
			Description:      p.Description,
			Status:           p.Status,
			Priority:         p.Priority,
			ExecutorType:     p.ExecutorType,
			ExecutorID:       p.ExecutorID,
			OwnerType:        p.OwnerType,
			OwnerID:          p.OwnerID,
			ReviewerType:     p.ReviewerType,
			ReviewerID:       p.ReviewerID,
			CreatorType:      p.CreatorType,
			CreatorID:        p.CreatorID,
			ParentIssueID:    p.ParentIssueID,
			Position:         newPosition,
			StartDate:        p.StartDate,
			DueDate:          p.DueDate,
			Number:           issueNumber,
			ProjectID:        projectID,
			Stage:            p.Stage,
		})
	}
	if err != nil {
		return IssueCreateResult{}, fmt.Errorf("create issue: %w", err)
	}

	if p.SourceContext != nil {
		if _, err := PersistSourceContext(ctx, qtx, *p.SourceContext, issue.ID, pgtype.UUID{}); err != nil {
			return IssueCreateResult{}, fmt.Errorf("persist source context: %w", err)
		}
	} else if p.OriginType.Valid && p.OriginType.String == "quick_create" && p.OriginID.Valid {
		task, taskErr := qtx.GetAgentTaskInWorkspace(ctx, db.GetAgentTaskInWorkspaceParams{
			ID: p.OriginID, WorkspaceID: p.WorkspaceID,
		})
		if taskErr != nil {
			return IssueCreateResult{}, fmt.Errorf("load quick-create origin task: %w", taskErr)
		}
		if p.CreatorType != "agent" || !p.CreatorID.Valid || p.CreatorID != task.AgentID {
			return IssueCreateResult{}, errors.New("quick-create origin task does not belong to the creating agent")
		}
		var quickCreate QuickCreateContext
		if err := json.Unmarshal(task.Context, &quickCreate); err != nil {
			return IssueCreateResult{}, fmt.Errorf("decode quick-create origin context: %w", err)
		}
		if quickCreate.Type != QuickCreateContextType {
			return IssueCreateResult{}, errors.New("quick-create origin task has invalid context type")
		}
		contextWorkspaceID, parseErr := util.ParseUUID(quickCreate.WorkspaceID)
		if parseErr != nil || contextWorkspaceID != p.WorkspaceID {
			return IssueCreateResult{}, errors.New("quick-create origin context has invalid workspace")
		}
		if quickCreate.SourceContextID != "" {
			contextID, parseErr := util.ParseUUID(quickCreate.SourceContextID)
			if parseErr != nil {
				return IssueCreateResult{}, fmt.Errorf("invalid quick-create source context id: %w", parseErr)
			}
			requesterID, parseErr := util.ParseUUID(quickCreate.RequesterID)
			if parseErr != nil || !task.OriginatorUserID.Valid || requesterID != task.OriginatorUserID {
				return IssueCreateResult{}, errors.New("quick-create source context has invalid requester")
			}
			pending, pendingErr := qtx.GetPendingIssueSourceContextByOriginTask(ctx, db.GetPendingIssueSourceContextByOriginTaskParams{
				WorkspaceID: p.WorkspaceID, OriginTaskID: task.ID,
			})
			if pendingErr != nil {
				if errors.Is(pendingErr, pgx.ErrNoRows) {
					return IssueCreateResult{}, ErrSourceContextAlreadyAttached
				}
				return IssueCreateResult{}, fmt.Errorf("load pending quick-create source context: %w", pendingErr)
			}
			if pending.ID != contextID || pending.CapturedByUserID != requesterID {
				return IssueCreateResult{}, errors.New("quick-create source context ownership mismatch")
			}
			if _, attachErr := qtx.AttachIssueSourceContext(ctx, db.AttachIssueSourceContextParams{
				IssueID: issue.ID, WorkspaceID: p.WorkspaceID, ID: contextID, OriginTaskID: task.ID,
			}); attachErr != nil {
				if errors.Is(attachErr, pgx.ErrNoRows) {
					return IssueCreateResult{}, ErrSourceContextAlreadyAttached
				}
				return IssueCreateResult{}, fmt.Errorf("attach quick-create source context: %w", attachErr)
			}
		}
	}

	// Attach labels inside the create transaction so the issue and its
	// labels commit together — the old flow created the issue first and
	// attached labels in a second, non-atomic round-trip whose partial
	// failure left the issue mis-categorized. AttachLabelToIssue is
	// workspace/resource_type-guarded and ON CONFLICT DO NOTHING, and the
	// ids were already validated above.
	for _, label := range labels {
		if err := qtx.AttachLabelToIssueOnCreate(ctx, db.AttachLabelToIssueOnCreateParams{
			IssueID:     issue.ID,
			LabelID:     label.ID,
			WorkspaceID: p.WorkspaceID,
		}); err != nil {
			return IssueCreateResult{}, fmt.Errorf("attach issue label: %w", err)
		}
	}

	// A queued row is claimable as soon as this transaction commits, including
	// by polling daemons that never receive our wakeup. Bind uploaded inputs
	// before the task becomes visible so its first run cannot miss attachments.
	attachments, err := linkIssueAttachments(ctx, qtx, issue, p.AttachmentIDs)
	if err != nil {
		return IssueCreateResult{}, err
	}
	executorTask, verdict, err := s.createIssueExecutorTask(ctx, qtx, issue, opts.ExecutorRunFireAt)
	if err != nil {
		return IssueCreateResult{}, fmt.Errorf("create executor task: %w", err)
	}
	reviewEntry := issuestatus.Effective(ctx, qtx, issue.WorkspaceID, issue.Status) == issuestatus.InReview
	if reviewEntry && issue.ReviewerType.Valid && issue.ReviewerType.String != "member" {
		if s.TaskService == nil || s.TaskService.Coordination == nil {
			return IssueCreateResult{}, errors.New("review entry requires agent coordination service")
		}
		actorUserID := pgtype.UUID{}
		if issue.CreatorType == "member" {
			actorUserID = issue.CreatorID
		}
		if err := s.TaskService.Coordination.RecordReviewEntryTx(ctx, qtx, issue, actorUserID, ""); err != nil {
			return IssueCreateResult{}, fmt.Errorf("record new issue review handoff: %w", err)
		}
	}

	if err := tx.Commit(ctx); err != nil {
		return IssueCreateResult{}, fmt.Errorf("commit: %w", err)
	}
	if reviewEntry && s.TaskService != nil && s.TaskService.Coordination != nil {
		s.TaskService.Coordination.Wake()
	}

	actorID := opts.ActorID
	if actorID == "" {
		actorID = util.UUIDToString(issue.CreatorID)
	}

	var executorTaskID pgtype.UUID
	if issue.ExecutorType.String == "agent" {
		executorTaskID = executorTask.ID
	}
	issueCreatedPending := isIssueCreatedPendingTask(executorTask)
	if executorTask.ID.Valid && executorTask.Status == "deferred" {
		if err := s.TaskService.hydrateDeferredChannelIssueTaskOverlay(ctx, executorTask); err != nil {
			// The deferred task is already durable. An optional integration failure
			// must not turn a committed issue into a retry duplicate.
			slog.Warn("hydrate deferred channel issue task overlay failed",
				"issue_id", util.UUIDToString(issue.ID), "task_id", util.UUIDToString(executorTask.ID), "error", err)
		}
	}
	if issueCreatedPending {
		if err := s.TaskService.hydrateIssueCreatedTaskOverlay(ctx, executorTask); err != nil {
			// The overlay is optional, while the issue and task are already durable.
			// Keep the publication fence until the task is activated so a slow or
			// unavailable integration cannot race the first claim.
			slog.Warn("hydrate issue-created task overlay failed",
				"issue_id", util.UUIDToString(issue.ID), "task_id", util.UUIDToString(executorTask.ID), "error", err)
		}
	}
	s.publishIssueCreated(issue, attachments, labels, p.CreatorType, actorID, opts)
	s.captureCreatedAnalytics(issue, p.CreatorType, actorID, opts)
	if issueCreatedPending {
		// Publish task:queued while the durable marker still excludes claims. The
		// activation CAS below is the only step that opens the claim gate, so a
		// polling daemon cannot emit task:dispatch between these two lifecycle
		// events.
		s.TaskService.broadcastTaskEvent(ctx, protocol.EventTaskQueued, executorTask)
		activated, activateErr := s.TaskService.activateIssueCreatedTask(ctx, executorTask.ID)
		if activateErr == nil {
			executorTask = activated
			s.TaskService.NotifyTaskEnqueued(ctx, executorTask)
		} else if !errors.Is(activateErr, pgx.ErrNoRows) {
			// A committed create must remain successful. A later claim poll will
			// retry the durable publication fence if activation failed here.
			slog.Warn("activate issue-created task failed",
				"issue_id", util.UUIDToString(issue.ID), "task_id", util.UUIDToString(executorTask.ID), "error", activateErr)
		}
	} else if executorTask.ID.Valid && executorTask.Status == "queued" {
		s.TaskService.PublishQueuedTask(ctx, executorTask)
	}
	if opts.ExecutorRunFireAt.IsZero() && issue.ExecutorType.String == "agent" && verdict.Reason == dispatch.ReasonRuntimeUnusable {
		s.noteRuntimeUnusable(ctx, issue, verdict)
	}

	return IssueCreateResult{Issue: issue, Attachments: attachments, Labels: labels, ExecutorTaskID: executorTaskID}, nil
}

// validateIssueLabels checks that every requested label exists in the
// workspace and is issue-scoped, returning the de-duplicated label rows to
// attach. Returning the full rows (not just ids) lets Create echo an
// authoritative label snapshot on the create response + issue:created event
// without a second query. It mirrors the workspace + resource_type='issue'
// guard already enforced by AttachLabelToIssue so an unknown or wrong-scope id
// surfaces as ErrIssueLabelNotFound instead of a silent no-op insert. Invalid
// (zero) UUIDs are skipped. The label count per issue is small, so a GetLabel
// per distinct id is fine and avoids introducing a new batch query.
func validateIssueLabels(ctx context.Context, qtx *db.Queries, workspaceID pgtype.UUID, labelIDs []pgtype.UUID) ([]db.IssueLabel, error) {
	if len(labelIDs) == 0 {
		return nil, nil
	}
	seen := make(map[string]struct{}, len(labelIDs))
	deduped := make([]db.IssueLabel, 0, len(labelIDs))
	for _, labelID := range labelIDs {
		if !labelID.Valid {
			continue
		}
		key := util.UUIDToString(labelID)
		if _, ok := seen[key]; ok {
			continue
		}
		seen[key] = struct{}{}

		label, err := qtx.GetLabel(ctx, db.GetLabelParams{
			ID:          labelID,
			WorkspaceID: workspaceID,
		})
		if err != nil {
			if errors.Is(err, pgx.ErrNoRows) {
				return nil, ErrIssueLabelNotFound
			}
			return nil, fmt.Errorf("get issue label: %w", err)
		}
		if label.ResourceType != "issue" {
			return nil, ErrIssueLabelNotFound
		}
		deduped = append(deduped, label)
	}
	return deduped, nil
}

// linkIssueAttachments binds available uploaded rows in the create transaction.
// The query skips stale, foreign and already-linked ids; database errors roll
// back the create rather than committing a claimable task with missing inputs.
func linkIssueAttachments(ctx context.Context, q *db.Queries, issue db.Issue, ids []pgtype.UUID) ([]db.Attachment, error) {
	if len(ids) == 0 {
		return nil, nil
	}
	if _, err := q.LinkAttachmentsToIssue(ctx, db.LinkAttachmentsToIssueParams{
		IssueID: issue.ID, WorkspaceID: issue.WorkspaceID, AttachmentIds: ids, BumpRevision: false,
	}); err != nil {
		return nil, fmt.Errorf("link issue attachments: %w", err)
	}
	list, err := q.ListAttachmentsByIssue(ctx, db.ListAttachmentsByIssueParams{
		IssueID: issue.ID, WorkspaceID: issue.WorkspaceID,
	})
	if err != nil {
		return nil, fmt.Errorf("list issue attachments: %w", err)
	}
	return list, nil
}

func (s *IssueService) publishIssueCreated(issue db.Issue, attachments []db.Attachment, labels []db.IssueLabel, creatorType, actorID string, opts IssueCreateOpts) {
	if s.Bus == nil {
		return
	}
	var payload map[string]any
	if opts.BroadcastPayload != nil {
		payload = opts.BroadcastPayload(issue, attachments, labels)
	} else {
		// Minimal fallback so cache invalidations still fire even if the
		// caller forgot to supply a builder. Front-ends that expect the
		// full IssueResponse must pass BroadcastPayload.
		payload = map[string]any{"issue_id": util.UUIDToString(issue.ID)}
	}
	s.Bus.Publish(events.Event{
		Type:        protocol.EventIssueCreated,
		WorkspaceID: util.UUIDToString(issue.WorkspaceID),
		ActorType:   creatorType,
		ActorID:     actorID,
		Payload:     payload,
		IssueCreated: &events.IssueCreated{
			ID: util.UUIDToString(issue.ID), WorkspaceID: util.UUIDToString(issue.WorkspaceID),
			Title: issue.Title, Status: issue.Status, CreatorType: issue.CreatorType, CreatorID: util.UUIDToString(issue.CreatorID),
			Description: util.TextToPtr(issue.Description),
			OwnerType:   util.TextToPtr(issue.OwnerType), OwnerID: util.UUIDToPtr(issue.OwnerID),
			ExecutorType: util.TextToPtr(issue.ExecutorType), ExecutorID: util.UUIDToPtr(issue.ExecutorID),
			ReviewerType: util.TextToPtr(issue.ReviewerType), ReviewerID: util.UUIDToPtr(issue.ReviewerID),
		},
	})
}

// PublishAttachmentsChanged refreshes attachments and the issue projection
// after a detached channel media transaction. Issue creation is broadcast
// before the remote download finishes, so the attachment event closes the
// cache gap for current clients. The issue:updated event carries the newly
// materialized description through a pre-existing protocol that installed
// desktop clients already understand.
func (s *IssueService) PublishAttachmentsChanged(ctx context.Context, issue db.Issue, actorID pgtype.UUID) {
	if s.Bus == nil {
		return
	}
	if s.Queries == nil {
		s.publishIssueAttachmentsChanged(issue, actorID, 0)
		return
	}

	current, err := s.Queries.GetIssueInWorkspace(ctx, db.GetIssueInWorkspaceParams{
		ID:          issue.ID,
		WorkspaceID: issue.WorkspaceID,
	})
	if err != nil {
		slog.Warn("failed to load issue after channel media bind",
			"issue_id", util.UUIDToString(issue.ID), "error", err)
		s.publishIssueAttachmentsChanged(issue, actorID, 0)
		return
	}
	workspace, err := s.Queries.GetWorkspace(ctx, issue.WorkspaceID)
	if err != nil {
		slog.Warn("failed to load workspace after channel media bind",
			"workspace_id", util.UUIDToString(issue.WorkspaceID), "error", err)
		// Without the workspace we cannot publish the matching owner snapshot.
		// Keep this auxiliary event unversioned so clients invalidate instead of
		// advancing the owner revision past a snapshot they never received.
		s.publishIssueAttachmentsChanged(issue, actorID, 0)
		return
	}
	s.Bus.Publish(events.Event{
		Type:        protocol.EventIssueUpdated,
		WorkspaceID: util.UUIDToString(issue.WorkspaceID),
		ActorType:   "member",
		ActorID:     util.UUIDToString(actorID),
		Payload: map[string]any{
			"issue":            IssueToMapResolved(ctx, s.Queries, current, workspace.IssuePrefix),
			"owner_changed":    false,
			"executor_changed": false,
			"status_changed":   false,
			"project_changed":  false,
		},
	})
	// Publish the auxiliary projection only after the full owner snapshot at
	// this revision. Reversing these two events makes revision-aware clients
	// reject the issue:updated payload as an equal-revision duplicate.
	s.publishIssueAttachmentsChanged(current, actorID, current.Revision)
}

func (s *IssueService) publishIssueAttachmentsChanged(issue db.Issue, actorID pgtype.UUID, revision int64) {
	payload := map[string]any{"issue_id": util.UUIDToString(issue.ID)}
	if revision > 0 {
		payload["issue_revision"] = revision
	}
	s.Bus.Publish(events.Event{
		Type:        protocol.EventIssueAttachmentsChanged,
		WorkspaceID: util.UUIDToString(issue.WorkspaceID),
		ActorType:   "member",
		ActorID:     util.UUIDToString(actorID),
		Payload:     payload,
	})
}

func (s *IssueService) captureCreatedAnalytics(issue db.Issue, creatorType, actorID string, opts IssueCreateOpts) {
	if s.Analytics == nil {
		return
	}
	source, taskID, automationRunID := classifyOrigin(issue, opts)
	analyticsActorID := actorID
	if creatorType == "agent" {
		analyticsActorID = "agent:" + actorID
	}
	obsmetrics.RecordEvent(s.Analytics, s.Metrics, analytics.IssueCreated(
		analyticsActorID,
		util.UUIDToString(issue.WorkspaceID),
		util.UUIDToString(issue.ID),
		opts.AnalyticsAgentID,
		taskID,
		automationRunID,
		source,
		opts.Platform,
	))
}

// classifyOrigin maps the issue's origin_type / origin_id columns into the
// analytics source labels. Unknown origin_type falls back to SourceManual
// with the warning logged — analytics drift is preferable to dropping the
// event entirely.
func classifyOrigin(issue db.Issue, opts IssueCreateOpts) (source, taskID, automationRunID string) {
	source = analytics.SourceManual
	if !issue.OriginType.Valid {
		return source, "", ""
	}
	originID := util.UUIDToString(issue.OriginID)
	switch issue.OriginType.String {
	case "quick_create", "agent_create":
		// Both link the issue back to the agent_task_queue row that created it
		// (agent_create is the ordinary agent `issue create` path, MUL-4305);
		// surface that task id and keep the manual source label.
		return analytics.SourceManual, originID, ""
	case "automation":
		return analytics.SourceAutomation, "", originID
	default:
		slog.Warn("analytics: unknown issue origin type",
			"origin_type", issue.OriginType.String,
			"issue_id", util.UUIDToString(issue.ID),
		)
		return analytics.SourceManual, "", ""
	}
}

type issueExecutorTarget struct {
	agent    db.Agent
	teamID   pgtype.UUID
	verdict  AgentVerdict
	admitted bool
}

// resolveCreatedIssueExecutor preserves the two admission policies: a direct
// agent may wait for an offline runtime, while a team leader must be ready now.
// Missing or blocked targets do not prevent issue creation; failed database
// reads do, because they cannot establish whether a task is required.
func (s *IssueService) resolveCreatedIssueExecutor(ctx context.Context, q *db.Queries, issue db.Issue) (issueExecutorTarget, error) {
	var target issueExecutorTarget
	category := issuestatus.Effective(ctx, q, issue.WorkspaceID, issue.Status)
	if !issue.ExecutorType.Valid || !issue.ExecutorID.Valid ||
		category == "backlog" || category == issuestatus.InReview {
		return target, nil
	}
	agentID := issue.ExecutorID
	switch issue.ExecutorType.String {
	case "agent":
	case "team":
		team, err := q.GetTeamInWorkspace(ctx, db.GetTeamInWorkspaceParams{ID: issue.ExecutorID, WorkspaceID: issue.WorkspaceID})
		if errors.Is(err, pgx.ErrNoRows) {
			return target, nil
		}
		if err != nil {
			return target, err
		}
		agentID, target.teamID = team.LeaderID, team.ID
	default:
		return target, nil
	}
	agent, err := q.GetAgent(ctx, agentID)
	if errors.Is(err, pgx.ErrNoRows) {
		return target, nil
	}
	if err != nil {
		return target, err
	}
	target.agent = agent
	target.verdict, err = AgentReadiness(ctx, s.runtimeLookup(q), agent)
	if errors.Is(err, pgx.ErrNoRows) {
		return target, nil
	}
	if err != nil {
		return target, err
	}
	target.admitted = !target.verdict.Blocked()
	if target.teamID.Valid {
		target.admitted = target.verdict.Ready()
	}
	return target, nil
}

func (s *IssueService) createIssueExecutorTask(ctx context.Context, q *db.Queries, issue db.Issue, fireAt time.Time) (db.AgentTaskQueue, AgentVerdict, error) {
	target, err := s.resolveCreatedIssueExecutor(ctx, q, issue)
	if err != nil || !target.admitted {
		return db.AgentTaskQueue{}, target.verdict, err
	}
	if s.TaskService == nil {
		return db.AgentTaskQueue{}, target.verdict, errors.New("task service is not configured")
	}
	// No Composio, bus or wakeup on this service: only database reads and writes
	// are allowed before the caller commits. For a team, route to its leader
	// without changing the persisted issue's executor or provenance.
	txTasks := &TaskService{Queries: q}
	issue.ExecutorID = target.agent.ID
	params, err := txTasks.prepareIssueTaskWithCommentPlan(ctx, issue, pgtype.UUID{}, nil, false, "", pgtype.UUID{}, pgtype.UUID{})
	if err != nil {
		return db.AgentTaskQueue{}, target.verdict, err
	}
	if target.teamID.Valid {
		params.IsLeaderTask = pgtype.Bool{Bool: true, Valid: true}
		params.TeamID = target.teamID
	}
	var deferredAt pgtype.Timestamptz
	if !fireAt.IsZero() && !target.teamID.Valid {
		deferredAt = pgtype.Timestamptz{Time: fireAt, Valid: true}
	} else {
		// Keep ordinary tasks behind the durable issue-created publication fence.
		// The optional Composio overlay is hydrated after the transaction commits,
		// while the claim query still excludes this marker.
		params.IssueCreatedPending = pgtype.Bool{Bool: true, Valid: true}
	}
	task, err := persistIssueTask(ctx, q, params, deferredAt)
	return task, target.verdict, err
}
