package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"strings"
	"sync"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/issuestatus"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

// The database CHECK constraint is deliberately the source of truth for this
// list. Do not add a new coordination event here without a schema contract and
// a corresponding mainline implementation.
const (
	CoordinationEventTaskCompleted  = "task_completed"
	CoordinationEventReviewReturned = "review_returned"

	CoordinationAssignmentReviewer = "reviewer"
	CoordinationAssignmentExecutor = "executor"
)

const (
	coordinationAssignmentIDContextKey   = "coordination_assignment_id"
	coordinationAssignmentRoleContextKey = "coordination_assignment_role"
	coordinationOwnerTypeContextKey      = "coordination_owner_type"
	coordinationOwnerIDContextKey        = "coordination_owner_id"
	coordinationOwnerGenerationKey       = "coordination_owner_generation"
	coordinationIssueRevisionKey         = "coordination_issue_revision"
	coordinationSideChatParentTaskKey    = "side_chat_parent_task_id"
	coordinationSideChatRootCommentKey   = "side_chat_root_comment_id"
)

const (
	coordinationPollInterval      = time.Second
	coordinationLease             = 30 * time.Second
	coordinationRuntimeStaleAfter = 2 * time.Minute
	coordinationNoOwnerRetry      = 30 * time.Second
	coordinationTransientRetry    = 10 * time.Second
	coordinationClaimBatchSize    = 16
)

var (
	ErrCoordinationLeaseLost = errors.New("agent coordination lease lost")
)

// coordinationTaskContext is the immutable provenance written into a
// coordinator-created task. The owner generation and issue revision are
// snapshots: completion producers use them to reject a stale task after an
// executor/reviewer change. Pointers preserve the distinction between an old
// row with no snapshot and a snapshot whose value happens to be zero.
type coordinationTaskContext struct {
	AssignmentID    string `json:"coordination_assignment_id"`
	AssignmentRole  string `json:"coordination_assignment_role,omitempty"`
	OwnerType       string `json:"coordination_owner_type,omitempty"`
	OwnerID         string `json:"coordination_owner_id,omitempty"`
	OwnerGeneration *int64 `json:"coordination_owner_generation,omitempty"`
	IssueRevision   *int64 `json:"coordination_issue_revision,omitempty"`
}

// coordinationEventPayload is deliberately a superset of the event routing
// data and task attribution data. A retry must be able to create the same
// child task without consulting a vanished source task, while the event row
// remains the idempotency boundary.
type coordinationEventPayload struct {
	AssignmentID    string `json:"assignment_id,omitempty"`
	AssignmentRole  string `json:"assignment_role,omitempty"`
	AgentID         string `json:"agent_id,omitempty"`
	OwnerType       string `json:"owner_type,omitempty"`
	OwnerID         string `json:"owner_id,omitempty"`
	OwnerGeneration *int64 `json:"owner_generation,omitempty"`
	IssueRevision   *int64 `json:"issue_revision,omitempty"`
	FollowUp        *bool  `json:"follow_up,omitempty"`
	Outcome         string `json:"outcome,omitempty"`
	HandoffNote     string `json:"handoff_note,omitempty"`
	SourceTaskID    string `json:"source_task_id,omitempty"`

	ExplicitReviewer      bool   `json:"explicit_reviewer,omitempty"`
	ReviewerReassignment bool   `json:"reviewer_reassignment,omitempty"`
	ReviewerType          string `json:"reviewer_type,omitempty"`
	ReviewerID            string `json:"reviewer_id,omitempty"`

	OriginatorUserID     string `json:"originator_user_id,omitempty"`
	AccountableUserID    string `json:"accountable_user_id,omitempty"`
	OriginatorSource     string `json:"originator_source,omitempty"`
	TriggerEvidenceKind  string `json:"trigger_evidence_kind,omitempty"`
	TriggerEvidenceRefID string `json:"trigger_evidence_ref_id,omitempty"`
	TeamID               string `json:"team_id,omitempty"`
}

type coordinationReviewPublication struct {
	eventID        pgtype.UUID
	publicationKey string
	previous       db.Issue
	updated        db.Issue
	activity       db.ActivityLog
}

// AgentCoordinationService is the durable producer/consumer for the
// agent_coordination_outbox. The event bus is intentionally not a dependency:
// a process restart must recover solely from PostgreSQL rows.
type AgentCoordinationService struct {
	Queries   *db.Queries
	TxStarter TxStarter
	Tasks     *TaskService
	WorkerID  string

	wake      chan struct{}
	done      chan struct{}
	startOnce sync.Once
}

func NewAgentCoordinationService(q *db.Queries, tx TxStarter, tasks *TaskService) *AgentCoordinationService {
	workerID := "agent-coordination"
	if id := dbid.NewV7(); id.Valid {
		workerID += ":" + util.UUIDToString(id)
	}
	return &AgentCoordinationService{
		Queries:   q,
		TxStarter: tx,
		Tasks:     tasks,
		WorkerID:  workerID,
		wake:      make(chan struct{}, 1),
		done:      make(chan struct{}),
	}
}

// Wake is a best-effort latency hint. The outbox and its available_at value
// remain authoritative, so a lost wake only adds one poll interval.
func (s *AgentCoordinationService) Wake() {
	if s == nil || s.wake == nil {
		return
	}
	select {
	case s.wake <- struct{}{}:
	default:
	}
}

// Start starts exactly one worker loop. Claiming expired processing rows makes
// restart recovery a database property rather than an in-memory handoff.
func (s *AgentCoordinationService) Start(ctx context.Context) {
	if s == nil {
		return
	}
	s.startOnce.Do(func() {
		go s.run(ctx)
	})
}

func (s *AgentCoordinationService) run(ctx context.Context) {
	defer close(s.done)
	ticker := time.NewTicker(coordinationPollInterval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-s.wake:
			s.RunOnce(ctx)
		case <-ticker.C:
			s.RunOnce(ctx)
		}
	}
}

// WaitWithTimeout is used by graceful shutdown after the shared worker
// context is cancelled. A false result means leases are left for expiry and
// the next process will reclaim them.
func (s *AgentCoordinationService) WaitWithTimeout(timeout time.Duration) bool {
	if s == nil || s.done == nil {
		return true
	}
	if timeout <= 0 {
		select {
		case <-s.done:
			return true
		default:
			return false
		}
	}
	timer := time.NewTimer(timeout)
	defer timer.Stop()
	select {
	case <-s.done:
		return true
	case <-timer.C:
		return false
	}
}

// RunOnce claims a bounded batch and processes each claim independently. A
// per-row error never aborts the batch; the row remains leased until its
// fenced retry or expiry, and another worker can recover it after a crash.
func (s *AgentCoordinationService) RunOnce(ctx context.Context) {
	if s == nil || s.Queries == nil {
		return
	}
	claimed, err := s.Queries.ClaimAgentCoordinationOutbox(ctx, db.ClaimAgentCoordinationOutboxParams{
		LeaseOwner:   pgtype.Text{String: s.WorkerID, Valid: s.WorkerID != ""},
		LeaseSeconds: coordinationLease.Seconds(),
		BatchSize:    coordinationClaimBatchSize,
	})
	if err != nil {
		slog.Warn("agent coordination claim failed", "error", err)
		return
	}
	for _, event := range claimed {
		task, created, err := s.processClaim(ctx, event)
		if err != nil {
			slog.Warn("agent coordination event failed",
				"event_id", util.UUIDToString(event.ID),
				"event_key", event.EventKey,
				"error", err,
			)
			continue
		}
		if created && s.Tasks != nil {
			// This happens after the transaction that linked the assignment and
			// outbox completion, so a client can never observe a queue hint for a
			// task whose assignment transaction later rolls back.
			s.Tasks.BroadcastTaskQueued(ctx, task)
			s.Tasks.NotifyTaskEnqueued(ctx, task)
		}
	}
}

