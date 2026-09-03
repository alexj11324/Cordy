// Package channelquota enforces hosted IM Agent-turn quotas against durable
// task/message provenance. Self-hosted messaging never calls this package.
package channelquota

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	"github.com/patchbay-ai/patchbay/server/internal/entitlement"
)

type Window struct {
	Limit       int64
	PeriodStart time.Time
	PeriodEnd   time.Time
	ResetAt     time.Time
}

type AdmissionKind uint8

const (
	AdmissionBypass AdmissionKind = iota
	AdmissionUnavailable
	AdmissionLimited
)

type Admission struct {
	Kind   AdmissionKind
	Window Window
}

type ExceededError struct {
	Used  int64
	Limit int64
}

func (e *ExceededError) Error() string {
	return fmt.Sprintf("hosted IM turn quota exceeded (%d/%d)", e.Used, e.Limit)
}

func Resolve(ctx context.Context, provider entitlement.Provider, managed bool, workspaceID uuid.UUID) Admission {
	if !managed {
		return Admission{Kind: AdmissionBypass}
	}
	if provider == nil {
		return Admission{Kind: AdmissionUnavailable}
	}
	decision := provider.Gate(ctx, workspaceID, entitlement.GateImAgentTurns)
	switch decision.Gate.Action {
	case entitlement.ActionOff, entitlement.ActionObserve:
		return Admission{Kind: AdmissionBypass}
	case entitlement.ActionEnforce:
		if decision.Gate.Limit == nil {
			return Admission{Kind: AdmissionBypass}
		}
		if *decision.Gate.Limit < 0 || decision.Gate.PeriodStart == nil || decision.Gate.PeriodEnd == nil || !decision.Gate.PeriodStart.Before(*decision.Gate.PeriodEnd) {
			return Admission{Kind: AdmissionUnavailable}
		}
		resetAt := *decision.Gate.PeriodEnd
		if decision.Gate.ResetAt != nil {
			resetAt = *decision.Gate.ResetAt
		}
		return Admission{Kind: AdmissionLimited, Window: Window{
			Limit: int64(*decision.Gate.Limit), PeriodStart: *decision.Gate.PeriodStart,
			PeriodEnd: *decision.Gate.PeriodEnd, ResetAt: resetAt,
		}}
	default:
		return Admission{Kind: AdmissionUnavailable}
	}
}

type dbtx interface {
	QueryRow(context.Context, string, ...any) pgx.Row
}

func HasUnownedChannelMessage(ctx context.Context, tx dbtx, chatSessionID uuid.UUID) (bool, error) {
	var found bool
	err := tx.QueryRow(ctx, `
SELECT EXISTS (
  SELECT 1 FROM chat_message
  WHERE chat_session_id = $1
    AND role = 'user'
    AND channel_ingested = TRUE
    AND task_id IS NULL
)`, chatSessionID).Scan(&found)
	return found, err
}

// AdmitTurn serializes all workspace admissions on the workspace row, then
// counts accepted terminal and in-flight channel tasks in one snapshot.
func AdmitTurn(ctx context.Context, tx dbtx, workspaceID uuid.UUID, window Window) error {
	var locked uuid.UUID
	if err := tx.QueryRow(ctx, `SELECT id FROM workspace WHERE id = $1 FOR UPDATE`, workspaceID).Scan(&locked); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return errors.New("workspace does not exist")
		}
		return err
	}
	var used, reserved int64
	err := tx.QueryRow(ctx, `
SELECT
  count(*) FILTER (WHERE task.status NOT IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')),
  count(*) FILTER (WHERE task.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred'))
FROM agent_task_queue AS task
JOIN agent ON agent.id = task.agent_id
WHERE task.chat_session_id IS NOT NULL
  AND agent.workspace_id = $1
  AND task.created_at >= $2
  AND task.created_at < $3
  AND EXISTS (
      SELECT 1 FROM chat_message AS message
      WHERE message.task_id = task.id
        AND message.role = 'user'
        AND message.channel_ingested = TRUE
  )`, workspaceID, window.PeriodStart, window.PeriodEnd).Scan(&used, &reserved)
	if err != nil {
		return err
	}
	if used+reserved >= window.Limit {
		return &ExceededError{Used: used + reserved, Limit: window.Limit}
	}
	return nil
}
