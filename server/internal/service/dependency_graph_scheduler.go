package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

// WakeDependencyGraphReadyTasks promotes the ready portion of one graph and
// admits its agent/team-owned issues into the normal task queue. Promotion is
// deliberately separate from task insertion: the graph remains the durable
// source of truth, while runtime claim recovery can replay the queue admission.
func (s *TaskService) WakeDependencyGraphReadyTasks(ctx context.Context, workspaceID, planID pgtype.UUID) error {
	if s == nil || s.Queries == nil || !workspaceID.Valid || !planID.Valid {
		return nil
	}

	var promoted []pgtype.UUID
	if err := s.runInTx(ctx, func(qtx *db.Queries) error {
		var err error
		promoted, err = qtx.PromoteReadyDependencyGraphIssuesForPlan(ctx, db.PromoteReadyDependencyGraphIssuesForPlanParams{
			WorkspaceID: workspaceID,
			PlanID:      planID,
		})
		return err
	}); err != nil {
		return fmt.Errorf("promote dependency graph tasks: %w", err)
	}

	if len(promoted) > 0 {
		s.publishDependencyGraphWakeup(workspaceID, planID, promoted)
	}
	issueIDs, err := s.Queries.ListReadyDependencyGraphIssueIDsForPlan(ctx, db.ListReadyDependencyGraphIssueIDsForPlanParams{
		WorkspaceID: workspaceID,
		PlanID:      planID,
	})
	if err != nil {
		return fmt.Errorf("list ready dependency graph tasks: %w", err)
	}
	s.enqueueReadyDependencyGraphIssueIDs(ctx, issueIDs)
	return nil
}

// WakeDependencyGraphDependents is the completion-path wakeup. The SQL
// predicate re-checks every active hard prerequisite, so duplicate or
// out-of-order terminal events cannot release a dependent early.
func (s *TaskService) WakeDependencyGraphDependents(ctx context.Context, workspaceID, prerequisiteIssueID pgtype.UUID) error {
	if s == nil || s.Queries == nil || !workspaceID.Valid || !prerequisiteIssueID.Valid {
		return nil
	}

	var promoted []pgtype.UUID
	if err := s.runInTx(ctx, func(qtx *db.Queries) error {
		var err error
		promoted, err = qtx.PromoteReadyDependencyGraphDependents(ctx, db.PromoteReadyDependencyGraphDependentsParams{
			WorkspaceID: workspaceID,
			FromIssueID: prerequisiteIssueID,
		})
		return err
	}); err != nil {
		return fmt.Errorf("promote dependency graph dependents: %w", err)
	}

	if len(promoted) > 0 {
		s.publishDependencyGraphWakeup(workspaceID, pgtype.UUID{}, promoted)
	}
	issueIDs, err := s.Queries.ListReadyDependencyGraphIssueIDsForWorkspace(ctx, workspaceID)
	if err != nil {
		return fmt.Errorf("list ready dependency graph dependents: %w", err)
	}
	s.enqueueReadyDependencyGraphIssueIDs(ctx, issueIDs)
	return nil
}

// FlagDependencyGraphAttention records a fail-closed graph condition. A
// failed/cancelled prerequisite must never make its dependents runnable; the
// active plan instead remains visible with an operator-facing attention marker.
func (s *TaskService) FlagDependencyGraphAttention(ctx context.Context, workspaceID, prerequisiteIssueID pgtype.UUID, reason string) error {
	if s == nil || s.Queries == nil || !workspaceID.Valid || !prerequisiteIssueID.Valid {
		return nil
	}
	planIDs, err := s.Queries.MarkDependencyGraphAttentionForPrerequisite(ctx, db.MarkDependencyGraphAttentionForPrerequisiteParams{
		WorkspaceID:     workspaceID,
		FromIssueID:     prerequisiteIssueID,
		AttentionReason: pgtype.Text{String: reason, Valid: reason != ""},
	})
	if err != nil {
		return fmt.Errorf("mark dependency graph attention: %w", err)
	}
	if len(planIDs) > 0 {
		s.publishDependencyGraphAttention(workspaceID, prerequisiteIssueID, planIDs, reason)
	}
	return nil
}

