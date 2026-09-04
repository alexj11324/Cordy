package main

import (
	"context"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/service"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// TestDispatchAutomationForPlanIsIdempotent locks in the
// occurrence-level idempotency contract (MUL-3551):
//
//   - A second DispatchAutomationForPlan with the same (trigger_id,
//     planned_at) MUST return the SAME run row that the first call
//     created. No second automation_run, no second issue / task, no
//     second failure recorded.
//
// This is the dispatch-layer half of the two-defence design. The
// primary defence lives in sys_cron_executions
// (uq_sys_cron_execution). This one catches the stale-steal case
// where a runner crashes between "create run" and "write SUCCESS in
// sys_cron_executions": the next runner re-enters the dispatch and
// must reuse the in-flight run instead of duplicating it.
func TestDispatchAutomationForPlanIsIdempotent(t *testing.T) {
	ctx := context.Background()
	queries := db.New(testPool)
	bus := events.New()
	taskSvc := service.NewTaskService(queries, testPool, nil, bus)
	automationSvc := service.NewAutomationService(queries, testPool, bus, taskSvc)

	var agentID string
	if err := testPool.QueryRow(ctx,
		`SELECT id::text FROM agent WHERE workspace_id = $1 ORDER BY created_at ASC LIMIT 1`,
		testWorkspaceID,
	).Scan(&agentID); err != nil {
		t.Fatalf("load fixture agent: %v", err)
	}

	ap, err := queries.CreateAutomation(ctx, db.CreateAutomationParams{
		WorkspaceID:        parseUUID(testWorkspaceID),
		Title:              "Dispatch for plan idempotency",
		Description:        pgtype.Text{String: "Dispatch for plan test", Valid: true},
		ExecutorType:       "agent",
		ExecutorID:         parseUUID(agentID),
		Status:             "active",
		ExecutionMode:      "run_only",
		IssueTitleTemplate: pgtype.Text{},
		CreatedByType:      "member",
		CreatedByID:        parseUUID(testUserID),
	})
	if err != nil {
		t.Fatalf("CreateAutomation: %v", err)
	}
	t.Cleanup(func() {
		if _, err := testPool.Exec(context.Background(),
			`DELETE FROM automation WHERE id = $1`, ap.ID); err != nil {
			t.Logf("cleanup automation: %v", err)
		}
	})

	trigger, err := queries.CreateAutomationTrigger(ctx, db.CreateAutomationTriggerParams{
		AutomationID:    ap.ID,
		Kind:           "schedule",
		Enabled:        true,
		CronExpression: pgtype.Text{String: "*/5 * * * *", Valid: true},
		Timezone:       pgtype.Text{String: "UTC", Valid: true},
	})
	if err != nil {
		t.Fatalf("CreateAutomationTrigger: %v", err)
	}

	// Use a fixed planned_at so the partial unique index has something
	// concrete to enforce against. Truncate to seconds — the column is
	// TIMESTAMPTZ and pgx round-trips sub-microsecond, but we want the
	// comparison to be byte-stable across the two calls.
	plannedAt := time.Now().UTC().Truncate(time.Second).Add(-30 * time.Second)

	first, err := automationSvc.DispatchAutomationForPlan(
		ctx, ap, trigger.ID, "schedule", nil, plannedAt,
	)
	if err != nil {
		t.Fatalf("first DispatchAutomationForPlan: %v", err)
	}
	if first == nil {
		t.Fatalf("first call returned nil run")
	}
	if !first.PlannedAt.Valid {
		t.Fatalf("first run should have planned_at set")
	}
	if !first.PlannedAt.Time.Equal(plannedAt) {
		t.Fatalf("first run planned_at mismatch: got %s, want %s",
			first.PlannedAt.Time.Format(time.RFC3339Nano),
			plannedAt.Format(time.RFC3339Nano))
	}

	// Second call with the SAME (trigger, planned_at) must reuse the
	// first run, not create a new one.
	second, err := automationSvc.DispatchAutomationForPlan(
		ctx, ap, trigger.ID, "schedule", nil, plannedAt,
	)
	if err != nil {
		t.Fatalf("second DispatchAutomationForPlan: %v", err)
	}
	if second == nil {
		t.Fatalf("second call returned nil run")
	}
	if second.ID != first.ID {
		t.Fatalf("second call must reuse first run: first.ID=%s second.ID=%s",
			util.UUIDToString(first.ID), util.UUIDToString(second.ID))
	}

	// Belt-and-suspenders: the partial unique index plus the lookup
	// in DispatchAutomationForPlan together guarantee exactly one row.
	var rowCount int
	if err := testPool.QueryRow(ctx,
		`SELECT COUNT(*) FROM automation_run WHERE automation_id = $1`, ap.ID,
	).Scan(&rowCount); err != nil {
		t.Fatalf("count rows: %v", err)
	}
	if rowCount != 1 {
		t.Fatalf("expected exactly 1 automation_run for the (trigger, planned_at) pair, got %d", rowCount)
	}

	// A different planned_at for the same trigger MUST be allowed —
	// it represents the next scheduled occurrence, not a duplicate.
	plannedAt2 := plannedAt.Add(5 * time.Minute)
	third, err := automationSvc.DispatchAutomationForPlan(
		ctx, ap, trigger.ID, "schedule", nil, plannedAt2,
	)
	if err != nil {
		t.Fatalf("third DispatchAutomationForPlan with new planned_at: %v", err)
	}
	if third.ID == first.ID {
		t.Fatalf("different planned_at must produce a different run, got reuse")
	}

	if err := testPool.QueryRow(ctx,
		`SELECT COUNT(*) FROM automation_run WHERE automation_id = $1`, ap.ID,
	).Scan(&rowCount); err != nil {
		t.Fatalf("count rows after 3rd call: %v", err)
	}
	if rowCount != 2 {
		t.Fatalf("expected 2 automation_run rows after distinct planned_ats, got %d", rowCount)
	}
}

func TestDispatchAutomationSuppressesRecentDuplicateIssue(t *testing.T) {
	ctx := context.Background()
	queries := db.New(testPool)
	bus := events.New()
	taskSvc := service.NewTaskService(queries, testPool, nil, bus)
	automationSvc := service.NewAutomationService(queries, testPool, bus, taskSvc)

	var agentID string
	if err := testPool.QueryRow(ctx,
		`SELECT id::text FROM agent WHERE workspace_id = $1 ORDER BY created_at ASC LIMIT 1`,
		testWorkspaceID,
	).Scan(&agentID); err != nil {
		t.Fatalf("load fixture agent: %v", err)
	}

	title := "Automation recent duplicate issue " + time.Now().UTC().Format("20060102150405.000000000")
	ap, err := queries.CreateAutomation(ctx, db.CreateAutomationParams{
		WorkspaceID:        parseUUID(testWorkspaceID),
		Title:              "Recent duplicate issue guard",
		Description:        pgtype.Text{String: "Recent duplicate issue guard test", Valid: true},
		ExecutorType:       "agent",
		ExecutorID:         parseUUID(agentID),
		Status:             "active",
		ExecutionMode:      "create_issue",
		IssueTitleTemplate: pgtype.Text{String: title, Valid: true},
		CreatedByType:      "member",
		CreatedByID:        parseUUID(testUserID),
	})
	if err != nil {
		t.Fatalf("CreateAutomation: %v", err)
	}
	t.Cleanup(func() {
		bg := context.Background()
		_, _ = testPool.Exec(bg, `DELETE FROM automation WHERE id = $1`, ap.ID)
		_, _ = testPool.Exec(bg, `DELETE FROM issue WHERE workspace_id = $1 AND title = $2`, testWorkspaceID, title)
	})

	first, err := automationSvc.DispatchAutomation(ctx, ap, pgtype.UUID{}, "manual", nil)
	if err != nil {
		t.Fatalf("first DispatchAutomation: %v", err)
	}
	if first == nil || first.Status != "issue_created" || !first.IssueID.Valid {
		t.Fatalf("first dispatch = %+v, want issue_created with issue_id", first)
	}

	second, err := automationSvc.DispatchAutomation(ctx, ap, pgtype.UUID{}, "manual", nil)
	if err != nil {
		t.Fatalf("second DispatchAutomation: %v", err)
	}
	if second == nil || second.Status != "skipped" {
		t.Fatalf("second dispatch = %+v, want skipped duplicate run", second)
	}
	if second.IssueID.Valid {
		t.Fatalf("duplicate run linked issue_id=%s, want no new issue", util.UUIDToString(second.IssueID))
	}

	var count int
	if err := testPool.QueryRow(ctx,
		`SELECT count(*) FROM issue WHERE workspace_id = $1 AND title = $2`,
		testWorkspaceID, title,
	).Scan(&count); err != nil {
		t.Fatalf("count issues: %v", err)
	}
	if count != 1 {
		t.Fatalf("recent duplicate automation dispatch should leave 1 matching issue, got %d", count)
	}
}

// TestDispatchAutomationForPlanRejectsZeroArgs locks in the
// fail-loud contract: a caller that forgets to set trigger_id or
// planned_at would silently disable the idempotency guard, and the
// only safe answer is an error.
func TestDispatchAutomationForPlanRejectsZeroArgs(t *testing.T) {
	ctx := context.Background()
	queries := db.New(testPool)
	bus := events.New()
	taskSvc := service.NewTaskService(queries, testPool, nil, bus)
	automationSvc := service.NewAutomationService(queries, testPool, bus, taskSvc)

	ap := db.Automation{
		ID:            parseUUID(testWorkspaceID), // placeholder; will not be loaded since validation fails first
		WorkspaceID:   parseUUID(testWorkspaceID),
		ExecutionMode: "run_only",
		ExecutorType:  "agent",
		ExecutorID:    parseUUID(testWorkspaceID), // arbitrary; we never get past the input guard
		Status:        "active",
	}

	t.Run("invalid trigger_id", func(t *testing.T) {
		_, err := automationSvc.DispatchAutomationForPlan(
			ctx, ap, pgtype.UUID{}, "schedule", nil, time.Now().UTC(),
		)
		if err == nil {
			t.Fatalf("expected error for invalid trigger_id")
		}
	})

	t.Run("zero planned_at", func(t *testing.T) {
		_, err := automationSvc.DispatchAutomationForPlan(
			ctx, ap, parseUUID(testWorkspaceID), "schedule", nil, time.Time{},
		)
		if err == nil {
			t.Fatalf("expected error for zero planned_at")
		}
	})
}

// TestDispatchAutomationForPlanRecoversPartialRun is the regression
// for the #4443 review blocker:
//
//	"DispatchAutomationForPlan reuses existing run unconditionally,
//	 will mark a half-written run as SUCCESS even when no
//	 issue/task was ever created."
//
// We seed a partial-state automation_run for (trigger, planned_at) —
// the run exists with a non-terminal status but the corresponding
// downstream linkage (task_id for run_only, issue_id for create_issue)
// is NULL. A subsequent DispatchAutomationForPlan call at the same
// (trigger, planned_at) MUST NOT return the partial row as-is;
// instead it must mark the partial row FAILED + clear its planned_at
// to release the partial-unique slot, then create a fresh dispatched
// run with the downstream linkage actually populated.
func TestDispatchAutomationForPlanRecoversPartialRun(t *testing.T) {
	for _, mode := range []string{"run_only", "create_issue"} {
		t.Run(mode, func(t *testing.T) {
			ctx := context.Background()
			queries := db.New(testPool)
			bus := events.New()
			taskSvc := service.NewTaskService(queries, testPool, nil, bus)
			automationSvc := service.NewAutomationService(queries, testPool, bus, taskSvc)

			var agentID string
			if err := testPool.QueryRow(ctx,
				`SELECT id::text FROM agent WHERE workspace_id = $1 ORDER BY created_at ASC LIMIT 1`,
				testWorkspaceID,
			).Scan(&agentID); err != nil {
				t.Fatalf("load fixture agent: %v", err)
			}

			ap, err := queries.CreateAutomation(ctx, db.CreateAutomationParams{
				WorkspaceID:        parseUUID(testWorkspaceID),
				Title:              "Partial recovery " + mode,
				Description:        pgtype.Text{String: "partial run recovery test", Valid: true},
				ExecutorType:       "agent",
				ExecutorID:         parseUUID(agentID),
				Status:             "active",
				ExecutionMode:      mode,
				IssueTitleTemplate: pgtype.Text{String: "Partial recovery", Valid: true},
				CreatedByType:      "member",
				CreatedByID:        parseUUID(testUserID),
			})
			if err != nil {
				t.Fatalf("CreateAutomation: %v", err)
			}
			t.Cleanup(func() {
				if _, err := testPool.Exec(context.Background(),
					`DELETE FROM automation WHERE id = $1`, ap.ID); err != nil {
					t.Logf("cleanup automation: %v", err)
				}
			})

			trigger, err := queries.CreateAutomationTrigger(ctx, db.CreateAutomationTriggerParams{
				AutomationID:    ap.ID,
				Kind:           "schedule",
				Enabled:        true,
				CronExpression: pgtype.Text{String: "*/5 * * * *", Valid: true},
				Timezone:       pgtype.Text{String: "UTC", Valid: true},
			})
			if err != nil {
				t.Fatalf("CreateAutomationTrigger: %v", err)
			}

			plannedAt := time.Now().UTC().Truncate(time.Second).Add(-2 * time.Minute)
			plannedTS := pgtype.Timestamptz{Time: plannedAt, Valid: true}

			// Seed a PARTIAL run: a prior attempt wrote the run row
			// (status reflects the dispatch path's initial state) but
			// crashed before creating the downstream resource —
			// task_id is NULL for run_only, issue_id is NULL for
			// create_issue.
			initialStatus := "running"
			if mode == "create_issue" {
				initialStatus = "issue_created"
			}
			partial, err := queries.CreateAutomationRun(ctx, db.CreateAutomationRunParams{
				AutomationID:    ap.ID,
				TriggerID:      trigger.ID,
				Source:         "schedule",
				Status:         initialStatus,
				TriggerPayload: nil,
				PlannedAt:      plannedTS,
			})
			if err != nil {
				t.Fatalf("seed partial run: %v", err)
			}
			// Confirm the partial state: no downstream linkage.
			if partial.TaskID.Valid {
				t.Fatalf("seed partial run should have task_id=NULL, got %s", util.UUIDToString(partial.TaskID))
			}
			if partial.IssueID.Valid {
				t.Fatalf("seed partial run should have issue_id=NULL, got %s", util.UUIDToString(partial.IssueID))
			}

			// Retry the dispatch — this is the stale-steal codepath.
			fresh, err := automationSvc.DispatchAutomationForPlan(
				ctx, ap, trigger.ID, "schedule", nil, plannedAt,
			)
			if err != nil {
				t.Fatalf("DispatchAutomationForPlan retry: %v", err)
			}
			if fresh == nil {
				t.Fatalf("retry returned nil run")
			}
			if fresh.ID == partial.ID {
				t.Fatalf("retry must NOT reuse the partial run; got the same id %s", util.UUIDToString(fresh.ID))
			}

			// The partial row must now be FAILED with planned_at
			// cleared, so the new row's planned_at is unique.
			var partialStatus string
			var partialPlannedAt pgtype.Timestamptz
			var partialFailureReason pgtype.Text
			if err := testPool.QueryRow(ctx,
				`SELECT status, planned_at, failure_reason FROM automation_run WHERE id = $1`,
				partial.ID,
			).Scan(&partialStatus, &partialPlannedAt, &partialFailureReason); err != nil {
				t.Fatalf("read partial row after recovery: %v", err)
			}
			if partialStatus != "failed" {
				t.Fatalf("partial run must be marked failed, got %q", partialStatus)
			}
			if partialPlannedAt.Valid {
				t.Fatalf("partial run planned_at must be cleared to release partial-unique slot, still valid")
			}
			if !partialFailureReason.Valid || partialFailureReason.String == "" {
				t.Fatalf("partial run must carry a recovery failure_reason for ops, got empty")
			}

			// The fresh row must carry the original planned_at and a
			// real downstream linkage from the just-completed
			// dispatch.
			if !fresh.PlannedAt.Valid {
				t.Fatalf("fresh run planned_at must be set")
			}
			if !fresh.PlannedAt.Time.Equal(plannedAt) {
				t.Fatalf("fresh run planned_at mismatch: got %s want %s",
					fresh.PlannedAt.Time.Format(time.RFC3339Nano),
					plannedAt.Format(time.RFC3339Nano))
			}
			switch mode {
			case "run_only":
				if !fresh.TaskID.Valid {
					t.Fatalf("run_only retry must produce a run with task_id set")
				}
			case "create_issue":
				if !fresh.IssueID.Valid {
					t.Fatalf("create_issue retry must produce a run with issue_id set")
				}
			}

			// Verify the partial-unique constraint is happy: exactly
			// one row per (trigger_id, planned_at) where both are
			// non-NULL.
			var liveRows int
			if err := testPool.QueryRow(ctx, `
				SELECT COUNT(*) FROM automation_run
				 WHERE trigger_id = $1 AND planned_at = $2
			`, trigger.ID, plannedTS).Scan(&liveRows); err != nil {
				t.Fatalf("count live (trigger, planned) rows: %v", err)
			}
			if liveRows != 1 {
				t.Fatalf("expected exactly 1 live row at (trigger, planned_at) after recovery, got %d", liveRows)
			}
		})
	}
}
