package service

import (
	"context"
	"errors"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestTerminalTaskHookFailureRollsBackStatusTransition(t *testing.T) {
	tests := []struct {
		name string
		run  func(context.Context, *TaskService, string, TerminalTaskTxHook) error
	}{
		{
			name: "complete",
			run: func(ctx context.Context, svc *TaskService, taskID string, hook TerminalTaskTxHook) error {
				_, err := svc.CompleteTaskWithTerminalHook(
					ctx,
					util.MustParseUUID(taskID),
					[]byte(`{"ok":true}`),
					"",
					"",
					"",
					false,
					"",
					"",
					hook,
				)
				return err
			},
		},
		{
			name: "fail",
			run: func(ctx context.Context, svc *TaskService, taskID string, hook TerminalTaskTxHook) error {
				_, err := svc.FailTaskWithTerminalHook(
					ctx,
					util.MustParseUUID(taskID),
					"worker failed",
					"",
					"",
					"",
					"agent_error",
					false,
					"",
					"",
					hook,
				)
				return err
			},
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			pool := newResolveOriginatorPool(t)
			ctx := context.Background()
			_, _, agentID, _ := seedAttributionFixture(t, pool)

			var runtimeID, taskID string
			if err := pool.QueryRow(ctx, `SELECT runtime_id::text FROM agent WHERE id = $1`, agentID).Scan(&runtimeID); err != nil {
				t.Fatalf("load agent runtime: %v", err)
			}
			if err := pool.QueryRow(ctx, `
				INSERT INTO agent_task_queue (agent_id, runtime_id, status, priority, attempt, max_attempts)
				VALUES ($1, $2, 'running', 0, 1, 1)
				RETURNING id
			`, agentID, runtimeID).Scan(&taskID); err != nil {
				t.Fatalf("seed running task: %v", err)
			}
			t.Cleanup(func() {
				_, _ = pool.Exec(context.Background(), `DELETE FROM agent_task_queue WHERE id = $1`, taskID)
			})

			sentinel := errors.New("terminal handoff failed")
			hook := func(ctx context.Context, tx pgx.Tx, _ *db.Queries, task db.AgentTaskQueue) error {
				if _, err := tx.Exec(ctx, `UPDATE agent_task_queue SET branch_name = 'hook-was-here' WHERE id = $1`, task.ID); err != nil {
					return err
				}
				return sentinel
			}
			svc := &TaskService{Queries: db.New(pool), TxStarter: pool, Bus: events.New()}

			err := test.run(ctx, svc, taskID, hook)
			if !errors.Is(err, sentinel) {
				t.Fatalf("terminal callback error = %v, want wrapped sentinel", err)
			}

			var status string
			var branchName *string
			if err := pool.QueryRow(ctx, `SELECT status, branch_name FROM agent_task_queue WHERE id = $1`, taskID).Scan(&status, &branchName); err != nil {
				t.Fatalf("read task after rollback: %v", err)
			}
			if status != "running" || branchName != nil {
				t.Fatalf("task after hook failure = status %q branch %v, want running with no branch", status, branchName)
			}
		})
	}
}