// RecordTaskCompleted is the standalone producer wrapper. Lifecycle paths
// that already own a transaction must call RecordTaskCompletedTx instead.
func (s *AgentCoordinationService) RecordTaskCompleted(ctx context.Context, task db.AgentTaskQueue) error {
	err := s.runInTx(ctx, func(qtx *db.Queries) error {
		return s.RecordTaskCompletedTx(ctx, qtx, task)
	})
	if err == nil {
		s.Wake()
	}
	return err
}

// RecordTaskCompletedTx records only issue-bound, non-side-chat coordination
// tasks with valid assignment provenance. Legacy issue tasks without that
// provenance are intentionally ignored; inventing an assignment after the
// fact would break the durable discovery contract.
func (s *AgentCoordinationService) RecordTaskCompletedTx(ctx context.Context, qtx *db.Queries, task db.AgentTaskQueue) error {
	if !coordinationTaskEligible(task) || task.Status != "completed" {
		return nil
	}
	taskContext, present, err := decodeCoordinationTaskContext(task.Context)
	if err != nil || !present {
		return err
	}
	assignmentID, err := util.ParseUUID(taskContext.AssignmentID)
	if err != nil {
		return fmt.Errorf("coordination completion: assignment id: %w", err)
	}
	agent, issue, err := coordinationIssueForTask(ctx, qtx, task)
	if err != nil {
		return err
	}
	assignment, err := qtx.GetAgentCoordinationAssignmentForTask(ctx, db.GetAgentCoordinationAssignmentForTaskParams{
		WorkspaceID:  agent.WorkspaceID,
		IssueID:      issue.ID,
		TaskID:       task.ID,
		AssignmentID: assignmentID,
	})
	if errors.Is(err, pgx.ErrNoRows) {
		// A task whose assignment was superseded or deleted is not allowed to
		// create a new durable event from stale context.
		return nil
	}
	if err != nil {
		return fmt.Errorf("coordination completion: load assignment: %w", err)
	}
	if !coordinationAssignmentMatchesTask(assignment, task, taskContext) {
		return nil
	}
	changed, err := qtx.CompleteAgentCoordinationAssignmentForTask(ctx, db.CompleteAgentCoordinationAssignmentForTaskParams{
		AssignmentID: assignment.ID,
		TaskID:       task.ID,
		AgentID:      task.AgentID,
		WorkspaceID:  agent.WorkspaceID,
		IssueID:      issue.ID,
	})
	if err != nil {
		return fmt.Errorf("coordination completion: close assignment: %w", err)
	}
	if changed != 1 {
		// The exact assignment/task relation was superseded between the read and
		// the fenced update. Do not derive a new event from stale provenance.
		return nil
	}
	if !coordinationCompletionStillOwnsIssue(issue, assignment.Role, taskContext, task.AgentID) {
		// The task was terminally acknowledged, but its owner/reviewer fence is
		// stale. Close only the exact assignment/task relation above; a separate
		// issue-reassignment producer owns any new handoff.
		return nil
	}

	followUp := assignment.Role == CoordinationAssignmentExecutor
	payload := coordinationCompletionPayload(task, assignment, issue, taskContext, followUp, "completed")
	if !followUp {
		return s.enqueueCoordinationEvent(ctx, qtx,
			"task_completed:"+util.UUIDToString(task.ID),
			CoordinationEventTaskCompleted,
			issue.WorkspaceID, issue.ID, task.ID, payload, "", "", pgtype.UUID{},
		)
	}

	ownerType, ownerID := coordinationIssueOwner(issue, CoordinationAssignmentReviewer)
	return s.enqueueCoordinationEvent(ctx, qtx,
		"task_completed:"+util.UUIDToString(task.ID),
		CoordinationEventTaskCompleted,
		issue.WorkspaceID, issue.ID, task.ID, payload,
		CoordinationAssignmentReviewer, ownerType, ownerID,
	)
}

// RecordTaskFailedTx is intentionally a no-op. The schema only permits the
// existing completion and review-return event types, and the mainline failure/retry lifecycle
// already owns failed-task recovery. Treating a failure as task_completed or
// inventing another event would create a false durable handoff.
func (s *AgentCoordinationService) RecordTaskFailedTx(_ context.Context, _ *db.Queries, _ db.AgentTaskQueue) error {
	return nil
}

// LockActiveReviewerTasksForReviewReturnTx acquires reviewer task rows before
// the issue row, matching the coordinator worker's task -> issue lock order.
func (s *AgentCoordinationService) LockActiveReviewerTasksForReviewReturnTx(ctx context.Context, qtx *db.Queries, issueID pgtype.UUID) ([]pgtype.UUID, error) {
	taskIDs, err := qtx.LockActiveReviewerTasksForReviewReturn(ctx, issueID)
	if err != nil {
		return nil, fmt.Errorf("review return: lock reviewer tasks: %w", err)
	}
	return taskIDs, nil
}

// RetireLockedReviewerTasksForReviewReturnTx cancels only rows fenced by the
// lock call above. A missing row means its state changed inside the supposed
// lock window and aborts the issue transaction instead of guessing.
func (s *AgentCoordinationService) RetireLockedReviewerTasksForReviewReturnTx(ctx context.Context, qtx *db.Queries, taskIDs []pgtype.UUID) ([]db.AgentTaskQueue, error) {
	retired := make([]db.AgentTaskQueue, 0, len(taskIDs))
	for _, taskID := range taskIDs {
		task, err := qtx.CancelAgentTask(ctx, taskID)
		if err != nil {
			return nil, fmt.Errorf("review return: retire reviewer task %s: %w", util.UUIDToString(taskID), err)
		}
		retired = append(retired, task)
	}
	return retired, nil
}

// RecordReviewReturn is the transaction-owning wrapper for callers that have
// already changed the issue review state. It reloads the issue by workspace in
// the producer transaction before writing the event.
func (s *AgentCoordinationService) RecordReviewReturn(ctx context.Context, issue db.Issue, sourceTaskID pgtype.UUID, handoffNote string) error {
	err := s.runInTx(ctx, func(qtx *db.Queries) error {
		return s.RecordReviewReturnTx(ctx, qtx, issue, sourceTaskID, handoffNote)
	})
	if err == nil {
		s.Wake()
	}
	return err
}

func (s *AgentCoordinationService) RecordReviewReturnTx(ctx context.Context, qtx *db.Queries, issue db.Issue, sourceTaskID pgtype.UUID, handoffNote string) error {
	current, err := qtx.GetIssueInWorkspace(ctx, db.GetIssueInWorkspaceParams{ID: issue.ID, WorkspaceID: issue.WorkspaceID})
	if err != nil {
		return fmt.Errorf("review return: load issue: %w", err)
	}
	payload, err := coordinationPayloadFromSourceTask(ctx, qtx, current.WorkspaceID, current.ID, sourceTaskID)
	if err != nil {
		return fmt.Errorf("review return: load source task: %w", err)
	}
	payload.AssignmentRole = CoordinationAssignmentExecutor
	payload.FollowUp = boolPtr(true)
	payload.Outcome = "review_returned"
	payload.HandoffNote = handoffNote
	payload.SourceTaskID = util.UUIDToString(sourceTaskID)
	payload.IssueRevision = int64Ptr(current.Revision)
	payload.TriggerEvidenceKind = "review_returned"
	payload.TriggerEvidenceRefID = util.UUIDToString(sourceTaskID)
	ownerType, ownerID := coordinationIssueOwner(current, CoordinationAssignmentExecutor)
	payload.OwnerType = ownerType
	payload.OwnerID = util.UUIDToString(ownerID)
	payload.OwnerGeneration = int64Ptr(current.ExecutorGeneration)
	return s.enqueueCoordinationEvent(ctx, qtx,
		"review_returned:"+util.UUIDToString(current.ID)+":"+fmt.Sprintf("%d", current.Revision),
		CoordinationEventReviewReturned,
		current.WorkspaceID, current.ID, sourceTaskID, payload,
		CoordinationAssignmentExecutor, ownerType, ownerID,
	)
}

