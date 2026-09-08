package service

import (
	"context"
	"errors"
	"fmt"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

var (
	ErrAgentThreadSteerNoActive      = errors.New("Agent thread has no executing task to steer")
	ErrAgentThreadSteerTaskNotQueued = errors.New("Agent thread task is no longer queued")
)

type AgentThreadSteerReceipt struct {
	Task       db.AgentTaskQueue
	ActiveTask db.AgentTaskQueue
}

func isAgentThreadExecuting(task db.AgentTaskQueue) bool {
	switch task.Status {
	case "dispatched", "running", "waiting_local_directory":
		return true
	default:
		return false
	}
}

// PrioritizeAgentThreadTask selects one lineage-validated continuation while
// holding the same Agent lock as claim. The caller then interrupts ActiveTask
// through the normal user cancellation path, matching Chat's Steer semantics.
func (s *TaskService) PrioritizeAgentThreadTask(
	ctx context.Context,
	threadTaskID pgtype.UUID,
	selectedTaskID pgtype.UUID,
	requesterUserID pgtype.UUID,
) (AgentThreadSteerReceipt, error) {
	if s.TxStarter == nil {
		return AgentThreadSteerReceipt{}, fmt.Errorf("agent thread transaction unavailable")
	}
	tx, err := s.TxStarter.Begin(ctx)
	if err != nil {
		return AgentThreadSteerReceipt{}, fmt.Errorf("begin Agent thread steer: %w", err)
	}
	defer tx.Rollback(ctx)
	qtx := s.Queries.WithTx(tx)

	lockedTask, err := qtx.LockAgentThreadTask(ctx, threadTaskID)
	if err != nil {
		return AgentThreadSteerReceipt{}, fmt.Errorf("lock Agent thread: %w", err)
	}
	agent, err := qtx.GetAgent(ctx, lockedTask.AgentID)
	if err != nil {
		return AgentThreadSteerReceipt{}, fmt.Errorf("load Agent thread Agent: %w", err)
	}
	if !agentThreadInvocationAllowed(ctx, qtx, agent, requesterUserID) {
		return AgentThreadSteerReceipt{}, ErrAgentThreadInvokeForbidden
	}

	thread, err := qtx.ListAgentThreadTasks(ctx, threadTaskID)
	if err != nil {
		return AgentThreadSteerReceipt{}, fmt.Errorf("list Agent thread tasks: %w", err)
	}
	if len(thread) == 0 || !AgentThreadRootEligible(thread[0]) {
		return AgentThreadSteerReceipt{}, ErrAgentThreadSteerTaskNotQueued
	}

	var selected *db.AgentTaskQueue
	var active *db.AgentTaskQueue
	threadIDs := make([]pgtype.UUID, 0, len(thread))
	for i := range thread {
		task := &thread[i]
		threadIDs = append(threadIDs, task.ID)
		if task.ID == selectedTaskID {
			selected = task
		}
		if active == nil && isAgentThreadExecuting(*task) {
			active = task
		}
	}
	if selected == nil || (selected.Status != "queued" && selected.Status != "deferred") {
		return AgentThreadSteerReceipt{}, ErrAgentThreadSteerTaskNotQueued
	}
	if active == nil {
		return AgentThreadSteerReceipt{}, ErrAgentThreadSteerNoActive
	}

	runtime, runtimeErr := s.runtimeLookup(qtx).Get(ctx, selected.RuntimeID)
	if err := AgentThreadBindingAvailability(*selected, agent, runtimeErr == nil && runtime.WorkspaceID == agent.WorkspaceID); err != nil {
		return AgentThreadSteerReceipt{}, err
	}
	if err := AgentThreadAvailability(*selected); err != nil {
		return AgentThreadSteerReceipt{}, err
	}

	if _, err := qtx.DeferOtherQueuedAgentThreadTasks(ctx, db.DeferOtherQueuedAgentThreadTasksParams{
		ThreadTaskIds:  threadIDs,
		SelectedTaskID: selectedTaskID,
	}); err != nil {
		return AgentThreadSteerReceipt{}, fmt.Errorf("defer other Agent thread tasks: %w", err)
	}
	prioritized, err := qtx.PrioritizeAgentThreadTask(ctx, selectedTaskID)
	if errors.Is(err, pgx.ErrNoRows) || isDuplicatePendingTaskErr(err) {
		return AgentThreadSteerReceipt{}, ErrAgentThreadSteerTaskNotQueued
	}
	if err != nil {
		return AgentThreadSteerReceipt{}, fmt.Errorf("prioritize Agent thread task: %w", err)
	}
	if err := tx.Commit(ctx); err != nil {
		return AgentThreadSteerReceipt{}, fmt.Errorf("commit Agent thread steer: %w", err)
	}

	s.broadcastTaskEvent(ctx, protocol.EventTaskQueued, prioritized)
	s.NotifyTaskEnqueued(ctx, prioritized)
	return AgentThreadSteerReceipt{Task: prioritized, ActiveTask: *active}, nil
}
