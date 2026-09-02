package service

import (
	"context"
	"fmt"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
)

// These queries intentionally live beside the service because the current
// generated package is owned by the main agent. The matching sqlc source
// queries are in pkg/db/queries/attachment.sql; once generated, callers can
// move to those typed methods without changing the transaction contract.
const listChatSessionAttachmentURLsSQL = `
WITH target AS MATERIALIZED (
    SELECT cs.id
    FROM chat_session cs
    WHERE cs.id = $1
      AND cs.workspace_id = $2
    FOR UPDATE
)
SELECT attachment.url
FROM attachment
WHERE attachment.workspace_id = $2
  AND (
      attachment.chat_session_id IN (SELECT id FROM target)
      OR attachment.chat_message_id IN (
          SELECT cm.id
          FROM chat_message cm
          WHERE cm.chat_session_id IN (SELECT id FROM target)
      )
  )
ORDER BY attachment.id
`

const listSystemRuntimeChatAttachmentURLsSQL = `
WITH runtime_scope AS MATERIALIZED (
    SELECT id, workspace_id
    FROM agent_runtime
    WHERE id = $1
), target_sessions AS MATERIALIZED (
    SELECT cs.id
    FROM chat_session cs
    JOIN agent system_agent ON system_agent.id = cs.agent_id
    JOIN runtime_scope runtime ON runtime.id = system_agent.runtime_id
    WHERE system_agent.kind = 'system'
      AND system_agent.workspace_id = runtime.workspace_id
      AND cs.workspace_id = runtime.workspace_id
    ORDER BY cs.id
)
SELECT attachment.url
FROM attachment
WHERE attachment.workspace_id = (SELECT workspace_id FROM runtime_scope)
  AND (
      attachment.chat_session_id IN (SELECT id FROM target_sessions)
      OR attachment.chat_message_id IN (
          SELECT cm.id
          FROM chat_message cm
          WHERE cm.chat_session_id IN (SELECT id FROM target_sessions)
      )
  )
ORDER BY attachment.id
`

const lockSystemRuntimeChatSessionsSQL = `
WITH runtime_scope AS MATERIALIZED (
    SELECT id, workspace_id
    FROM agent_runtime
    WHERE id = $1
)
SELECT cs.id,
       cs.workspace_id AS session_workspace_id,
       system_agent.workspace_id AS agent_workspace_id,
       runtime.workspace_id AS runtime_workspace_id
FROM chat_session cs
JOIN agent system_agent ON system_agent.id = cs.agent_id
JOIN runtime_scope runtime ON runtime.id = system_agent.runtime_id
WHERE system_agent.kind = 'system'
ORDER BY cs.id
FOR UPDATE OF cs
`

const lockSystemRuntimeAgentsSQL = `
SELECT system_agent.id,
       system_agent.workspace_id AS agent_workspace_id,
       runtime.workspace_id AS runtime_workspace_id
FROM agent system_agent
JOIN agent_runtime runtime ON runtime.id = system_agent.runtime_id
WHERE runtime.id = $1
  AND system_agent.kind = 'system'
ORDER BY system_agent.id
FOR UPDATE OF system_agent
`

const validateSystemRuntimeChatSessionsSQL = `
SELECT cs.id,
       cs.workspace_id AS session_workspace_id,
       system_agent.workspace_id AS agent_workspace_id,
       runtime.workspace_id AS runtime_workspace_id
FROM chat_session cs
JOIN agent system_agent ON system_agent.id = cs.agent_id
JOIN agent_runtime runtime ON runtime.id = system_agent.runtime_id
WHERE runtime.id = $1
  AND system_agent.kind = 'system'
ORDER BY cs.id
`

const deleteSystemAgentIfOrphanedSQL = `
DELETE FROM agent AS target
WHERE target.id = $1
  AND target.workspace_id = $2
  AND target.kind = 'system'
  AND target.system_key LIKE 'agent_builder:%'
  AND NOT EXISTS (
      SELECT 1
      FROM chat_session session
      WHERE session.agent_id = target.id
  )
`

// ListChatSessionAttachmentURLs locks the session and returns every URL that
// DeleteChatSession removes, including session-only uploads and attachments
// linked through a chat_message. The caller must keep the same transaction
// open through the subsequent database delete.
func ListChatSessionAttachmentURLs(ctx context.Context, tx pgx.Tx, sessionID, workspaceID pgtype.UUID) ([]string, error) {
	return queryAttachmentURLs(ctx, tx, listChatSessionAttachmentURLsSQL, sessionID, workspaceID)
}