// RecordReviewerReassignment is the explicit reviewer handoff producer. The
// issue writer remains responsible for changing issue.reviewer_id; this method
// only records the durable dispatch obligation for that already-committed
// decision.
func (s *AgentCoordinationService) RecordReviewerReassignment(ctx context.Context, issue db.Issue, sourceTaskID pgtype.UUID) error {
	err := s.runInTx(ctx, func(qtx *db.Queries) error {
		return s.RecordReviewerReassignmentTx(ctx, qtx, issue, sourceTaskID)
	})
	if err == nil {
		s.Wake()
	}
	return err
}

func (s *AgentCoordinationService) RecordReviewerReassignmentTx(ctx context.Context, qtx *db.Queries, issue db.Issue, sourceTaskID pgtype.UUID) error {
	current, err := qtx.GetIssueInWorkspace(ctx, db.GetIssueInWorkspaceParams{ID: issue.ID, WorkspaceID: issue.WorkspaceID})
	if err != nil {
		return fmt.Errorf("reviewer reassignment: load issue: %w", err)
	}
	payload, err := coordinationPayloadFromSourceTask(ctx, qtx, current.WorkspaceID, current.ID, sourceTaskID)
	if err != nil {
		return fmt.Errorf("reviewer reassignment: load source task: %w", err)
	}
	reviewerType, reviewerID := coordinationIssueOwner(current, CoordinationAssignmentReviewer)
	if reviewerType == "" || !reviewerID.Valid {
		return fmt.Errorf("reviewer reassignment: current reviewer is missing")
	}
	payload.AssignmentRole = CoordinationAssignmentReviewer
	payload.FollowUp = boolPtr(true)
	payload.Outcome = "reviewer_reassigned"
	payload.SourceTaskID = util.UUIDToString(sourceTaskID)
	payload.IssueRevision = int64Ptr(current.Revision)
	payload.TriggerEvidenceKind = "reviewer_reassignment"
	payload.TriggerEvidenceRefID = util.UUIDToString(sourceTaskID)
	payload.ExplicitReviewer = true
	payload.ReviewerReassignment = true
	payload.ReviewerType = reviewerType
	payload.ReviewerID = util.UUIDToString(reviewerID)
	ownerType := ""
	ownerID := pgtype.UUID{}
	if reviewerType == "agent" {
		ownerType = reviewerType
		ownerID = reviewerID
		payload.OwnerType = ownerType
		payload.OwnerID = util.UUIDToString(ownerID)
	}
	return s.enqueueCoordinationEvent(ctx, qtx,
		"reviewer_reassigned:"+util.UUIDToString(current.ID)+":"+fmt.Sprintf("%d", current.Revision),
		CoordinationEventTaskCompleted,
		current.WorkspaceID, current.ID, sourceTaskID, payload,
		CoordinationAssignmentReviewer, ownerType, ownerID,
	)
}

// RecordReviewerTaskCancelledTx implements only the durable reviewer retry
// case. An executor cancellation is not silently converted into a reviewer
// event, and a queued reviewer task with no current reviewer is not retried from guessed
// state.
func (s *AgentCoordinationService) RecordReviewerTaskCancelledTx(ctx context.Context, qtx *db.Queries, task db.AgentTaskQueue) error {
	if !coordinationTaskEligible(task) || task.Status != "cancelled" {
		return nil
	}
	taskContext, present, err := decodeCoordinationTaskContext(task.Context)
	if err != nil || !present {
		return err
	}
	assignmentID, err := util.ParseUUID(taskContext.AssignmentID)
	if err != nil {
		return fmt.Errorf("reviewer cancellation: assignment id: %w", err)
	}
	agent, issue, err := coordinationIssueForTask(ctx, qtx, task)
	if err != nil {
		return err
	}
	assignment, err := qtx.GetAgentCoordinationAssignmentForTask(ctx, db.GetAgentCoordinationAssignmentForTaskParams{
		WorkspaceID:  agent.WorkspaceID,
		IssueID:      issue.ID,
		TaskID:       task.ID,
		AssignmentID: assignmentID,
	})
	if errors.Is(err, pgx.ErrNoRows) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("reviewer cancellation: load assignment: %w", err)
	}
	if assignment.Role != CoordinationAssignmentReviewer || !sameCoordinationUUID(assignment.ID, assignmentID) || assignment.Status != "dispatched" || !sameCoordinationUUID(assignment.DispatchedTaskID, task.ID) {
		return nil
	}
	changed, err := qtx.CompleteAgentCoordinationAssignmentForTask(ctx, db.CompleteAgentCoordinationAssignmentForTaskParams{
		AssignmentID: assignment.ID, TaskID: task.ID, AgentID: task.AgentID, WorkspaceID: agent.WorkspaceID, IssueID: issue.ID,
	})
	if err != nil {
		return fmt.Errorf("reviewer cancellation: close assignment: %w", err)
	}
	if changed != 1 {
		return nil
	}
	if !sameCoordinationUUID(issue.ReviewerID, task.AgentID) || coordinationText(issue.ReviewerType) != "agent" {
		// The reviewer changed before cancellation was observed. The exact
		// dispatched assignment is closed, while the issue's new reviewer (if
		// any) is handled by its own reassignment producer.
		return nil
	}
	payload := coordinationEventPayload{
		AssignmentRole:       CoordinationAssignmentReviewer,
		OwnerType:            "agent",
		OwnerID:              util.UUIDToString(task.AgentID),
		FollowUp:             boolPtr(true),
		Outcome:              "reviewer_task_cancelled",
		SourceTaskID:         util.UUIDToString(task.ID),
		IssueRevision:        int64Ptr(issue.Revision),
		OriginatorUserID:     util.UUIDToString(task.OriginatorUserID),
		AccountableUserID:    util.UUIDToString(task.AccountableUserID),
		OriginatorSource:     coordinationText(task.OriginatorSource),
		TriggerEvidenceKind:  "reviewer_task_cancelled",
		TriggerEvidenceRefID: util.UUIDToString(task.ID),
		TeamID:               util.UUIDToString(task.TeamID),
	}
	return s.enqueueCoordinationEvent(ctx, qtx,
		"reviewer_task_cancelled:"+util.UUIDToString(task.ID),
		CoordinationEventTaskCompleted,
		issue.WorkspaceID, issue.ID, task.ID, payload,
		CoordinationAssignmentReviewer, "agent", task.AgentID,
	)
}

