package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"math"
	"os"
	"strconv"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/service"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

// failureMonitorConfig is the tunable knob set for the automation failure
// monitor. Defaults match the proposal in MUL-1336 §6 action item #2:
// pause automations whose recent run history is dominated by failures and that
// have run enough times that the failure rate is statistically meaningful.
//
// All values can be overridden via env vars (see envFailureMonitorConfig).
// Setting Interval <= 0 disables the monitor entirely.
type failureMonitorConfig struct {
	Interval     time.Duration
	Lookback     time.Duration
	MinRuns      int64
	FailRatio    float64
	StartupDelay time.Duration
}

func defaultFailureMonitorConfig() failureMonitorConfig {
	return failureMonitorConfig{
		Interval:     24 * time.Hour,
		Lookback:     7 * 24 * time.Hour,
		MinRuns:      50,
		FailRatio:    0.9,
		StartupDelay: 1 * time.Minute,
	}
}

func envFailureMonitorConfig() failureMonitorConfig {
	cfg := defaultFailureMonitorConfig()
	cfg.Interval = envDurationOrZero("AUTOMATION_FAIL_MONITOR_INTERVAL", cfg.Interval)
	cfg.Lookback = envDurationPositive("AUTOMATION_FAIL_MONITOR_LOOKBACK", cfg.Lookback)
	cfg.StartupDelay = envDurationNonNegative("AUTOMATION_FAIL_MONITOR_STARTUP_DELAY", cfg.StartupDelay)
	if v, ok := envInt64Positive("AUTOMATION_FAIL_MONITOR_MIN_RUNS"); ok {
		cfg.MinRuns = v
	}
	if v, ok := envFloatInUnitInterval("AUTOMATION_FAIL_MONITOR_FAIL_RATIO"); ok {
		cfg.FailRatio = v
	}
	return cfg
}

// runAutomationFailureMonitor periodically pauses automations whose recent run
// history exceeds the configured failure threshold. This stops runaway
// scheduled automations from burning tasks/tokens on a hot loop (e.g. the
// `Registro de ls cada 5 min` case in MUL-1336: 1,475 / 1,476 runs failed
// over 7 days, still firing every 5 min). The monitor leaves a
// `severity=attention` inbox notification for the automation's creator (or the
// agent's owner if the automation was created by an agent) so somebody human
// learns that auto-pause happened.
//
// Disable with `AUTOMATION_FAIL_MONITOR_INTERVAL=0`.
func runAutomationFailureMonitor(ctx context.Context, queries *db.Queries, bus *events.Bus, cfg failureMonitorConfig) {
	if cfg.Interval <= 0 {
		slog.Info("automation failure monitor: disabled (interval <= 0)")
		return
	}

	slog.Info(
		"automation failure monitor: starting",
		"interval", cfg.Interval.String(),
		"lookback", cfg.Lookback.String(),
		"min_runs", cfg.MinRuns,
		"fail_ratio", cfg.FailRatio,
	)

	// Stagger startup so we don't all-or-nothing hit the DB the moment the
	// process boots — important during a fleet rolling restart.
	if cfg.StartupDelay > 0 {
		select {
		case <-ctx.Done():
			return
		case <-time.After(cfg.StartupDelay):
		}
	}

	// Run once immediately after the startup delay so a freshly-deployed node
	// catches existing offenders without waiting a full interval.
	tickAutomationFailureMonitor(ctx, queries, bus, cfg)

	ticker := time.NewTicker(cfg.Interval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			tickAutomationFailureMonitor(ctx, queries, bus, cfg)
		}
	}
}

