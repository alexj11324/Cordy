package service

import (
	"context"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	"github.com/orvilo-ai/orvilo/server/internal/util"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

func TestRecoverOrphansPreservesOnlyExactPendingClaimInRuntime(t *testing.T) {
	pool := newResolveOriginatorPool(t)
	workspaceID, userID, agentID, _ := seedAttributionFixture(t, pool)
	fx := testutil.New(pool, workspaceID, userID)
	var runtimeID string
	fx.QueryRow(t, `SELECT runtime_id::text FROM agent WHERE id=$1`, agentID).Scan(&runtimeID)
	otherRuntimeID := fx.Insert(t, "agent_runtime", testutil.Cols{"workspace_id": workspaceID, "owner_id": userID, "name": "other pending runtime", "runtime_mode": "cloud", "provider": "codex", "status": "online", "device_info": "", "metadata": "{}"})
	svc := &TaskService{Queries: db.New(pool), TxStarter: pool, Bus: events.New()}
	seed := func(status, rid string) db.AgentTaskQueue {
		taskID := fx.Task(t, agentID, testutil.Cols{"runtime_id": rid, "status": status, "issue_id": fx.Issue(t, "orphan fence test"), "dispatched_at": time.Now().Add(-time.Minute), "started_at": time.Now().Add(-time.Minute), "attempt": 0, "max_attempts": 1})
		task, err := svc.Queries.GetAgentTask(context.Background(), util.MustParseUUID(taskID))
		if err != nil {
			t.Fatal(err)
		}
		return task
	}
	saved := seed("running", runtimeID)
	unlisted := seed("dispatched", runtimeID)
	stale := seed("running", runtimeID)
	waiting := seed("waiting_local_directory", runtimeID)
	otherRuntime := seed("running", otherRuntimeID)
	pending := []PendingTerminalReportClaim{
		{TaskID: saved.ID, ClaimFence: TaskClaimFence(saved)},
		{TaskID: stale.ID, ClaimFence: TaskClaimFence(stale) - 32},
		{TaskID: waiting.ID, ClaimFence: TaskClaimFence(waiting)},
		{TaskID: otherRuntime.ID, ClaimFence: TaskClaimFence(otherRuntime)},
	}
	rows, err := svc.RecoverOrphanedTasksForRuntime(context.Background(), util.MustParseUUID(runtimeID), pending...)
	if err != nil {
		t.Fatal(err)
	}
	recovered := map[pgtype.UUID]bool{}
	for _, task := range rows {
		recovered[task.ID] = true
	}
	if len(rows) != 2 || !recovered[unlisted.ID] || !recovered[stale.ID] {
		t.Fatalf("recovered task IDs=%v; want only unlisted and stale", recovered)
	}
	for _, task := range []db.AgentTaskQueue{saved, waiting, otherRuntime} {
		current, err := svc.Queries.GetAgentTask(context.Background(), task.ID)
		if err != nil {
			t.Fatal(err)
		}
		if current.Status != task.Status {
			t.Fatalf("preserved task %s changed %s to %s", util.UUIDToString(task.ID), task.Status, current.Status)
		}
		if got := fx.Count(t, `SELECT count(*) FROM agent_task_queue WHERE parent_task_id=$1`, task.ID); got != 0 {
			t.Fatalf("preserved task acquired %d retry children", got)
		}
	}
	// Legacy daemons sending no exclusions retain normal orphan recovery.
	rows, err = svc.RecoverOrphanedTasksForRuntime(context.Background(), util.MustParseUUID(runtimeID))
	if err != nil {
		t.Fatal(err)
	}
	if len(rows) != 2 {
		t.Fatalf("legacy recovery got %d tasks, want saved and waiting", len(rows))
	}
}