func (s *AgentCoordinationService) enqueueCoordinationEvent(
	ctx context.Context,
	qtx *db.Queries,
	eventKey, eventType string,
	workspaceID, issueID, sourceTaskID pgtype.UUID,
	payload coordinationEventPayload,
	role, ownerType string,
	ownerID pgtype.UUID,
) error {
	if eventType != CoordinationEventTaskCompleted && eventType != CoordinationEventReviewReturned {
		return fmt.Errorf("coordination: unsupported event type %q", eventType)
	}
	if role != "" && role != CoordinationAssignmentReviewer && role != CoordinationAssignmentExecutor {
		return fmt.Errorf("coordination: unsupported assignment role %q", role)
	}
	if eventKey == "" || !workspaceID.Valid || !issueID.Valid {
		return fmt.Errorf("coordination: event scope is incomplete")
	}
	if payload.OwnerType == "" {
		payload.OwnerType = ownerType
	}
	if payload.OwnerID == "" {
		payload.OwnerID = util.UUIDToString(ownerID)
	}
	payloadJSON, err := json.Marshal(payload)
	if err != nil {
		return fmt.Errorf("coordination: marshal event payload: %w", err)
	}
	event, err := qtx.EnqueueAgentCoordinationEvent(ctx, db.EnqueueAgentCoordinationEventParams{
		EventKey:     eventKey,
		WorkspaceID:  workspaceID,
		IssueID:      issueID,
		SourceTaskID: sourceTaskID,
		EventType:    eventType,
		Payload:      payloadJSON,
	})
	if errors.Is(err, pgx.ErrNoRows) {
		return fmt.Errorf("coordination: event idempotency metadata mismatch")
	}
	if err != nil {
		return fmt.Errorf("coordination: enqueue event: %w", err)
	}
	if role == "" {
		return nil
	}
	assignment, err := qtx.UpsertAgentCoordinationAssignment(ctx, db.UpsertAgentCoordinationAssignmentParams{
		EventID:      event.ID,
		WorkspaceID:  workspaceID,
		IssueID:      issueID,
		SourceTaskID: sourceTaskID,
		Role:         role,
	})
	if errors.Is(err, pgx.ErrNoRows) {
		return fmt.Errorf("coordination: assignment idempotency metadata mismatch")
	}
	if err != nil {
		return fmt.Errorf("coordination: upsert assignment: %w", err)
	}
	if ownerType == "agent" && ownerID.Valid && (assignment.Status == "pending" || assignment.Status == "assigned") {
		changed, err := qtx.SetAgentCoordinationAssignmentOwner(ctx, db.SetAgentCoordinationAssignmentOwnerParams{
			EventKey: eventKey, WorkspaceID: workspaceID, IssueID: issueID, Role: role, OwnerID: ownerID,
		})
		if err != nil {
			return fmt.Errorf("coordination: preassign owner: %w", err)
		}
		if changed != 1 {
			return fmt.Errorf("coordination: preassign owner fence rejected")
		}
	}
	return nil
}