func (s *TaskService) publishDependencyGraphWakeup(workspaceID, planID pgtype.UUID, promoted []pgtype.UUID) {
	if s.Bus == nil {
		return
	}
	ids := make([]string, 0, len(promoted))
	for _, id := range promoted {
		ids = append(ids, util.UUIDToString(id))
	}
	payload := map[string]any{
		"plan_id":            nil,
		"promoted_issue_ids": ids,
	}
	if planID.Valid {
		payload["plan_id"] = util.UUIDToString(planID)
	}
	s.Bus.Publish(events.Event{
		Type:        protocol.EventDependencyGraphUpdated,
		WorkspaceID: util.UUIDToString(workspaceID),
		ActorType:   "system",
		ActorID:     "",
		Payload:     payload,
	})
}

func (s *TaskService) publishDependencyGraphAttention(workspaceID, prerequisiteIssueID pgtype.UUID, planIDs []pgtype.UUID, reason string) {
	if s.Bus == nil {
		return
	}
	ids := make([]string, 0, len(planIDs))
	for _, id := range planIDs {
		ids = append(ids, util.UUIDToString(id))
	}
	s.Bus.Publish(events.Event{
		Type:        protocol.EventDependencyGraphUpdated,
		WorkspaceID: util.UUIDToString(workspaceID),
		ActorType:   "system",
		ActorID:     "",
		Payload: map[string]any{
			"plan_ids":              ids,
			"attention_required":    true,
			"prerequisite_issue_id": util.UUIDToString(prerequisiteIssueID),
			"reason":                reason,
		},
	})
}

// reconcileDependencyTasksForRuntime closes the crash window between graph
// promotion and queue insertion. It runs before ClaimTaskForRuntime's empty
// cache check, otherwise a cached-empty runtime could hide newly-ready graph
// work until the cache expires.
func (s *TaskService) reconcileDependencyTasksForRuntime(ctx context.Context, runtimeID pgtype.UUID) error {
	if s == nil || s.Queries == nil || !runtimeID.Valid {
		return nil
	}

	// This is one atomic UPDATE statement. Keep it on the base query handle so
	// runtime recovery does not consume a caller's transaction sequence before
	// the normal per-agent claim transactions (the batch claim path relies on
	// those transactions remaining independently fenced).
	promoted, err := s.Queries.PromoteReadyDependencyGraphIssuesForRuntime(ctx, runtimeID)
	if err != nil {
		return fmt.Errorf("promote runtime dependency graph tasks: %w", err)
	}
	var workspaceID pgtype.UUID
	if len(promoted) > 0 {
		issue, err := s.Queries.GetIssue(ctx, promoted[0])
		if err != nil {
			return fmt.Errorf("load promoted dependency graph issue: %w", err)
		}
		workspaceID = issue.WorkspaceID
	}

	if len(promoted) > 0 && workspaceID.Valid {
		s.publishDependencyGraphWakeup(workspaceID, pgtype.UUID{}, promoted)
	}
	issueIDs, err := s.Queries.ListReadyDependencyGraphIssueIDsForRuntime(ctx, runtimeID)
	if err != nil {
		return fmt.Errorf("list runtime dependency graph tasks: %w", err)
	}
	s.enqueueReadyDependencyGraphIssueIDs(ctx, issueIDs)
	return nil
}