// ListSystemRuntimeChatAttachmentURLs locks the system-agent chat sessions
// belonging to a runtime and returns every URL removed when
// DeleteSystemAgentsByRuntime runs. It deliberately excludes user-agent
// sessions, whose rows and task history survive runtime teardown.
func ListSystemRuntimeChatAttachmentURLs(ctx context.Context, tx pgx.Tx, runtimeID pgtype.UUID) ([]string, error) {
	// Keep the same session -> agent order as DeleteChatSession. The first
	// statement fences sessions that already exist; the second fences creation
	// of a new session through the agent FK; the final URL query gets a fresh
	// READ COMMITTED snapshot after both locks. This closes the window where a
	// new session could be inserted after the URL snapshot but before
	// DeleteSystemAgentsByRuntime cascaded it away.
	if err := lockRuntimeRows(ctx, tx, lockSystemRuntimeChatSessionsSQL, runtimeID); err != nil {
		return nil, fmt.Errorf("lock system-agent chat sessions: %w", err)
	}
	if err := lockRuntimeRows(ctx, tx, lockSystemRuntimeAgentsSQL, runtimeID); err != nil {
		return nil, fmt.Errorf("lock system agents: %w", err)
	}
	// A session can have committed between the first session-lock statement and
	// the agent lock. Re-read the scope after the agent lock so that case is
	// validated too; once the agent lock is held, a new FK-backed session cannot
	// commit until this transaction finishes.
	if err := lockRuntimeRows(ctx, tx, validateSystemRuntimeChatSessionsSQL, runtimeID); err != nil {
		return nil, fmt.Errorf("validate system-agent chat sessions: %w", err)
	}
	return queryAttachmentURLs(ctx, tx, listSystemRuntimeChatAttachmentURLsSQL, runtimeID)
}

// DeleteSystemAgentIfOrphaned is the application-owned guard for the direct
// builder-session delete path. Once the requested session is removed, the
// system agent is deleted only if no other chat session still points at it.
// This keeps an unexpected second session (and its attachments) from being
// removed by an agent-level cascade.
func DeleteSystemAgentIfOrphaned(ctx context.Context, tx pgx.Tx, agentID, workspaceID pgtype.UUID) (bool, error) {
	tag, err := tx.Exec(ctx, deleteSystemAgentIfOrphanedSQL, agentID, workspaceID)
	if err != nil {
		return false, fmt.Errorf("delete orphaned system agent: %w", err)
	}
	return tag.RowsAffected() > 0, nil
}

func queryAttachmentURLs(ctx context.Context, tx pgx.Tx, query string, args ...any) ([]string, error) {
	rows, err := tx.Query(ctx, query, args...)
	if err != nil {
		return nil, fmt.Errorf("list attachment URLs: %w", err)
	}
	defer rows.Close()

	var urls []string
	for rows.Next() {
		var url string
		if err := rows.Scan(&url); err != nil {
			return nil, fmt.Errorf("scan attachment URL: %w", err)
		}
		urls = append(urls, url)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate attachment URLs: %w", err)
	}
	return urls, nil
}

func lockRuntimeRows(ctx context.Context, tx pgx.Tx, query string, runtimeID pgtype.UUID) error {
	rows, err := tx.Query(ctx, query, runtimeID)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		if err := validateSystemRuntimeScopeRow(rows, query == lockSystemRuntimeAgentsSQL); err != nil {
			return err
		}
	}
	return rows.Err()
}

func validateSystemRuntimeScopeRow(rows pgx.Rows, agentLockQuery bool) error {
	var (
		id                 pgtype.UUID
		sessionWorkspaceID pgtype.UUID
		agentWorkspaceID   pgtype.UUID
		runtimeWorkspaceID pgtype.UUID
	)
	if agentLockQuery {
		if err := rows.Scan(&id, &agentWorkspaceID, &runtimeWorkspaceID); err != nil {
			return err
		}
		if agentWorkspaceID != runtimeWorkspaceID {
			return fmt.Errorf("%w: system agent workspace mismatch", ErrRuntimeWorkspaceMismatch)
		}
		return nil
	}
	if err := rows.Scan(&id, &sessionWorkspaceID, &agentWorkspaceID, &runtimeWorkspaceID); err != nil {
		return err
	}
	if sessionWorkspaceID != runtimeWorkspaceID || agentWorkspaceID != runtimeWorkspaceID {
		return fmt.Errorf("%w: system-agent chat session workspace mismatch", ErrRuntimeWorkspaceMismatch)
	}
	return nil
}