func (s *AgentCoordinationService) processClaim(ctx context.Context, event db.AgentCoordinationOutbox) (db.AgentTaskQueue, bool, error) {
	var createdTask db.AgentTaskQueue
	var created bool
	var reviewPublication *coordinationReviewPublication
	err := s.runInTx(ctx, func(qtx *db.Queries) error {
		payload, payloadErr := decodeCoordinationEventPayload(event.Payload)
		if payloadErr != nil {
			return s.retryClaimWithoutAssignment(ctx, qtx, event, payloadErr.Error(), coordinationTransientRetry)
		}
		handleIssueLoadError := func(message string) error {
			// Preserve the existing assignment defer semantics for a malformed
			// pre-existing orphan. A real issue delete removes both rows before
			// commit, so this lookup normally finds nothing and the lease fence
			// prevents the stale claimed event from being retried forever.
			assignment, assignmentErr := qtx.GetAgentCoordinationAssignmentForLease(ctx, db.GetAgentCoordinationAssignmentForLeaseParams{
				EventID: event.ID, WorkspaceID: event.WorkspaceID, IssueID: event.IssueID, LeaseOwner: event.LeaseOwner,
			})
			if assignmentErr == nil {
				return s.deferClaim(ctx, qtx, event, assignment, message)
			}
			if !errors.Is(assignmentErr, pgx.ErrNoRows) {
				message += "; load assignment: " + assignmentErr.Error()
			}
			return s.retryClaimWithoutAssignment(ctx, qtx, event, message, coordinationTransientRetry)
		}
		issue, issueErr := qtx.LockAgentCoordinationIssue(ctx, db.LockAgentCoordinationIssueParams{
			IssueID: event.IssueID, WorkspaceID: event.WorkspaceID,
		})
		if errors.Is(issueErr, pgx.ErrNoRows) {
			// A claimed event can race an issue deletion. The delete transaction
			// removes this event atomically with the issue; if this claim already
			// owns an older snapshot, the lease update below will fence it out.
			// Keep the existing retry path for a pre-existing orphan and do not
			// terminally discard work from the worker side.
			return handleIssueLoadError("load issue: " + issueErr.Error())
		}
		if issueErr != nil {
			return handleIssueLoadError("load issue: " + issueErr.Error())
		}
		assignment, err := qtx.GetAgentCoordinationAssignmentForLease(ctx, db.GetAgentCoordinationAssignmentForLeaseParams{
			EventID: event.ID, WorkspaceID: event.WorkspaceID, IssueID: event.IssueID, LeaseOwner: event.LeaseOwner,
		})
		if errors.Is(err, pgx.ErrNoRows) {
			if !coordinationFollowUp(payload) {
				return s.completeClaimedOutbox(ctx, qtx, event)
			}
			return s.retryClaimWithoutAssignment(ctx, qtx, event, "assignment row is missing", coordinationTransientRetry)
		}
		if err != nil {
			return s.retryClaimWithoutAssignment(ctx, qtx, event, "load assignment: "+err.Error(), coordinationTransientRetry)
		}
		if !coordinationFollowUp(payload) || assignment.Status == "completed" {
			return s.completeClaimedOutbox(ctx, qtx, event)
		}
		if payload.AssignmentRole != "" && payload.AssignmentRole != assignment.Role {
			return s.deferClaim(ctx, qtx, event, assignment, "assignment role does not match event payload")
		}
		if assignment.Status == "blocked" {
			// A blocked assignment is an explicit durable decision. Keep the
			// outbox retryable, but do not silently unblock it by dispatching a
			// child task; another writer may later move it back to assigned.
			return s.retryClaim(ctx, qtx, event, "assignment is blocked", coordinationTransientRetry)
		}

		// An implementation completion creates a reviewer assignment, but the
		// reviewer may be a human/team role rather than a concrete agent. Resolve
		// that role while the coordination event is leased, lock the selected agent,
		// and atomically move the issue into review before dispatching the reviewer
		// task. This is the durable Rust handoff boundary; dispatching directly from
		// the member/team placeholder would leave the event retrying forever.
		if event.EventType == CoordinationEventTaskCompleted &&
			payload.AssignmentRole != CoordinationAssignmentReviewer &&
			assignment.Role == CoordinationAssignmentReviewer {
			category := issuestatus.Effective(ctx, qtx, issue.WorkspaceID, issue.Status)
			if category != issuestatus.InProgress {
				return s.completeClaimedAssignment(ctx, qtx, event, assignment, map[string]any{
					"outcome": "ignored",
					"reason":  "issue_not_in_progress",
				}, "")
			}

			if !issueExecutorCanCoordinate(issue) {
				return s.completeClaimedAssignment(ctx, qtx, event, assignment, map[string]any{
					"outcome": "blocked",
					"reason":  "implementation_owner_not_agent_or_team",
				}, "implementation owner is not an agent or team")
			}

			previousIssue := issue
			reviewerID := pgtype.UUID{}
			if assignment.OwnerType.Valid && assignment.OwnerType.String == "agent" && assignment.OwnerID.Valid {
				reviewerID = assignment.OwnerID
			} else if coordinationText(issue.ReviewerType) == "agent" && issue.ReviewerID.Valid {
				reviewerID = issue.ReviewerID
			}
			explicitReviewerType := coordinationText(issue.ReviewerType)
			if explicitReviewerType != "" && explicitReviewerType != "agent" {
				return s.deferClaim(ctx, qtx, event, assignment, "explicit reviewer is not an agent or is unavailable")
			}

			teamID := pgtype.UUID{}
			if coordinationText(issue.ExecutorType) == "team" {
				teamID = issue.ExecutorID
			}
			candidate, err := qtx.SelectCoordinationReviewer(ctx, db.SelectCoordinationReviewerParams{
				WorkspaceID:   issue.WorkspaceID,
				ReviewerID:    reviewerID,
				SourceAgentID: optionalUUID(payload.AgentID),
				TeamID:        teamID,
				AssignmentID:  assignment.ID,
			})
			if errors.Is(err, pgx.ErrNoRows) {
				return s.deferClaim(ctx, qtx, event, assignment, "no reviewer with role=reviewer and a bound runtime")
			}
			if err != nil {
				return s.deferClaim(ctx, qtx, event, assignment, "select reviewer: "+err.Error())
			}

			updated, err := qtx.UpdateIssueForCoordinationReview(ctx, db.UpdateIssueForCoordinationReviewParams{
				ReviewerID:       candidate.ID,
				IssueID:          issue.ID,
				WorkspaceID:      issue.WorkspaceID,
				ExpectedRevision: issue.Revision,
			})
			if errors.Is(err, pgx.ErrNoRows) {
				return s.retryClaim(ctx, qtx, event, "issue changed before reviewer handoff", coordinationTransientRetry)
			}
			if err != nil {
				return fmt.Errorf("coordination: promote issue to review: %w", err)
			}

			publicationKey := coordinationIssueUpdatePublicationKey(
				"review_handoff", updated, previousIssue.ReviewerType, previousIssue.ReviewerID,
			)
			decision, err := json.Marshal(map[string]any{
				"policy":                         map[bool]string{true: "team_reviewer_role", false: "workspace_reviewer_role"}[teamID.Valid],
				"role":                           CoordinationAssignmentReviewer,
				"review_publication":             "review_handoff",
				"issue_update_publication_key":   publicationKey,
				"assignment_activity_published": false,
				"candidate_agent_id":             util.UUIDToString(candidate.ID),
				"candidate_agent_name":           candidate.Name,
				"explicit_reviewer":              explicitReviewerType != "",
				"previous_status":                previousIssue.Status,
				"previous_executor_type":         coordinationText(previousIssue.ExecutorType),
				"previous_executor_id":           util.UUIDToString(previousIssue.ExecutorID),
				"previous_reviewer_type":           coordinationText(previousIssue.ReviewerType),
				"previous_reviewer_id":           util.UUIDToString(previousIssue.ReviewerID),
			})
			if err != nil {
				return fmt.Errorf("coordination: encode reviewer decision: %w", err)
			}
			changed, err := qtx.AssignAgentCoordinationAssignmentForLease(ctx, db.AssignAgentCoordinationAssignmentForLeaseParams{
				AssignmentID: assignment.ID,
				EventID:      event.ID,
				WorkspaceID:  event.WorkspaceID,
				IssueID:      event.IssueID,
				OwnerID:      candidate.ID,
				Decision:     decision,
				LeaseOwner:   event.LeaseOwner,
			})
			if err != nil {
				return fmt.Errorf("coordination: record reviewer decision: %w", err)
			}
			if changed != 1 {
				return ErrCoordinationLeaseLost
			}
			assignment.OwnerType = pgtype.Text{String: "agent", Valid: true}
			assignment.OwnerID = candidate.ID
			assignment.Status = "assigned"
			assignment.Decision = decision
			issue = updated

			audit, err := json.Marshal(map[string]any{
				"event_id":       util.UUIDToString(event.ID),
				"assignment_id":  util.UUIDToString(assignment.ID),
				"source_task_id": util.UUIDToString(event.SourceTaskID),
				"role":           CoordinationAssignmentReviewer,
				"owner_type":     "agent",
				"owner_id":       util.UUIDToString(candidate.ID),
				"reason":         "implementation task completed",
			})
			if err != nil {
				return fmt.Errorf("coordination: encode reviewer activity: %w", err)
			}
			activity, err := qtx.CreateActivity(ctx, db.CreateActivityParams{
				ID:          dbid.NewV7(),
				WorkspaceID: issue.WorkspaceID,
				IssueID:     issue.ID,
				ActorType:   pgtype.Text{String: "system", Valid: true},
				Action:      "coordinator_assignment",
				Details:     audit,
			})
			if err != nil {
				return fmt.Errorf("coordination: record reviewer activity: %w", err)
			}
			reviewPublication = &coordinationReviewPublication{
				eventID:        event.ID,
				publicationKey: publicationKey,
				previous:       previousIssue,
				updated:        updated,
				activity:       activity,
			}
		}

		ownerType, ownerID, ownerErr := coordinationDispatchOwner(event, payload, assignment, issue)
		if ownerErr != nil {
			return s.deferClaim(ctx, qtx, event, assignment, ownerErr.Error())
		}
		if (ownerType != "agent" && ownerType != "team") || !ownerID.Valid {
			return s.deferClaim(ctx, qtx, event, assignment, "owner is not an agent or team")
		}
		if assignment.Status != "dispatched" && !coordinationOwnerMatchesIssue(issue, assignment.Role, ownerType, ownerID, payload.OwnerGeneration) {
			if ownerType == "agent" && ownerID.Valid {
				changed, completeErr := qtx.CompleteAgentCoordinationAssignmentForLease(ctx, db.CompleteAgentCoordinationAssignmentForLeaseParams{
					AssignmentID: assignment.ID, EventID: event.ID, WorkspaceID: event.WorkspaceID, IssueID: event.IssueID,
					Status: "completed", LeaseOwner: event.LeaseOwner,
				})
				if completeErr != nil {
					return completeErr
				}
				if changed != 1 {
					return ErrCoordinationLeaseLost
				}
				return s.completeClaimedOutbox(ctx, qtx, event)
			}
			return s.deferClaim(ctx, qtx, event, assignment, "owner decision is stale for the current issue assignment")
		}
		if assignment.Status == "dispatched" && (assignment.OwnerType.String != ownerType || !sameCoordinationUUID(assignment.OwnerID, ownerID)) {
			return s.deferClaim(ctx, qtx, event, assignment, "dispatched assignment owner changed")
		}
		dispatchAgentID := ownerID
		teamID := optionalUUID(payload.TeamID)
		if ownerType == "team" {
			team, teamErr := qtx.GetTeamInWorkspace(ctx, db.GetTeamInWorkspaceParams{
				ID:          ownerID,
				WorkspaceID: event.WorkspaceID,
			})
			if errors.Is(teamErr, pgx.ErrNoRows) {
				return s.deferClaim(ctx, qtx, event, assignment, "executor team is missing or archived")
			}
			if teamErr != nil {
				return s.deferClaim(ctx, qtx, event, assignment, "load executor team: "+teamErr.Error())
			}
			dispatchAgentID = team.LeaderID
			teamID = team.ID
		}
		target, err := qtx.GetCoordinationAgentForDispatch(ctx, db.GetCoordinationAgentForDispatchParams{
			AgentID: dispatchAgentID, WorkspaceID: event.WorkspaceID, RuntimeStaleSeconds: coordinationRuntimeStaleAfter.Seconds(),
		})
		if errors.Is(err, pgx.ErrNoRows) {
			return s.deferClaim(ctx, qtx, event, assignment, "agent is archived, rebound, offline, or stale")
		}
		if err != nil {
			return s.deferClaim(ctx, qtx, event, assignment, "load dispatch agent: "+err.Error())
		}

		if assignment.Status == "pending" || assignment.Status == "assigned" {
			decision, marshalErr := json.Marshal(map[string]any{
				"event_key":  event.EventKey,
				"owner_type": ownerType,
				"owner_id":   util.UUIDToString(ownerID),
			})
			if marshalErr != nil {
				return s.deferClaim(ctx, qtx, event, assignment, "marshal owner decision: "+marshalErr.Error())
			}
			var (
				changed   int64
				updateErr error
			)
			if ownerType == "team" {
				changed, updateErr = qtx.AssignTeamCoordinationAssignmentForLease(ctx, db.AssignTeamCoordinationAssignmentForLeaseParams{
					AssignmentID: assignment.ID, EventID: event.ID, WorkspaceID: event.WorkspaceID, IssueID: event.IssueID,
					TeamID: ownerID, Decision: decision, LeaseOwner: event.LeaseOwner,
				})
			} else {
				changed, updateErr = qtx.AssignAgentCoordinationAssignmentForLease(ctx, db.AssignAgentCoordinationAssignmentForLeaseParams{
					AssignmentID: assignment.ID, EventID: event.ID, WorkspaceID: event.WorkspaceID, IssueID: event.IssueID,
					OwnerID: ownerID, Decision: decision, LeaseOwner: event.LeaseOwner,
				})
			}
			if updateErr != nil {
				return updateErr
			}
			if changed != 1 {
				return ErrCoordinationLeaseLost
			}
			assignment.OwnerType = pgtype.Text{String: ownerType, Valid: true}
			assignment.OwnerID = ownerID
			assignment.Status = "assigned"
		}

		activeTask, activeErr := qtx.GetActiveCoordinationTask(ctx, db.GetActiveCoordinationTaskParams{
			WorkspaceID: event.WorkspaceID, IssueID: event.IssueID, AgentID: dispatchAgentID, AssignmentID: assignment.ID,
		})
		if errors.Is(activeErr, pgx.ErrNoRows) {
			contextJSON, contextErr := marshalCoordinationTaskContext(assignment, issue)
			if contextErr != nil {
				return s.deferClaim(ctx, qtx, event, assignment, "marshal task provenance: "+contextErr.Error())
			}
			triggerEvidenceKind := optionalText(payload.TriggerEvidenceKind)
			if !triggerEvidenceKind.Valid {
				triggerEvidenceKind = optionalText("coordination_event")
			}
			triggerEvidenceRefID := optionalUUID(payload.TriggerEvidenceRefID)
			if !triggerEvidenceRefID.Valid {
				triggerEvidenceRefID = event.ID
			}
			activeTask, err = qtx.CreateCoordinationAgentTask(ctx, db.CreateCoordinationAgentTaskParams{
				ID:                   dbid.NewV7(),
				AgentID:              dispatchAgentID,
				RuntimeID:            target.RuntimeID,
				IssueID:              event.IssueID,
				WorkspaceID:          event.WorkspaceID,
				Priority:             priorityToInt(issue.Priority),
				Context:              contextJSON,
				HandoffNote:          optionalText(payload.HandoffNote),
				TeamID:               teamID,
				OriginatorUserID:     optionalUUID(payload.OriginatorUserID),
				AccountableUserID:    optionalUUID(payload.AccountableUserID),
				OriginatorSource:     optionalText(payload.OriginatorSource),
				TriggerEvidenceKind:  triggerEvidenceKind,
				TriggerEvidenceRefID: triggerEvidenceRefID,
			})
			if errors.Is(err, pgx.ErrNoRows) {
				return s.deferClaim(ctx, qtx, event, assignment, "task owner fence rejected")
			}
			if err != nil {
				return s.deferClaim(ctx, qtx, event, assignment, "create coordination task: "+err.Error())
			}
			created = true
		} else if activeErr != nil {
			return s.deferClaim(ctx, qtx, event, assignment, "load active coordination task: "+activeErr.Error())
		}

		decision, marshalErr := json.Marshal(map[string]any{
			"event_key": event.EventKey,
			"task_id":   util.UUIDToString(activeTask.ID),
			"agent_id":  util.UUIDToString(dispatchAgentID),
		})
		if marshalErr != nil {
			return marshalErr
		}
		changed, err := qtx.CompleteAgentCoordinationAssignmentForLease(ctx, db.CompleteAgentCoordinationAssignmentForLeaseParams{
			AssignmentID: assignment.ID, EventID: event.ID, WorkspaceID: event.WorkspaceID, IssueID: event.IssueID,
			Status: "dispatched", DispatchedTaskID: activeTask.ID, Decision: decision, LeaseOwner: event.LeaseOwner,
		})
		if err != nil {
			return err
		}
		if changed != 1 {
			return ErrCoordinationLeaseLost
		}
		if err := s.completeClaimedOutbox(ctx, qtx, event); err != nil {
			return err
		}
		createdTask = activeTask
		return nil
	})
	if err == nil && reviewPublication != nil {
		s.publishCoordinationReviewHandoff(ctx, *reviewPublication)
	}
	return createdTask, created, err
}