func (s *TaskService) enqueueReadyDependencyGraphIssueIDs(ctx context.Context, issueIDs []pgtype.UUID) {
	for _, issueID := range issueIDs {
		var (
			issue    db.Issue
			admitted bool
		)
		err := s.runInTx(ctx, func(qtx *db.Queries) error {
			var err error
			issue, err = qtx.GetIssue(ctx, issueID)
			if err != nil {
				return err
			}
			admittedIDs, err := qtx.AdmitReadyDependencyGraphIssue(ctx, db.AdmitReadyDependencyGraphIssueParams{
				ID:          issue.ID,
				WorkspaceID: issue.WorkspaceID,
			})
			if err != nil {
				return err
			}
			if len(admittedIDs) == 0 {
				return nil
			}
			admitted = true
			issue, err = qtx.GetIssueInWorkspace(ctx, db.GetIssueInWorkspaceParams{
				ID:          issue.ID,
				WorkspaceID: issue.WorkspaceID,
			})
			if err != nil {
				return fmt.Errorf("reload admitted dependency graph issue: %w", err)
			}
			details, err := json.Marshal(map[string]any{
				"from_status": "todo",
				"to_status":   "in_progress",
				"reason":      "dependency_graph_coordinator",
			})
			if err != nil {
				return fmt.Errorf("marshal dependency graph admission activity: %w", err)
			}
			if _, err := qtx.CreateActivity(ctx, db.CreateActivityParams{
				ID:          dbid.NewV7(),
				WorkspaceID: issue.WorkspaceID,
				IssueID:     issue.ID,
				ActorType:   pgtype.Text{String: "system", Valid: true},
				Action:      "dependency_issue_admitted",
				Details:     details,
			}); err != nil {
				return fmt.Errorf("record dependency graph admission activity: %w", err)
			}
			return nil
		})
		if err != nil {
			if !errors.Is(err, pgx.ErrNoRows) {
				slog.Warn("dependency graph admission failed", "issue_id", util.UUIDToString(issueID), "error", err)
			}
			continue
		}
		if !admitted {
			continue
		}

		s.enqueueAdmittedDependencyGraphIssue(ctx, issue)
	}
}

func (s *TaskService) enqueueAdmittedDependencyGraphIssue(ctx context.Context, issue db.Issue) {
	var err error
	switch issue.ExecutorType.String {
	case "agent":
		_, err = s.EnqueueTaskForIssue(ctx, issue)
	case "team":
		team, teamErr := s.Queries.GetTeamInWorkspace(ctx, db.GetTeamInWorkspaceParams{
			ID:          issue.ExecutorID,
			WorkspaceID: issue.WorkspaceID,
		})
		if teamErr != nil {
			err = fmt.Errorf("load dependency graph team: %w", teamErr)
		} else if team.ArchivedAt.Valid {
			err = errors.New("dependency graph team is archived")
		} else {
			_, err = s.EnqueueTaskForTeamLeader(ctx, issue, team.LeaderID, team.ID, pgtype.UUID{})
		}
	default:
		err = fmt.Errorf("unsupported dependency graph executor type %q", issue.ExecutorType.String)
	}
	if err != nil && !errors.Is(err, ErrDuplicatePendingTask) {
		slog.Warn("ready dependency graph task enqueue failed", "issue_id", util.UUIDToString(issue.ID), "error", err)
	}
}

func (s *TaskService) flagDependencyAttentionForIssueTask(ctx context.Context, issueID pgtype.UUID, reason string) {
	if !issueID.Valid || s == nil || s.Queries == nil {
		return
	}
	issue, err := s.Queries.GetIssue(ctx, issueID)
	if err != nil {
		if !errors.Is(err, pgx.ErrNoRows) {
			slog.Warn("load prerequisite issue for dependency attention failed", "issue_id", util.UUIDToString(issueID), "error", err)
		}
		return
	}
	if err := s.FlagDependencyGraphAttention(ctx, issue.WorkspaceID, issue.ID, reason); err != nil {
		slog.Warn("mark dependency graph attention failed", "issue_id", util.UUIDToString(issue.ID), "error", err)
	}
}

func (s *TaskService) flagDependencyAttentionForCancelledTasks(ctx context.Context, tasks []db.AgentTaskQueue) {
	for _, task := range tasks {
		s.flagDependencyAttentionForIssueTask(ctx, task.IssueID, "prerequisite task cancelled")
	}
}
