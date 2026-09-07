package service

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/dispatch"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

type automationReadinessFailingDBTX struct {
	db.DBTX
	querySubstring string
	err            error
}

func (d automationReadinessFailingDBTX) Query(ctx context.Context, query string, args ...any) (pgx.Rows, error) {
	if strings.Contains(query, d.querySubstring) {
		return nil, d.err
	}
	return d.DBTX.Query(ctx, query, args...)
}

func TestAutomationReadinessIsRequiredForDispatch(t *testing.T) {
	pool := newResolveOriginatorPool(t)
	ctx := context.Background()
	q := db.New(pool)
	workspaceID, ownerID, agentID, _ := seedAttributionFixture(t, pool)

	createAutomation := func(t *testing.T) db.Automation {
		t.Helper()
		automation, err := q.CreateAutomation(ctx, db.CreateAutomationParams{
			WorkspaceID:   util.MustParseUUID(workspaceID),
			Title:         "trigger readiness test",
			ExecutorType:  "agent",
			ExecutorID:    util.MustParseUUID(agentID),
			Status:        "active",
			ExecutionMode: "run_only",
			CreatedByType: "member",
			CreatedByID:   util.MustParseUUID(ownerID),
		})
		if err != nil {
			t.Fatalf("create automation: %v", err)
		}
		return automation
	}

	createTrigger := func(t *testing.T, automationID pgtype.UUID, params db.CreateAutomationTriggerParams) {
		t.Helper()
		params.AutomationID = automationID
		if _, err := q.CreateAutomationTrigger(ctx, params); err != nil {
			t.Fatalf("create trigger: %v", err)
		}
	}

	validSchedule := func(t *testing.T, automation db.Automation) {
		t.Helper()
		createTrigger(t, automation.ID, db.CreateAutomationTriggerParams{
			Kind:           "schedule",
			Enabled:        true,
			CronExpression: pgtype.Text{String: "0 * * * *", Valid: true},
			Timezone:       pgtype.Text{String: "UTC", Valid: true},
			NextRunAt:      pgtype.Timestamptz{Time: time.Now().UTC().Add(time.Hour), Valid: true},
			Provider:       pgtype.Text{String: "generic", Valid: true},
		})
	}

	service := &AutomationService{Queries: q}

	t.Run("no trigger blocks dispatch", func(t *testing.T) {
		automation := createAutomation(t)
		readiness, err := service.CheckAutomationReadiness(ctx, automation)
		if err != nil {
			t.Fatalf("CheckAutomationReadiness: %v", err)
		}
		if readiness.Ready || len(readiness.Triggers) != 0 {
			t.Fatalf("no-trigger readiness = %+v, want blocked with no trigger rows", readiness)
		}
		if len(readiness.Reasons) != 1 || !strings.Contains(readiness.Reasons[0], "at least one trigger") {
			t.Fatalf("no-trigger reasons = %#v", readiness.Reasons)
		}
		reason, code, skipped, admissionErr := service.shouldSkipDispatch(ctx, automation, pgtype.UUID{})
		if admissionErr != nil || !skipped || code != dispatch.ReasonTriggerNotReady || !strings.Contains(reason, "at least one trigger") {
			t.Fatalf("dispatch admission = %q/%q/%v err=%v, want trigger_not_ready", reason, code, skipped, admissionErr)
		}
	})

	t.Run("incomplete native trigger blocks dispatch", func(t *testing.T) {
		automation := createAutomation(t)
		createTrigger(t, automation.ID, db.CreateAutomationTriggerParams{
			Kind:     "webhook",
			Enabled:  true,
			Provider: pgtype.Text{String: "github", Valid: true},
			Preset:   pgtype.Text{String: "github.pull_request.opened", Valid: true},
			Config:   []byte(`{}`),
		})
		readiness, err := service.CheckAutomationReadiness(ctx, automation)
		if err != nil {
			t.Fatalf("CheckAutomationReadiness: %v", err)
		}
		if readiness.Ready || len(readiness.Triggers) != 1 || readiness.Triggers[0].Ready {
			t.Fatalf("incomplete native readiness = %+v", readiness)
		}
		if !strings.Contains(strings.Join(readiness.Triggers[0].Reasons, " "), "Connect GitHub") {
			t.Fatalf("incomplete native reasons = %#v", readiness.Triggers[0].Reasons)
		}
	})

	t.Run("configured schedule admits", func(t *testing.T) {
		automation := createAutomation(t)
		validSchedule(t, automation)
		readiness, err := service.CheckAutomationReadiness(ctx, automation)
		if err != nil {
			t.Fatalf("CheckAutomationReadiness: %v", err)
		}
		if !readiness.Ready || len(readiness.Reasons) != 0 || len(readiness.Triggers) != 1 || !readiness.Triggers[0].Ready {
			t.Fatalf("configured schedule readiness = %+v", readiness)
		}
		if reason, code, skipped, admissionErr := service.shouldSkipDispatch(ctx, automation, pgtype.UUID{}); skipped || admissionErr != nil {
			t.Fatalf("configured schedule should admit, got %q/%q err=%v", reason, code, admissionErr)
		}
	})

	t.Run("legacy generic webhook with token admits", func(t *testing.T) {
		automation := createAutomation(t)
		createTrigger(t, automation.ID, db.CreateAutomationTriggerParams{
			Kind:         "webhook",
			Enabled:      true,
			Provider:     pgtype.Text{String: "generic", Valid: true},
			WebhookToken: pgtype.Text{String: "legacy-token", Valid: true},
		})
		readiness, err := service.CheckAutomationReadiness(ctx, automation)
		if err != nil {
			t.Fatalf("CheckAutomationReadiness: %v", err)
		}
		if !readiness.Ready || len(readiness.Reasons) != 0 || !readiness.Triggers[0].Ready {
			t.Fatalf("legacy generic readiness = %+v", readiness)
		}
	})

	t.Run("Slack channel-created does not require a destination channel", func(t *testing.T) {
		var installationID string
		if err := pool.QueryRow(ctx, `
			INSERT INTO channel_installation (workspace_id, agent_id, channel_type, config, status, installer_user_id)
			VALUES ($1, $2, 'slack', '{}'::jsonb, 'installed', $3)
			RETURNING id`, workspaceID, agentID, ownerID).Scan(&installationID); err != nil {
			t.Fatalf("create Slack installation: %v", err)
		}
		t.Cleanup(func() {
			_, _ = pool.Exec(context.Background(), `DELETE FROM channel_installation WHERE id = $1`, installationID)
		})
		automation := createAutomation(t)
		createTrigger(t, automation.ID, db.CreateAutomationTriggerParams{
			Kind:     "webhook",
			Enabled:  true,
			Provider: pgtype.Text{String: "slack", Valid: true},
			Preset:   pgtype.Text{String: "slack.channel_created", Valid: true},
			Config:   []byte(`{"installation_id":"` + installationID + `"}`),
		})
		readiness, err := service.CheckAutomationReadiness(ctx, automation)
		if err != nil {
			t.Fatalf("CheckAutomationReadiness: %v", err)
		}
		if !readiness.Ready || len(readiness.Reasons) != 0 {
			t.Fatalf("Slack channel-created readiness = %+v", readiness)
		}
	})

	t.Run("Slack channel-created defaults to any connected workspace", func(t *testing.T) {
		var installationID string
		if err := pool.QueryRow(ctx, `
			INSERT INTO channel_installation (workspace_id, agent_id, channel_type, config, status, installer_user_id)
			VALUES ($1, $2, 'slack', '{}'::jsonb, 'installed', $3)
			RETURNING id`, workspaceID, agentID, ownerID).Scan(&installationID); err != nil {
			t.Fatalf("create Slack installation: %v", err)
		}
		t.Cleanup(func() {
			_, _ = pool.Exec(context.Background(), `DELETE FROM channel_installation WHERE id = $1`, installationID)
		})
		automation := createAutomation(t)
		createTrigger(t, automation.ID, db.CreateAutomationTriggerParams{
			Kind:     "webhook",
			Enabled:  true,
			Provider: pgtype.Text{String: "slack", Valid: true},
			Preset:   pgtype.Text{String: "slack.channel_created", Valid: true},
			Config:   []byte(`{}`),
		})
		readiness, err := service.CheckAutomationReadiness(ctx, automation)
		if err != nil {
			t.Fatalf("CheckAutomationReadiness: %v", err)
		}
		if !readiness.Ready || len(readiness.Reasons) != 0 {
			t.Fatalf("Slack channel-created wildcard readiness = %+v", readiness)
		}
	})

	t.Run("provider lookup failure is returned for retry", func(t *testing.T) {
		automation := createAutomation(t)
		createTrigger(t, automation.ID, db.CreateAutomationTriggerParams{
			Kind:     "webhook",
			Enabled:  true,
			Provider: pgtype.Text{String: "slack", Valid: true},
			Preset:   pgtype.Text{String: "slack.channel_created", Valid: true},
			Config:   []byte(`{}`),
		})
		triggers, err := q.ListAutomationTriggers(ctx, automation.ID)
		if err != nil || len(triggers) != 1 {
			t.Fatalf("load trigger: triggers=%d err=%v", len(triggers), err)
		}
		injected := errors.New("injected Slack provider lookup failure")
		failingQueries := db.New(automationReadinessFailingDBTX{
			DBTX:           pool,
			querySubstring: "ListChannelInstallationsByWorkspace",
			err:            injected,
		})
		failingService := &AutomationService{Queries: failingQueries}
		if _, err := failingService.CheckAutomationReadiness(ctx, automation); !errors.Is(err, injected) {
			t.Fatalf("readiness error = %v, want injected provider error", err)
		}
		_, _, skipped, admissionErr := failingService.shouldSkipDispatch(ctx, automation, pgtype.UUID{})
		if skipped || !errors.Is(admissionErr, injected) {
			t.Fatalf("dispatch admission = skipped=%v err=%v, want retryable provider error", skipped, admissionErr)
		}
		_, admitErr := failingService.AdmitAutomationWebhookDelivery(ctx, automation, triggers[0].ID, []byte(`{}`), util.MustParseUUID(uuid.NewString()))
		if !errors.Is(admitErr, injected) {
			t.Fatalf("webhook admission error = %v, want injected provider error", admitErr)
		}
		var runs int
		if err := pool.QueryRow(ctx, `SELECT count(*) FROM automation_run WHERE automation_id = $1`, automation.ID).Scan(&runs); err != nil {
			t.Fatalf("count terminal runs: %v", err)
		}
		if runs != 0 {
			t.Fatalf("provider lookup failure created %d terminal runs, want zero", runs)
		}
	})

	t.Run("legacy disabled trigger remains blocking", func(t *testing.T) {
		automation := createAutomation(t)
		createTrigger(t, automation.ID, db.CreateAutomationTriggerParams{
			Kind:           "schedule",
			Enabled:        false,
			CronExpression: pgtype.Text{String: "0 * * * *", Valid: true},
			Timezone:       pgtype.Text{String: "UTC", Valid: true},
			NextRunAt:      pgtype.Timestamptz{Time: time.Now().UTC().Add(time.Hour), Valid: true},
			Provider:       pgtype.Text{String: "generic", Valid: true},
		})
		readiness, err := service.CheckAutomationReadiness(ctx, automation)
		if err != nil {
			t.Fatalf("CheckAutomationReadiness: %v", err)
		}
		if readiness.Ready || !strings.Contains(strings.Join(readiness.Triggers[0].Reasons, " "), "legacy trigger is disabled") {
			t.Fatalf("disabled trigger readiness = %+v", readiness)
		}
	})
}