func (s *AgentCoordinationService) completeClaimedOutbox(ctx context.Context, qtx *db.Queries, event db.AgentCoordinationOutbox) error {
	_, err := qtx.CompleteAgentCoordinationOutbox(ctx, db.CompleteAgentCoordinationOutboxParams{
		ID: event.ID, LeaseOwner: event.LeaseOwner,
	})
	if errors.Is(err, pgx.ErrNoRows) {
		return ErrCoordinationLeaseLost
	}
	return err
}

func (s *AgentCoordinationService) completeClaimedAssignment(
	ctx context.Context,
	qtx *db.Queries,
	event db.AgentCoordinationOutbox,
	assignment db.AgentCoordinationAssignment,
	decision map[string]any,
	lastError string,
) error {
	decisionJSON, err := json.Marshal(decision)
	if err != nil {
		return fmt.Errorf("coordination: encode terminal decision: %w", err)
	}
	changed, err := qtx.CompleteAgentCoordinationAssignmentForLease(ctx, db.CompleteAgentCoordinationAssignmentForLeaseParams{
		AssignmentID: assignment.ID,
		EventID:      event.ID,
		WorkspaceID:  event.WorkspaceID,
		IssueID:      event.IssueID,
		Status:       "completed",
		Decision:     decisionJSON,
		LastError:    optionalText(lastError),
		LeaseOwner:   event.LeaseOwner,
	})
	if err != nil {
		return fmt.Errorf("coordination: complete assignment: %w", err)
	}
	if changed != 1 {
		return ErrCoordinationLeaseLost
	}
	return s.completeClaimedOutbox(ctx, qtx, event)
}

func (s *AgentCoordinationService) publishCoordinationReviewHandoff(ctx context.Context, publication coordinationReviewPublication) {
	if s == nil || s.Tasks == nil || s.Tasks.Bus == nil {
		return
	}
	workspaceID := util.UUIDToString(publication.updated.WorkspaceID)
	issuePayload := IssueToMapResolved(ctx, s.Tasks.Queries, publication.updated, s.Tasks.getIssuePrefix(publication.updated.WorkspaceID))
	payload := map[string]any{
		"issue":                         issuePayload,
		"executor_changed":              false,
		"status_changed":                true,
		"review_handoff":                true,
		"coordination_publication":      "review_handoff",
		"coordination_publication_key":  publication.publicationKey,
		"coordination_event_id":         util.UUIDToString(publication.eventID),
		"priority_changed":              false,
		"project_changed":               false,
		"start_date_changed":            false,
		"due_date_changed":              false,
		"description_changed":           false,
		"title_changed":                 false,
		"prev_status":                   publication.previous.Status,
		"prev_executor_type":            util.TextToPtr(publication.previous.ExecutorType),
		"prev_executor_id":              util.UUIDToPtr(publication.previous.ExecutorID),
		"prev_reviewer_type":            util.TextToPtr(publication.previous.ReviewerType),
		"prev_reviewer_id":              util.UUIDToPtr(publication.previous.ReviewerID),
	}
	s.Tasks.Bus.Publish(events.Event{
		Type:        protocol.EventIssueUpdated,
		WorkspaceID: workspaceID,
		ActorType:   "system",
		Payload:     payload,
	})

	activity := publication.activity
	if !activity.ID.Valid {
		return
	}
	actorType := coordinationText(activity.ActorType)
	s.Tasks.Bus.Publish(events.Event{
		Type:        protocol.EventActivityCreated,
		WorkspaceID: workspaceID,
		ActorType:   "system",
		Payload: map[string]any{
			"issue_id": util.UUIDToString(activity.IssueID),
			"entry": map[string]any{
				"type":       "activity",
				"id":         util.UUIDToString(activity.ID),
				"actor_type": actorType,
				"actor_id":   util.UUIDToString(activity.ActorID),
				"action":     activity.Action,
				"details":    json.RawMessage(activity.Details),
				"created_at": util.TimestampToString(activity.CreatedAt),
			},
		},
	})
}