// tickAutomationFailureMonitor performs a single sweep: query candidates,
// attempt to pause each, and emit notifications + WS events on success.
func tickAutomationFailureMonitor(ctx context.Context, queries *db.Queries, bus *events.Bus, cfg failureMonitorConfig) {
	since := time.Now().Add(-cfg.Lookback)
	candidates, err := queries.SelectAutomationsExceedingFailureThreshold(
		ctx,
		db.SelectAutomationsExceedingFailureThresholdParams{
			MinRuns:            cfg.MinRuns,
			FailRatioThreshold: cfg.FailRatio,
			Since:              pgtype.Timestamptz{Time: since, Valid: true},
		},
	)
	if err != nil {
		slog.Warn("automation failure monitor: failed to query candidates", "error", err)
		return
	}
	if len(candidates) == 0 {
		return
	}

	slog.Info("automation failure monitor: candidates", "count", len(candidates))

	for _, c := range candidates {
		paused, err := queries.SystemPauseAutomation(ctx, c.ID)
		if err != nil {
			// pgx returns ErrNoRows when the WHERE status='active' clause
			// matched zero rows — i.e. another caller (manual UI action,
			// concurrent monitor) paused it first. Treat as a benign no-op.
			if isNoRows(err) {
				continue
			}
			slog.Warn("automation failure monitor: pause failed",
				"automation_id", util.UUIDToString(c.ID),
				"error", err,
			)
			continue
		}

		// A system auto-pause is a substantive status change (MUL-4302 §3.4).
		// Record it as a rule-version publish with a 'system' publisher (no member
		// actor). Best-effort: the monitor is a background sweep, a paused automation
		// does not dispatch (so this version is never the active version at a real
		// run — a later member resume would supersede it), and a failed write must
		// not abort the sweep.
		if verr := service.RecordAutomationRuleVersion(ctx, queries, paused, "system", pgtype.UUID{}); verr != nil {
			slog.Warn("automation failure monitor: record rule version failed",
				"automation_id", util.UUIDToString(paused.ID), "error", verr)
		}

		failPct := 100.0
		if c.TotalRuns > 0 {
			failPct = math.Round(float64(c.FailedRuns)/float64(c.TotalRuns)*1000) / 10 // one decimal place
		}

		slog.Info(
			"automation failure monitor: paused automation",
			"automation_id", util.UUIDToString(c.ID),
			"workspace_id", util.UUIDToString(c.WorkspaceID),
			"title", c.Title,
			"failed_runs", c.FailedRuns,
			"total_runs", c.TotalRuns,
			"fail_pct", failPct,
		)

		emitAutomationPausedNotifications(ctx, queries, bus, paused, c, cfg, failPct)

		// Fan out the status change so any open UI updates the automation row.
		workspaceID := util.UUIDToString(paused.WorkspaceID)
		bus.Publish(events.Event{
			Type:        protocol.EventAutomationUpdated,
			WorkspaceID: workspaceID,
			ActorType:   "system",
			Payload: map[string]any{
				"automation": automationEventPayload(paused),
				"reason":    "auto_paused_high_failure_rate",
			},
		})
	}
}

// emitAutomationPausedNotifications creates one inbox_item per relevant
// recipient and publishes inbox:new events so each lands live. Recipients:
//
//  1. The automation creator if a member.
//  2. If the automation creator is an agent, the agent's owner_id (mapped to a
//     workspace member).
//
// Resolving against owner_id keeps us from pinging an agent whose inbox isn't
// actionable, while still attributing the alert to whoever set the automation
// up. If neither path lands a human (e.g. agent has no owner), we skip
// silently — the WS automation:updated event still surfaces the change in the
// UI for any logged-in workspace member.
func emitAutomationPausedNotifications(
	ctx context.Context,
	queries *db.Queries,
	bus *events.Bus,
	automation db.Automation,
	candidate db.SelectAutomationsExceedingFailureThresholdRow,
	cfg failureMonitorConfig,
	failPct float64,
) {
	recipients := resolveAutomationPausedRecipients(ctx, queries, automation)
	if len(recipients) == 0 {
		return
	}

	title := fmt.Sprintf("Automation paused: %s", automation.Title)
	body := fmt.Sprintf(
		"Auto-paused after %d of %d runs failed (%.1f%%) in the last %s. Investigate the failures, fix the root cause, then re-enable from the automation page.",
		candidate.FailedRuns, candidate.TotalRuns, failPct, formatLookback(cfg.Lookback),
	)
	details, _ := json.Marshal(map[string]any{
		"automation_id":         util.UUIDToString(automation.ID),
		"automation_title":      automation.Title,
		"failed_runs":          candidate.FailedRuns,
		"total_runs":           candidate.TotalRuns,
		"fail_pct":             failPct,
		"lookback_seconds":     int64(cfg.Lookback.Seconds()),
		"threshold_min_runs":   cfg.MinRuns,
		"threshold_fail_ratio": cfg.FailRatio,
		"reason":               "auto_paused_high_failure_rate",
	})

	workspaceID := util.UUIDToString(automation.WorkspaceID)
	automationIDStr := util.UUIDToString(automation.ID)

	emitted := make(map[string]bool, len(recipients))
	for _, r := range recipients {
		key := r.Type + ":" + util.UUIDToString(r.ID)
		if emitted[key] {
			continue
		}
		emitted[key] = true

		item, err := queries.CreateInboxItem(ctx, db.CreateInboxItemParams{
			ID:            dbid.NewV7(),
			WorkspaceID:   automation.WorkspaceID,
			RecipientType: r.Type,
			RecipientID:   r.ID,
			Type:          "automation_paused",
			Severity:      "attention",
			IssueID:       pgtype.UUID{},
			Title:         title,
			Body:          util.StrToText(body),
			ActorType:     util.StrToText("system"),
			ActorID:       pgtype.UUID{},
			Details:       details,
		})
		if err != nil {
			slog.Warn("automation failure monitor: inbox write failed",
				"automation_id", automationIDStr,
				"recipient_type", r.Type,
				"recipient_id", util.UUIDToString(r.ID),
				"error", err,
			)
			continue
		}

		bus.Publish(events.Event{
			Type:        protocol.EventInboxNew,
			WorkspaceID: workspaceID,
			ActorType:   "system",
			ActorID:     "",
			Payload:     map[string]any{"item": inboxItemToResponse(item)},
		})
	}
}

// pausedRecipient identifies a single inbox_item recipient.
type pausedRecipient struct {
	Type string // "member" or "agent"
	ID   pgtype.UUID
}

