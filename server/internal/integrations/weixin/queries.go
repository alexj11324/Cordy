package weixin

import (
	"context"
	"errors"
	"fmt"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/util"
)

// sqlExecutor is intentionally smaller than *db.Queries. Weixin's receive
// cursor and sealed iLink context are adapter-owned additions to the generic
// channel tables; keeping these statements here avoids editing generated
// output before the main sqlc pass.
type sqlExecutor interface {
	Exec(context.Context, string, ...any) (pgconn.CommandTag, error)
	QueryRow(context.Context, string, ...any) pgx.Row
}

const (
	selectReceiveCursorSQL = `SELECT cursor
FROM channel_receive_state
WHERE installation_id = $1 AND channel_type = $2`
	upsertReceiveCursorSQL = `INSERT INTO channel_receive_state (installation_id, channel_type, cursor, updated_at)
VALUES ($1, $2, $3, now())
ON CONFLICT (installation_id, channel_type) DO UPDATE
SET cursor = EXCLUDED.cursor, updated_at = now()`
	deleteReceiveCursorSQL = `DELETE FROM channel_receive_state
WHERE installation_id = $1 AND channel_type = $2`
	mergeBindingConfigSQL = `UPDATE channel_chat_session_binding
SET config = channel_chat_session_binding.config || jsonb_strip_nulls($4::jsonb)
WHERE installation_id = $1
  AND channel_type = $2
  AND channel_chat_id = $3
  AND retired_at IS NULL`
	lockAgentSlotSQL = `SELECT pg_advisory_xact_lock(hashtext($1::text), hashtext($2::text))`

	// The generic channel schema deliberately has no FK/cascade, so
	// replacing an upstream account must explicitly remove provider-owned state
	// while detaching the audit trail for operator diagnosis.
	deleteInstallationForReplacementSQL = `WITH doomed AS (
    DELETE FROM channel_installation WHERE id = $1 RETURNING id
), cleared_task_deliveries AS (
    DELETE FROM channel_task_delivery
    WHERE installation_id IN (SELECT id FROM doomed)
), cleared_outbound_messages AS (
    DELETE FROM channel_outbound_message
    WHERE installation_id IN (SELECT id FROM doomed)
), cleared_chat_sessions AS (
    DELETE FROM channel_chat_session_binding
    WHERE installation_id IN (SELECT id FROM doomed)
    RETURNING chat_session_id
), cleared_chat_contexts AS (
    DELETE FROM channel_chat_context_generation
    WHERE chat_session_id IN (SELECT chat_session_id FROM cleared_chat_sessions)
), cleared_outbound_cards AS (
    DELETE FROM channel_outbound_card_message
    WHERE chat_session_id IN (SELECT chat_session_id FROM cleared_chat_sessions)
), cleared_group_routes AS (
    DELETE FROM dingtalk_group_route
    WHERE installation_id IN (SELECT id FROM doomed)
), cleared_binding_tokens AS (
    DELETE FROM channel_binding_token
    WHERE installation_id IN (SELECT id FROM doomed)
), cleared_user_bindings AS (
    DELETE FROM channel_user_binding
    WHERE installation_id IN (SELECT id FROM doomed)
), cleared_inbound_dedup AS (
    DELETE FROM channel_inbound_message_dedup
    WHERE installation_id IN (SELECT id FROM doomed)
), cleared_receive_state AS (
    DELETE FROM channel_receive_state
    WHERE installation_id IN (SELECT id FROM doomed)
), detached_audit AS (
    UPDATE channel_inbound_audit SET installation_id = NULL
    WHERE installation_id IN (SELECT id FROM doomed)
)
SELECT id FROM doomed`
)

func loadReceiveCursor(ctx context.Context, q sqlExecutor, installationID pgtype.UUID) (string, error) {
	var cursor string
	err := q.QueryRow(ctx, selectReceiveCursorSQL, installationID, string(TypeWeixin)).Scan(&cursor)
	if errors.Is(err, pgx.ErrNoRows) {
		return "", nil
	}
	if err != nil {
		return "", fmt.Errorf("load weixin receive cursor: %w", err)
	}
	return cursor, nil
}

func saveReceiveCursor(ctx context.Context, q sqlExecutor, installationID pgtype.UUID, cursor string) error {
	if !installationID.Valid || cursor == "" {
		return nil
	}
	if _, err := q.Exec(ctx, upsertReceiveCursorSQL, installationID, string(TypeWeixin), cursor); err != nil {
		return fmt.Errorf("save weixin receive cursor: %w", err)
	}
	return nil
}

func deleteReceiveCursor(ctx context.Context, q sqlExecutor, installationID pgtype.UUID) error {
	if !installationID.Valid {
		return nil
	}
	if _, err := q.Exec(ctx, deleteReceiveCursorSQL, installationID, string(TypeWeixin)); err != nil {
		return fmt.Errorf("delete weixin receive cursor: %w", err)
	}
	return nil
}

func mergeBindingConfig(ctx context.Context, q sqlExecutor, installationID pgtype.UUID, chatID string, config []byte) error {
	if !installationID.Valid || chatID == "" || len(config) == 0 {
		return nil
	}
	if _, err := q.Exec(ctx, mergeBindingConfigSQL, installationID, string(TypeWeixin), chatID, config); err != nil {
		return fmt.Errorf("merge weixin chat binding config: %w", err)
	}
	return nil
}

func lockInstallationAgentSlot(ctx context.Context, tx pgx.Tx, workspaceID, agentID pgtype.UUID) error {
	if !workspaceID.Valid || !agentID.Valid {
		return errors.New("weixin: invalid workspace or agent id for install lock")
	}
	key := util.UUIDToString(workspaceID) + ":" + util.UUIDToString(agentID)
	if _, err := tx.Exec(ctx, lockAgentSlotSQL, string(TypeWeixin), key); err != nil {
		return fmt.Errorf("lock weixin installation agent slot: %w", err)
	}
	return nil
}

func deleteInstallationForReplacement(ctx context.Context, tx pgx.Tx, installationID pgtype.UUID) error {
	if !installationID.Valid {
		return nil
	}
	if _, err := tx.Exec(ctx, deleteInstallationForReplacementSQL, installationID); err != nil {
		return fmt.Errorf("delete replaced weixin installation: %w", err)
	}
	return nil
}