func coordinationIssueUpdatePublicationKey(publication string, issue db.Issue, previousReviewerType pgtype.Text, previousReviewerID pgtype.UUID) string {
	return fmt.Sprintf("%s:%d:%s:%s:%s:%s",
		publication,
		issue.Revision,
		coordinationText(previousReviewerType),
		util.UUIDToString(previousReviewerID),
		coordinationText(issue.ReviewerType),
		util.UUIDToString(issue.ReviewerID),
	)
}

func issueExecutorCanCoordinate(issue db.Issue) bool {
	ownerType := coordinationText(issue.ExecutorType)
	return (ownerType == "agent" || ownerType == "team") && issue.ExecutorID.Valid
}

func (s *AgentCoordinationService) retryClaimWithoutAssignment(ctx context.Context, qtx *db.Queries, event db.AgentCoordinationOutbox, message string, delay time.Duration) error {
	return s.retryClaim(ctx, qtx, event, message, delay)
}

func (s *AgentCoordinationService) deferClaim(ctx context.Context, qtx *db.Queries, event db.AgentCoordinationOutbox, assignment db.AgentCoordinationAssignment, message string) error {
	if _, err := qtx.DeferAgentCoordinationAssignmentForLease(ctx, db.DeferAgentCoordinationAssignmentForLeaseParams{
		AssignmentID: assignment.ID, EventID: event.ID, WorkspaceID: event.WorkspaceID, IssueID: event.IssueID,
		LastError: coordinationErrorText(message), LeaseOwner: event.LeaseOwner,
	}); err != nil {
		return err
	}
	return s.retryClaim(ctx, qtx, event, message, coordinationRetryFor(message))
}

func (s *AgentCoordinationService) retryClaim(ctx context.Context, qtx *db.Queries, event db.AgentCoordinationOutbox, message string, delay time.Duration) error {
	changed, err := qtx.RetryAgentCoordinationOutbox(ctx, db.RetryAgentCoordinationOutboxParams{
		ID: event.ID, LeaseOwner: event.LeaseOwner, DelaySeconds: delay.Seconds(), LastError: coordinationErrorText(message),
	})
	if err != nil {
		return err
	}
	if changed != 1 {
		return ErrCoordinationLeaseLost
	}
	return nil
}

func (s *AgentCoordinationService) runInTx(ctx context.Context, fn func(*db.Queries) error) error {
	if s.Queries == nil {
		return errors.New("agent coordination: queries are nil")
	}
	if s.TxStarter == nil {
		return errors.New("agent coordination: transaction starter is required")
	}
	tx, err := s.TxStarter.Begin(ctx)
	if err != nil {
		return fmt.Errorf("agent coordination: begin tx: %w", err)
	}
	defer tx.Rollback(ctx)
	if err := fn(s.Queries.WithTx(tx)); err != nil {
		return err
	}
	return tx.Commit(ctx)
}

func decodeCoordinationTaskContext(raw []byte) (coordinationTaskContext, bool, error) {
	if len(raw) == 0 {
		return coordinationTaskContext{}, false, nil
	}
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(raw, &fields); err != nil {
		// Existing non-coordination tasks may carry opaque context. They do
		// not become a completion blocker merely because it is malformed.
		return coordinationTaskContext{}, false, nil
	}
	assignmentRaw, ok := fields[coordinationAssignmentIDContextKey]
	if !ok || string(assignmentRaw) == "null" {
		return coordinationTaskContext{}, false, nil
	}
	var result coordinationTaskContext
	if err := json.Unmarshal(raw, &result); err != nil {
		return coordinationTaskContext{}, true, fmt.Errorf("decode coordination context: %w", err)
	}
	if strings.TrimSpace(result.AssignmentID) == "" {
		return coordinationTaskContext{}, true, errors.New("coordination context has an empty assignment id")
	}
	return result, true, nil
}

func marshalCoordinationTaskContext(assignment db.AgentCoordinationAssignment, issue db.Issue) ([]byte, error) {
	if !assignment.ID.Valid || !assignment.OwnerID.Valid {
		return nil, errors.New("assignment has no durable id or owner")
	}
	result := coordinationTaskContext{
		AssignmentID:   util.UUIDToString(assignment.ID),
		AssignmentRole: assignment.Role,
		OwnerType:      coordinationText(assignment.OwnerType),
		OwnerID:        util.UUIDToString(assignment.OwnerID),
		IssueRevision:  int64Ptr(issue.Revision),
	}
	if assignment.Role == CoordinationAssignmentExecutor && sameCoordinationUUID(assignment.OwnerID, issue.ExecutorID) {
		result.OwnerGeneration = int64Ptr(issue.ExecutorGeneration)
	}
	return json.Marshal(result)
}

func coordinationTaskEligible(task db.AgentTaskQueue) bool {
	return task.IssueID.Valid && !task.ChatSessionID.Valid && !isCoordinationSideChat(task.Context)
}

func isCoordinationSideChat(raw []byte) bool {
	if len(raw) == 0 {
		return false
	}
	var fields map[string]json.RawMessage
	if json.Unmarshal(raw, &fields) != nil {
		return false
	}
	for _, key := range []string{coordinationSideChatParentTaskKey, coordinationSideChatRootCommentKey} {
		value, ok := fields[key]
		if !ok || string(value) == "null" {
			continue
		}
		var marker string
		if json.Unmarshal(value, &marker) == nil && strings.TrimSpace(marker) != "" {
			return true
		}
	}
	return false
}

func coordinationIssueForTask(ctx context.Context, qtx *db.Queries, task db.AgentTaskQueue) (db.Agent, db.Issue, error) {
	agent, err := qtx.GetAgent(ctx, task.AgentID)
	if err != nil {
		return db.Agent{}, db.Issue{}, fmt.Errorf("coordination: load task agent: %w", err)
	}
	issue, err := qtx.GetIssueInWorkspace(ctx, db.GetIssueInWorkspaceParams{ID: task.IssueID, WorkspaceID: agent.WorkspaceID})
	if err != nil {
		return db.Agent{}, db.Issue{}, fmt.Errorf("coordination: load task issue: %w", err)
	}
	return agent, issue, nil
}

func coordinationPayloadFromSourceTask(ctx context.Context, qtx *db.Queries, workspaceID, issueID, sourceTaskID pgtype.UUID) (coordinationEventPayload, error) {
	if !sourceTaskID.Valid {
		return coordinationEventPayload{}, nil
	}
	sourceTask, err := qtx.GetAgentTaskInWorkspace(ctx, db.GetAgentTaskInWorkspaceParams{
		ID: sourceTaskID, WorkspaceID: workspaceID,
	})
	if err != nil {
		return coordinationEventPayload{}, err
	}
	if !sameCoordinationUUID(sourceTask.IssueID, issueID) {
		return coordinationEventPayload{}, errors.New("source task is not bound to the issue")
	}
	return coordinationTaskPayload(sourceTask), nil
}