func resolveAutomationPausedRecipients(
	ctx context.Context,
	queries *db.Queries,
	automation db.Automation,
) []pausedRecipient {
	if automation.CreatedByType == "member" {
		return []pausedRecipient{{Type: "member", ID: automation.CreatedByID}}
	}

	// Creator is an agent — find the agent's human owner so the alert lands
	// somewhere actionable. If we can't resolve a member, skip notification
	// rather than spam an agent that can't act on it.
	agent, err := queries.GetAgent(ctx, automation.CreatedByID)
	if err != nil {
		slog.Debug("automation failure monitor: failed to load creator agent",
			"agent_id", util.UUIDToString(automation.CreatedByID),
			"error", err,
		)
		return nil
	}
	if !agent.OwnerID.Valid {
		return nil
	}

	member, err := queries.GetMemberByUserAndWorkspace(ctx, db.GetMemberByUserAndWorkspaceParams{
		UserID:      agent.OwnerID,
		WorkspaceID: automation.WorkspaceID,
	})
	if err != nil {
		return nil
	}
	return []pausedRecipient{{Type: "member", ID: member.UserID}}
}

// automationEventPayload builds the minimal payload shape consumed by
// frontend listeners (mirrors handler.AutomationResponse). Kept here instead
// of importing the handler package to avoid a cycle (handler imports the
// service which we're sitting alongside in cmd/server).
func automationEventPayload(a db.Automation) map[string]any {
	return map[string]any{
		"id":                   util.UUIDToString(a.ID),
		"workspace_id":         util.UUIDToString(a.WorkspaceID),
		"title":                a.Title,
		"description":          util.TextToPtr(a.Description),
		"assignee_id":          util.UUIDToString(a.AssigneeID),
		"status":               a.Status,
		"execution_mode":       a.ExecutionMode,
		"issue_title_template": util.TextToPtr(a.IssueTitleTemplate),
		"created_by_type":      a.CreatedByType,
		"created_by_id":        util.UUIDToString(a.CreatedByID),
		"last_run_at":          util.TimestampToPtr(a.LastRunAt),
		"created_at":           util.TimestampToString(a.CreatedAt),
		"updated_at":           util.TimestampToString(a.UpdatedAt),
	}
}

// isNoRows wraps the sentinel for pgx :one queries that match no rows. The
// SystemPauseAutomation UPDATE returns no rows when the automation was already
// paused/archived, which we want to treat as a benign no-op rather than an
// error to log.
func isNoRows(err error) bool {
	return errors.Is(err, pgx.ErrNoRows)
}

func formatLookback(d time.Duration) string {
	if d <= 0 {
		return "0s"
	}
	hours := d / time.Hour
	if hours >= 24 && d%(24*time.Hour) == 0 {
		days := hours / 24
		if days == 1 {
			return "1 day"
		}
		return fmt.Sprintf("%d days", days)
	}
	if d%time.Hour == 0 {
		if hours == 1 {
			return "1 hour"
		}
		return fmt.Sprintf("%d hours", hours)
	}
	return d.String()
}

// envDurationOrZero parses a duration env var. An explicit 0/negative is
// honored (used to disable the monitor); empty returns the default; an
// unparseable value warns and returns the default.
func envDurationOrZero(name string, def time.Duration) time.Duration {
	raw := os.Getenv(name)
	if raw == "" {
		return def
	}
	v, err := time.ParseDuration(raw)
	if err != nil {
		slog.Warn("invalid env var, using default", "name", name, "value", raw, "default", def.String(), "error", err)
		return def
	}
	return v
}

func envDurationPositive(name string, def time.Duration) time.Duration {
	raw := os.Getenv(name)
	if raw == "" {
		return def
	}
	v, err := time.ParseDuration(raw)
	if err != nil || v <= 0 {
		slog.Warn("invalid env var, using default", "name", name, "value", raw, "default", def.String(), "error", err)
		return def
	}
	return v
}

func envDurationNonNegative(name string, def time.Duration) time.Duration {
	raw := os.Getenv(name)
	if raw == "" {
		return def
	}
	v, err := time.ParseDuration(raw)
	if err != nil || v < 0 {
		slog.Warn("invalid env var, using default", "name", name, "value", raw, "default", def.String(), "error", err)
		return def
	}
	return v
}

func envInt64Positive(name string) (int64, bool) {
	raw := os.Getenv(name)
	if raw == "" {
		return 0, false
	}
	v, err := strconv.ParseInt(raw, 10, 64)
	if err != nil || v <= 0 {
		slog.Warn("invalid env var, ignored", "name", name, "value", raw, "error", err)
		return 0, false
	}
	return v, true
}

func envFloatInUnitInterval(name string) (float64, bool) {
	raw := os.Getenv(name)
	if raw == "" {
		return 0, false
	}
	v, err := strconv.ParseFloat(raw, 64)
	if err != nil || v <= 0 || v > 1 {
		slog.Warn("invalid env var (must be in (0,1]), ignored", "name", name, "value", raw, "error", err)
		return 0, false
	}
	return v, true
}