func coordinationAssignmentMatchesTask(assignment db.AgentCoordinationAssignment, task db.AgentTaskQueue, taskContext coordinationTaskContext) bool {
	assignmentID, err := util.ParseUUID(taskContext.AssignmentID)
	if err != nil || !sameCoordinationUUID(assignment.ID, assignmentID) {
		return false
	}
	if assignment.Role != taskContext.AssignmentRole && taskContext.AssignmentRole != "" {
		return false
	}
	if !assignment.OwnerType.Valid || !sameCoordinationUUID(assignment.DispatchedTaskID, task.ID) || (assignment.Status != "assigned" && assignment.Status != "dispatched" && assignment.Status != "completed") {
		return false
	}
	if assignment.OwnerType.String == "agent" {
		return sameCoordinationUUID(assignment.OwnerID, task.AgentID)
	}
	ownerID, err := util.ParseUUID(taskContext.OwnerID)
	return assignment.OwnerType.String == "team" && taskContext.OwnerType == "team" && err == nil && sameCoordinationUUID(assignment.OwnerID, ownerID)
}

func coordinationCompletionStillOwnsIssue(issue db.Issue, role string, taskContext coordinationTaskContext, agentID pgtype.UUID) bool {
	ownerID, err := util.ParseUUID(taskContext.OwnerID)
	if err != nil {
		return false
	}
	switch role {
	case CoordinationAssignmentExecutor:
		if coordinationText(taskContext.OwnerType) == "team" {
			if coordinationText(issue.ExecutorType) != "team" || !sameCoordinationUUID(issue.ExecutorID, ownerID) {
				return false
			}
			return taskContext.OwnerGeneration == nil || *taskContext.OwnerGeneration == issue.ExecutorGeneration
		}
		if !sameCoordinationUUID(ownerID, agentID) || coordinationText(issue.ExecutorType) != "agent" || !sameCoordinationUUID(issue.ExecutorID, agentID) {
			return false
		}
		return taskContext.OwnerGeneration == nil || *taskContext.OwnerGeneration == issue.ExecutorGeneration
	case CoordinationAssignmentReviewer:
		return sameCoordinationUUID(ownerID, agentID) && coordinationText(issue.ReviewerType) == "agent" && sameCoordinationUUID(issue.ReviewerID, agentID)
	default:
		return false
	}
}

func coordinationCompletionPayload(task db.AgentTaskQueue, assignment db.AgentCoordinationAssignment, issue db.Issue, taskContext coordinationTaskContext, followUp bool, outcome string) coordinationEventPayload {
	payload := coordinationTaskPayload(task)
	payload.AssignmentID = util.UUIDToString(assignment.ID)
	payload.AssignmentRole = assignment.Role
	payload.OwnerType = taskContext.OwnerType
	payload.OwnerID = taskContext.OwnerID
	payload.OwnerGeneration = taskContext.OwnerGeneration
	payload.FollowUp = boolPtr(followUp)
	payload.Outcome = outcome
	payload.SourceTaskID = util.UUIDToString(task.ID)
	payload.IssueRevision = taskContext.IssueRevision
	if payload.IssueRevision == nil {
		payload.IssueRevision = int64Ptr(issue.Revision)
	}
	return payload
}

func coordinationTaskPayload(task db.AgentTaskQueue) coordinationEventPayload {
	return coordinationEventPayload{
		AgentID:              util.UUIDToString(task.AgentID),
		OriginatorUserID:     util.UUIDToString(task.OriginatorUserID),
		AccountableUserID:    util.UUIDToString(task.AccountableUserID),
		OriginatorSource:     coordinationText(task.OriginatorSource),
		HandoffNote:          coordinationText(task.HandoffNote),
		TeamID:               util.UUIDToString(task.TeamID),
		TriggerEvidenceKind:  coordinationText(task.TriggerEvidenceKind),
		TriggerEvidenceRefID: util.UUIDToString(task.TriggerEvidenceRefID),
	}
}

func coordinationIssueOwner(issue db.Issue, role string) (string, pgtype.UUID) {
	if role == CoordinationAssignmentReviewer {
		return coordinationText(issue.ReviewerType), issue.ReviewerID
	}
	return coordinationText(issue.ExecutorType), issue.ExecutorID
}

func coordinationOwnerMatchesIssue(issue db.Issue, role, ownerType string, ownerID pgtype.UUID, expectedGeneration *int64) bool {
	currentType, currentID := coordinationIssueOwner(issue, role)
	if role == CoordinationAssignmentReviewer && ownerType != "agent" {
		return false
	}
	if role == CoordinationAssignmentExecutor && ownerType != "agent" && ownerType != "team" {
		return false
	}
	if currentType != ownerType || !sameCoordinationUUID(currentID, ownerID) {
		return false
	}
	return role != CoordinationAssignmentExecutor || expectedGeneration == nil || *expectedGeneration == issue.ExecutorGeneration
}

func coordinationDispatchOwner(event db.AgentCoordinationOutbox, payload coordinationEventPayload, assignment db.AgentCoordinationAssignment, issue db.Issue) (string, pgtype.UUID, error) {
	if assignment.OwnerID.Valid {
		return coordinationText(assignment.OwnerType), assignment.OwnerID, nil
	}
	if payload.OwnerID != "" {
		ownerID, err := util.ParseUUID(payload.OwnerID)
		if err != nil {
			return "", pgtype.UUID{}, fmt.Errorf("event owner id: %w", err)
		}
		return strings.TrimSpace(payload.OwnerType), ownerID, nil
	}
	if event.EventType == CoordinationEventReviewReturned && assignment.Role == CoordinationAssignmentExecutor {
		ownerType, ownerID := coordinationIssueOwner(issue, CoordinationAssignmentExecutor)
		return ownerType, ownerID, nil
	}
	if assignment.Role == CoordinationAssignmentReviewer {
		ownerType, ownerID := coordinationIssueOwner(issue, CoordinationAssignmentReviewer)
		return ownerType, ownerID, nil
	}
	if assignment.Role == CoordinationAssignmentExecutor {
		ownerType, ownerID := coordinationIssueOwner(issue, CoordinationAssignmentExecutor)
		return ownerType, ownerID, nil
	}
	return "", pgtype.UUID{}, nil
}

func decodeCoordinationEventPayload(raw []byte) (coordinationEventPayload, error) {
	if len(raw) == 0 {
		return coordinationEventPayload{}, nil
	}
	var payload coordinationEventPayload
	if err := json.Unmarshal(raw, &payload); err != nil {
		return coordinationEventPayload{}, fmt.Errorf("decode coordination event payload: %w", err)
	}
	return payload, nil
}

func coordinationFollowUp(payload coordinationEventPayload) bool {
	return payload.FollowUp == nil || *payload.FollowUp
}

func coordinationRetryFor(message string) time.Duration {
	if strings.Contains(message, "offline") || strings.Contains(message, "stale") || strings.Contains(message, "archived") || strings.Contains(message, "rebound") || strings.Contains(message, "explicit agent") {
		return coordinationNoOwnerRetry
	}
	return coordinationTransientRetry
}

func coordinationErrorText(message string) string {
	message = util.SanitizeTextForPostgres(strings.TrimSpace(message))
	if len(message) > 4096 {
		return message[:4096]
	}
	return message
}

func coordinationText(value pgtype.Text) string {
	if !value.Valid {
		return ""
	}
	return strings.TrimSpace(value.String)
}

func optionalUUID(raw string) pgtype.UUID {
	if strings.TrimSpace(raw) == "" {
		return pgtype.UUID{}
	}
	value, err := util.ParseUUID(raw)
	if err != nil {
		return pgtype.UUID{}
	}
	return value
}

func optionalText(raw string) pgtype.Text {
	raw = strings.TrimSpace(raw)
	return pgtype.Text{String: raw, Valid: raw != ""}
}

func sameCoordinationUUID(left, right pgtype.UUID) bool {
	return left.Valid && right.Valid && left == right
}

func boolPtr(value bool) *bool {
	return &value
}

func int64Ptr(value int64) *int64 {
	return &value
}
