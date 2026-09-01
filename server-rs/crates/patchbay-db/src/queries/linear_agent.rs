//! Persistence for native Linear Agent Session correlations.
//!
//! Provider session ids and Patchbay task ids are different identity domains.
//! Keeping their association here makes prompts resumable without changing
//! the runtime provider's own `agent_task_queue.session_id` contract.

use crate::models::{AgentTaskQueue, LinearAgentSession};
use serde_json::Value;
use sqlx::{Executor, PgConnection, Postgres};
use uuid::Uuid;

fn columns() -> &'static str {
    "id, workspace_id, connection_id, linear_session_id, linear_issue_id,\
     patchbay_issue_id, agent_id, task_id, action, status, prompt_context,\
     prompt_body, requester_linear_user_id,\
     last_event_id, last_event_at_ms, created_at, updated_at"
}

#[allow(clippy::too_many_arguments)]
pub async fn upsert_linear_agent_session(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_session_id: &str,
    linear_issue_id: &str,
    patchbay_issue_id: Option<Uuid>,
    agent_id: Option<Uuid>,
    task_id: Option<Uuid>,
    action: &str,
    status: &str,
    prompt_context: Option<&str>,
    prompt_body: Option<&str>,
    requester_linear_user_id: Option<&str>,
    last_event_id: &str,
    last_event_at_ms: Option<i64>,
) -> anyhow::Result<Option<LinearAgentSession>> {
    let query = format!(
        "INSERT INTO linear_agent_session \
         (id, workspace_id, connection_id, linear_session_id, linear_issue_id,\
          patchbay_issue_id, agent_id, task_id, action, status, prompt_context,\
          prompt_body, requester_linear_user_id, last_event_id, last_event_at_ms)\
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15) \
         ON CONFLICT (connection_id, linear_session_id) DO UPDATE SET \
           workspace_id = EXCLUDED.workspace_id,\
           linear_issue_id = EXCLUDED.linear_issue_id,\
           patchbay_issue_id = COALESCE(EXCLUDED.patchbay_issue_id, linear_agent_session.patchbay_issue_id),\
           agent_id = COALESCE(EXCLUDED.agent_id, linear_agent_session.agent_id),\
           task_id = CASE
             WHEN EXCLUDED.action = 'prompted'
              AND EXCLUDED.last_event_at_ms IS NOT NULL
              AND (linear_agent_session.last_event_at_ms IS NULL
                   OR EXCLUDED.last_event_at_ms > linear_agent_session.last_event_at_ms)
             THEN EXCLUDED.task_id
             ELSE COALESCE(EXCLUDED.task_id, linear_agent_session.task_id)
           END,\
           action = EXCLUDED.action,\
           status = EXCLUDED.status,\
           prompt_context = COALESCE(EXCLUDED.prompt_context, linear_agent_session.prompt_context),\
           prompt_body = COALESCE(EXCLUDED.prompt_body, linear_agent_session.prompt_body),\
           requester_linear_user_id = COALESCE(EXCLUDED.requester_linear_user_id, linear_agent_session.requester_linear_user_id),\
           last_event_id = EXCLUDED.last_event_id,\
           last_event_at_ms = EXCLUDED.last_event_at_ms,\
           updated_at = now() \
         WHERE (linear_agent_session.status NOT IN ('completed', 'failed', 'cancelled') \
                OR (EXCLUDED.action = 'prompted' \
                    AND EXCLUDED.last_event_at_ms IS NOT NULL \
                    AND (linear_agent_session.last_event_at_ms IS NULL \
                         OR EXCLUDED.last_event_at_ms > linear_agent_session.last_event_at_ms))) \
           AND (EXCLUDED.last_event_at_ms IS NULL \
            OR linear_agent_session.last_event_at_ms IS NULL \
            OR EXCLUDED.last_event_at_ms > linear_agent_session.last_event_at_ms \
            OR EXCLUDED.last_event_id = linear_agent_session.last_event_id) \
         RETURNING {columns}",
        columns = columns(),
    );
    Ok(sqlx::query_as::<_, LinearAgentSession>(&query)
        .bind(id)
        .bind(workspace_id)
        .bind(connection_id)
        .bind(linear_session_id)
        .bind(linear_issue_id)
        .bind(patchbay_issue_id)
        .bind(agent_id)
        .bind(task_id)
        .bind(action)
        .bind(status)
        .bind(prompt_context)
        .bind(prompt_body)
        .bind(requester_linear_user_id)
        .bind(last_event_id)
        .bind(last_event_at_ms)
        .fetch_optional(executor)
        .await?)
}

pub async fn get_linear_agent_session(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_session_id: &str,
) -> anyhow::Result<Option<LinearAgentSession>> {
    let query = format!(
        "SELECT {columns} FROM linear_agent_session \
         WHERE workspace_id = $1 AND connection_id = $2 AND linear_session_id = $3",
        columns = columns(),
    );
    Ok(sqlx::query_as::<_, LinearAgentSession>(&query)
        .bind(workspace_id)
        .bind(connection_id)
        .bind(linear_session_id)
        .fetch_optional(executor)
        .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn lock_linear_agent_initial_dispatch_authority(
    executor: &mut PgConnection,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_session_id: &str,
    linear_issue_id: &str,
    patchbay_issue_id: Uuid,
    agent_id: Uuid,
    requester_linear_user_id: &str,
    last_event_id: &str,
    claim_owner: &str,
) -> anyhow::Result<bool> {
    let connection = sqlx::query_scalar::<_, Uuid>(
        r#"SELECT id FROM linear_connection
           WHERE workspace_id = $1 AND id = $2
             AND status = 'active' AND actor_id <> ''
           FOR SHARE"#,
    )
    .bind(workspace_id)
    .bind(connection_id)
    .fetch_optional(&mut *executor)
    .await?;
    if connection.is_none() {
        return Ok(false);
    }
    let session = sqlx::query_scalar::<_, Uuid>(
        r#"SELECT id FROM linear_agent_session
           WHERE workspace_id = $1 AND connection_id = $2
             AND linear_session_id = $3 AND linear_issue_id = $4
             AND patchbay_issue_id = $5 AND agent_id = $6
             AND requester_linear_user_id = $7 AND last_event_id = $8
             AND status = $9
           FOR UPDATE"#,
    )
    .bind(workspace_id)
    .bind(connection_id)
    .bind(linear_session_id)
    .bind(linear_issue_id)
    .bind(patchbay_issue_id)
    .bind(agent_id)
    .bind(requester_linear_user_id)
    .bind(last_event_id)
    .bind(format!("dispatching:{claim_owner}"))
    .fetch_optional(&mut *executor)
    .await?;
    if session.is_none() {
        return Ok(false);
    }
    Ok(true)
}

pub async fn lock_linear_agent_initial_dispatch_binding(
    executor: &mut PgConnection,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_issue_id: &str,
    patchbay_issue_id: Uuid,
) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"SELECT link.id
           FROM linear_issue_link AS link
           JOIN linear_project_binding AS binding ON binding.id = link.binding_id
           WHERE link.workspace_id = $1
             AND link.linear_issue_id = $3
             AND link.patchbay_issue_id = $4
             AND link.sync_status NOT IN ('deleted', 'agent_selection_required')
             AND binding.workspace_id = $1
             AND binding.connection_id = $2
             AND binding.status = 'active'
             AND binding.sync_mode IN ('import', 'two_way')
           FOR SHARE OF binding, link"#,
    )
    .bind(workspace_id)
    .bind(connection_id)
    .bind(linear_issue_id)
    .bind(patchbay_issue_id)
    .fetch_optional(&mut *executor)
    .await?
    .is_some())
}

/// Revalidates the complete durable authority chain for a Linear-originated
/// continuation. A handler-level route decision is not sufficient on its
/// own: the installation, binding, link, Issue executor, claimed session, and
/// exact task correlation must all still agree at the service boundary.
pub async fn linear_agent_continuation_authorized(
    executor: impl Executor<'_, Database = Postgres>,
    connection_id: Uuid,
    linear_session_id: &str,
    task_id: Uuid,
    requester_user_id: Uuid,
) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS (
               SELECT 1
               FROM linear_agent_session session
               JOIN linear_connection connection
                 ON connection.id = session.connection_id
                AND connection.workspace_id = session.workspace_id
               JOIN linear_issue_link link
                 ON link.workspace_id = session.workspace_id
                AND link.linear_issue_id = session.linear_issue_id
               JOIN linear_project_binding binding
                 ON binding.id = link.binding_id
                AND binding.workspace_id = session.workspace_id
                AND binding.connection_id = session.connection_id
               JOIN issue
                 ON issue.id = session.patchbay_issue_id
                AND issue.workspace_id = session.workspace_id
               JOIN agent_task_queue task
                 ON task.id = session.task_id
                AND task.agent_id = session.agent_id
                AND task.issue_id = session.patchbay_issue_id
               JOIN member requester
                 ON requester.workspace_id = session.workspace_id
                AND requester.user_id = $4
               JOIN linear_member_binding requester_binding
                 ON requester_binding.workspace_id = session.workspace_id
                AND requester_binding.connection_id = session.connection_id
                AND requester_binding.patchbay_user_id = requester.user_id
                AND requester_binding.linear_user_id = session.requester_linear_user_id
               WHERE session.connection_id = $1
                 AND session.linear_session_id = $2
                 AND session.task_id = $3
                 AND session.status LIKE 'dispatching:%'
                 AND connection.status = 'active'
                 AND connection.actor_id <> ''
                 AND binding.status = 'active'
                 AND link.sync_status NOT IN ('deleted', 'agent_selection_required')
                 AND issue.executor_type = 'agent'
                 AND issue.executor_id = session.agent_id
           )"#,
    )
    .bind(connection_id)
    .bind(linear_session_id)
    .bind(task_id)
    .bind(requester_user_id)
    .fetch_one(executor)
    .await?)
}

/// Claims a non-terminal Agent Session delivery before any task or provider
/// side effects run. A terminal Inbox worker treats `dispatching` as a short
/// lived mutex; an idempotent retry of the same delivery may reclaim its own
/// abandoned claim.
#[allow(clippy::too_many_arguments)]
pub async fn claim_linear_agent_session_dispatch(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_session_id: &str,
    linear_issue_id: &str,
    patchbay_issue_id: Uuid,
    agent_id: Uuid,
    action: &str,
    prompt_context: Option<&str>,
    prompt_body: Option<&str>,
    requester_linear_user_id: Option<&str>,
    last_event_id: &str,
    last_event_at_ms: Option<i64>,
    claim_owner: &str,
    allow_equal_timestamp: bool,
    allow_terminal_reopen: bool,
) -> anyhow::Result<Option<LinearAgentSession>> {
    let query = format!(
        "INSERT INTO linear_agent_session \
         (id, workspace_id, connection_id, linear_session_id, linear_issue_id,\
          patchbay_issue_id, agent_id, action, status, prompt_context, prompt_body,\
          requester_linear_user_id, last_event_id, last_event_at_ms) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $14, $9, $10, $11, $12, $13) \
         ON CONFLICT (connection_id, linear_session_id) DO UPDATE SET \
           workspace_id = EXCLUDED.workspace_id,\
           linear_issue_id = EXCLUDED.linear_issue_id,\
           patchbay_issue_id = EXCLUDED.patchbay_issue_id,\
           agent_id = EXCLUDED.agent_id,\
           action = EXCLUDED.action,\
           status = EXCLUDED.status,\
           prompt_context = COALESCE(EXCLUDED.prompt_context, linear_agent_session.prompt_context),\
           prompt_body = COALESCE(EXCLUDED.prompt_body, linear_agent_session.prompt_body),\
           requester_linear_user_id = COALESCE(EXCLUDED.requester_linear_user_id, linear_agent_session.requester_linear_user_id),\
           last_event_id = EXCLUDED.last_event_id,\
           last_event_at_ms = EXCLUDED.last_event_at_ms,\
           updated_at = now() \
         WHERE (linear_agent_session.status NOT IN ('completed', 'failed', 'cancelled') \
                OR ($16 AND EXCLUDED.action = 'prompted')) \
           AND (linear_agent_session.status NOT IN ('dispatching', 'terminal_dispatching') \
             AND linear_agent_session.status NOT LIKE 'dispatching:%' \
             AND linear_agent_session.status NOT LIKE 'terminal_dispatching:%' \
             OR linear_agent_session.status = EXCLUDED.status \
             OR linear_agent_session.updated_at <= now() - interval '60 seconds') \
           AND (EXCLUDED.last_event_at_ms IS NULL \
            OR linear_agent_session.last_event_at_ms IS NULL \
            OR EXCLUDED.last_event_at_ms > linear_agent_session.last_event_at_ms \
            OR ($15 AND EXCLUDED.last_event_at_ms = linear_agent_session.last_event_at_ms) \
            OR EXCLUDED.last_event_id = linear_agent_session.last_event_id) \
         RETURNING {columns}",
        columns = columns(),
    );
    Ok(sqlx::query_as::<_, LinearAgentSession>(&query)
        .bind(id)
        .bind(workspace_id)
        .bind(connection_id)
        .bind(linear_session_id)
        .bind(linear_issue_id)
        .bind(patchbay_issue_id)
        .bind(agent_id)
        .bind(action)
        .bind(prompt_context)
        .bind(prompt_body)
        .bind(requester_linear_user_id)
        .bind(last_event_id)
        .bind(last_event_at_ms)
        .bind(format!("dispatching:{claim_owner}"))
        .bind(allow_equal_timestamp)
        .bind(allow_terminal_reopen)
        .fetch_optional(executor)
        .await?)
}

pub async fn claim_linear_agent_session_terminal(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_session_id: &str,
    last_event_id: &str,
    last_event_at_ms: Option<i64>,
    claim_owner: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_agent_session
           SET status = $6,
               last_event_id = $4,
               last_event_at_ms = COALESCE($5, last_event_at_ms),
               updated_at = now()
           WHERE workspace_id = $1
             AND connection_id = $2
             AND linear_session_id = $3
             AND status NOT IN ('completed', 'failed', 'cancelled')
             AND ((status <> 'dispatching' AND status NOT LIKE 'dispatching:%')
                  OR updated_at <= now() - interval '60 seconds')
             AND ((status <> 'terminal_dispatching' AND status NOT LIKE 'terminal_dispatching:%')
                  OR status = $6 OR updated_at <= now() - interval '60 seconds')
             AND ($5 IS NULL OR last_event_at_ms IS NULL OR $5 > last_event_at_ms OR last_event_id = $4)"#,
    )
    .bind(workspace_id)
    .bind(connection_id)
    .bind(linear_session_id)
    .bind(last_event_id)
    .bind(last_event_at_ms)
    .bind(format!("terminal_dispatching:{claim_owner}"))
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn correlate_linear_agent_session_dispatch(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_session_id: &str,
    last_event_id: &str,
    task_id: Uuid,
    claim_owner: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_agent_session
           SET task_id = $5, updated_at = now()
           WHERE workspace_id = $1
             AND connection_id = $2
             AND linear_session_id = $3
             AND last_event_id = $4
             AND status = $6"#,
    )
    .bind(workspace_id)
    .bind(connection_id)
    .bind(linear_session_id)
    .bind(last_event_id)
    .bind(task_id)
    .bind(format!("dispatching:{claim_owner}"))
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

#[allow(clippy::too_many_arguments)]
pub async fn release_linear_agent_session_dispatch(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_session_id: &str,
    last_event_id: &str,
    task_id: Uuid,
    status: &str,
    claim_owner: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_agent_session
           SET task_id = $5, status = $6, updated_at = now()
           WHERE workspace_id = $1
             AND connection_id = $2
             AND linear_session_id = $3
             AND last_event_id = $4
             AND status = $7"#,
    )
    .bind(workspace_id)
    .bind(connection_id)
    .bind(linear_session_id)
    .bind(last_event_id)
    .bind(task_id)
    .bind(status)
    .bind(format!("dispatching:{claim_owner}"))
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn set_linear_agent_session_task(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_session_id: &str,
    task_id: Uuid,
    status: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_agent_session
           SET task_id = $4, status = $5, updated_at = now()
           WHERE workspace_id = $1 AND connection_id = $2 AND linear_session_id = $3
             AND (task_id IS NULL OR task_id = $4)"#,
    )
    .bind(workspace_id)
    .bind(connection_id)
    .bind(linear_session_id)
    .bind(task_id)
    .bind(status)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn list_waiting_linear_agent_sessions(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    patchbay_issue_id: Uuid,
) -> anyhow::Result<Vec<LinearAgentSession>> {
    let query = format!(
        "SELECT {columns} FROM linear_agent_session \
         WHERE workspace_id = $1 AND patchbay_issue_id = $2 \
           AND status = 'agent_selection_required' \
         ORDER BY created_at, id",
        columns = columns(),
    );
    Ok(sqlx::query_as::<_, LinearAgentSession>(&query)
        .bind(workspace_id)
        .bind(patchbay_issue_id)
        .fetch_all(executor)
        .await?)
}

pub async fn list_linear_agent_sessions_awaiting_issue_link(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_issue_id: &str,
) -> anyhow::Result<Vec<LinearAgentSession>> {
    let query = format!(
        "SELECT {columns} FROM linear_agent_session \
         WHERE workspace_id = $1 AND connection_id = $2 AND linear_issue_id = $3 \
           AND status = 'awaiting_issue_link' \
         ORDER BY created_at, id",
        columns = columns(),
    );
    Ok(sqlx::query_as::<_, LinearAgentSession>(&query)
        .bind(workspace_id)
        .bind(connection_id)
        .bind(linear_issue_id)
        .fetch_all(executor)
        .await?)
}

/// Replays a waiting native Agent Session after the Issue has acquired a
/// valid Patchbay executor. The replay is itself an Inbox row so selecting an
/// Agent and dispatching the session remain durable across worker crashes.
pub async fn enqueue_linear_agent_session_retry(
    executor: &mut PgConnection,
    connection_id: Uuid,
    delivery_id: &str,
    session: &LinearAgentSession,
    agent_id: Option<Uuid>,
) -> anyhow::Result<bool> {
    let payload = serde_json::json!({
        "action": session.action,
        "agentSession": {
            "id": session.linear_session_id,
            "issue": {"id": session.linear_issue_id},
            "promptContext": session.prompt_context,
            "promptBody": session.prompt_body,
            "creatorId": session.requester_linear_user_id,
        },
        "selectedAgentId": agent_id,
        "linearAgentSessionRetry": true,
        "webhookTimestamp": session.last_event_at_ms,
    });
    crate::queries::linear::insert_sync_inbox(
        executor,
        Uuid::now_v7(),
        connection_id,
        delivery_id,
        "linear.agentSession.retry",
        &payload,
    )
    .await
}

/// Adds a terminal task result to the same durable Inbox consumed by the
/// Agent Session worker. The session identity is joined in SQL so callers do
/// not need to copy a provider id into the runtime task model.
pub async fn enqueue_linear_agent_terminal_event(
    executor: &mut PgConnection,
    task_id: Uuid,
    delivery_id: &str,
    payload: &Value,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        r#"WITH RECURSIVE task_chain AS (
               SELECT id, parent_task_id
               FROM agent_task_queue
               WHERE id = $3
               UNION ALL
               SELECT parent.id, parent.parent_task_id
               FROM agent_task_queue AS parent
               JOIN task_chain AS child ON child.parent_task_id = parent.id
           )
           INSERT INTO linear_sync_inbox
           (id, connection_id, delivery_id, event_type, payload)
           SELECT gen_random_uuid(),
                  session.connection_id,
                  $1 || ':' || session.linear_session_id,
                  'linear.agentSession.terminal',
                  $2 || jsonb_build_object(
                      'agentSession', jsonb_build_object(
                          'id', session.linear_session_id,
                          'issue', jsonb_build_object('id', session.linear_issue_id)
                      )
                  )
           FROM linear_agent_session AS session
           WHERE session.task_id IN (SELECT id FROM task_chain)
           ON CONFLICT (connection_id, delivery_id) DO NOTHING
           RETURNING id"#,
    )
    .bind(delivery_id)
    .bind(payload)
    .bind(task_id)
    .fetch_all(&mut *executor)
    .await?;
    Ok(!row.is_empty())
}

/// Reconstructs a missing failed-task terminal delivery from the durable task
/// tree. Runtime sweep transitions are committed before their side effects;
/// this repair runs every sweep tick so a transient enqueue failure cannot
/// strand the provider session permanently.
pub async fn recover_missing_failed_terminal_events(
    executor: &mut PgConnection,
    limit: i64,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"WITH RECURSIVE task_tree AS (
               SELECT session.id AS session_id,
                      session.connection_id,
                      session.linear_session_id,
                      session.linear_issue_id,
                      task.id AS task_id,
                      task.status,
                      task.result,
                      task.error,
                      task.failure_reason,
                      task.created_at,
                      task.parent_task_id
               FROM linear_agent_session AS session
               JOIN agent_task_queue AS task ON task.id = session.task_id
               WHERE session.status NOT IN ('completed', 'failed', 'cancelled')
               UNION ALL
               SELECT tree.session_id,
                      tree.connection_id,
                      tree.linear_session_id,
                      tree.linear_issue_id,
                      child.id,
                      child.status,
                      child.result,
                      child.error,
                      child.failure_reason,
                      child.created_at,
                      child.parent_task_id
               FROM task_tree AS tree
               JOIN agent_task_queue AS child ON child.parent_task_id = tree.task_id
           ), leaves AS (
               SELECT tree.*
               FROM task_tree AS tree
               WHERE NOT EXISTS (
                   SELECT 1 FROM task_tree AS child
                   WHERE child.session_id = tree.session_id
                     AND child.parent_task_id = tree.task_id
               )
           ), latest_leaf AS (
               SELECT DISTINCT ON (leaf.session_id) leaf.*
               FROM leaves AS leaf
               ORDER BY leaf.session_id, leaf.created_at DESC, leaf.task_id DESC
           ), candidates AS (
               SELECT latest.*
               FROM latest_leaf AS latest
               WHERE latest.status = 'failed'
                 AND NOT EXISTS (
                     SELECT 1 FROM leaves AS active
                     WHERE active.session_id = latest.session_id
                       AND active.status IN ('queued', 'deferred', 'dispatched', 'running',
                                             'waiting_local_directory', 'waiting_capacity')
                 )
                 AND NOT EXISTS (
                     SELECT 1 FROM linear_sync_inbox AS pending
                     WHERE pending.connection_id = latest.connection_id
                       AND pending.event_type = 'linear.agentSession.terminal'
                       AND pending.payload->'agentSession'->>'id' = latest.linear_session_id
                       AND pending.processed_at IS NULL
                 )
               ORDER BY latest.created_at, latest.task_id
               LIMIT $1
           )
           INSERT INTO linear_sync_inbox
               (id, connection_id, delivery_id, event_type, payload)
           SELECT gen_random_uuid(),
                  candidate.connection_id,
                  'linear-agent-terminal-recovery:' || candidate.task_id::text || ':' || candidate.linear_session_id,
                  'linear.agentSession.terminal',
                  jsonb_build_object(
                      'action', 'terminal',
                      'linearAgentSessionTerminal', true,
                      'status', 'failed',
                      'error', candidate.error,
                      'failureReason', candidate.failure_reason,
                      'taskId', candidate.task_id,
                      'agentSession', jsonb_build_object(
                          'id', candidate.linear_session_id,
                          'issue', jsonb_build_object('id', candidate.linear_issue_id)
                      )
                  )
           FROM candidates AS candidate
           ON CONFLICT (connection_id, delivery_id) DO NOTHING"#,
    )
    .bind(limit)
    .execute(&mut *executor)
    .await?;
    Ok(result.rows_affected())
}

pub async fn list_pending_revocation_cancellation_connections(
    executor: impl Executor<'_, Database = Postgres>,
    limit: i64,
) -> anyhow::Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"SELECT DISTINCT connection_id
           FROM linear_agent_session
           WHERE status = 'revocation_cancellation_pending'
              OR (status LIKE 'revocation_cancellation_dispatching:%'
                  AND updated_at <= now() - interval '60 seconds')
           ORDER BY connection_id
           LIMIT $1"#,
    )
    .bind(limit)
    .fetch_all(executor)
    .await?)
}

pub async fn claim_revocation_cancellation(
    executor: impl Executor<'_, Database = Postgres>,
    connection_id: Uuid,
    claim_owner: &str,
) -> anyhow::Result<bool> {
    let claim_status = format!("revocation_cancellation_dispatching:{claim_owner}");
    let result = sqlx::query(
        r#"UPDATE linear_agent_session AS session
           SET status = $2, updated_at = now()
           WHERE connection_id = $1
             AND (status = 'revocation_cancellation_pending'
                  OR (status LIKE 'revocation_cancellation_dispatching:%'
                      AND updated_at <= now() - interval '60 seconds'))
             AND NOT EXISTS (
                 SELECT 1 FROM linear_agent_session AS claimed
                 WHERE claimed.connection_id = $1
                   AND claimed.status LIKE 'revocation_cancellation_dispatching:%'
                   AND claimed.updated_at > now() - interval '60 seconds'
             )"#,
    )
    .bind(connection_id)
    .bind(claim_status)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list_revocation_cancelled_tasks(
    executor: impl Executor<'_, Database = Postgres>,
    connection_id: Uuid,
    claim_owner: &str,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    Ok(sqlx::query_as::<_, AgentTaskQueue>(
        r#"WITH RECURSIVE task_tree AS (
               SELECT task_id AS id
               FROM linear_agent_session
               WHERE connection_id = $1
                 AND status = $2
                 AND task_id IS NOT NULL
               UNION
               SELECT child.id
               FROM agent_task_queue AS child
               JOIN task_tree AS parent ON child.parent_task_id = parent.id
           )
           SELECT queue.*
           FROM agent_task_queue AS queue
           JOIN task_tree AS task ON task.id = queue.id
           WHERE queue.status = 'cancelled'
           ORDER BY queue.created_at, queue.id"#,
    )
    .bind(connection_id)
    .bind(format!("revocation_cancellation_dispatching:{claim_owner}"))
    .fetch_all(executor)
    .await?)
}

pub async fn complete_revocation_cancellation(
    executor: impl Executor<'_, Database = Postgres>,
    connection_id: Uuid,
    claim_owner: &str,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"UPDATE linear_agent_session
           SET status = 'cancelled', updated_at = now()
           WHERE connection_id = $1
             AND status = $2"#,
    )
    .bind(connection_id)
    .bind(format!("revocation_cancellation_dispatching:{claim_owner}"))
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

pub async fn release_revocation_cancellation(
    executor: impl Executor<'_, Database = Postgres>,
    connection_id: Uuid,
    claim_owner: &str,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"UPDATE linear_agent_session
           SET status = 'revocation_cancellation_pending', updated_at = now()
           WHERE connection_id = $1 AND status = $2"#,
    )
    .bind(connection_id)
    .bind(format!("revocation_cancellation_dispatching:{claim_owner}"))
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Settles terminal deliveries that were created for a session before its
/// Issue is deleted. The deletion transaction can then enqueue one explicit
/// cancellation per current task without an older result racing it after the
/// session correlation is removed.
pub async fn settle_pending_terminal_events_for_issue(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    patchbay_issue_id: Uuid,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"UPDATE linear_sync_inbox AS inbox
           SET processed_at = now(),
               locked_by = NULL,
               locked_until = NULL,
               last_error = 'superseded by Patchbay Issue deletion'
           FROM linear_agent_session AS session
           WHERE session.workspace_id = $1
             AND session.patchbay_issue_id = $2
             AND inbox.connection_id = session.connection_id
             AND inbox.event_type = 'linear.agentSession.terminal'
             AND inbox.payload->'agentSession'->>'id' = session.linear_session_id
             AND inbox.processed_at IS NULL"#,
    )
    .bind(workspace_id)
    .bind(patchbay_issue_id)
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// A terminal event is current when its task is the session-correlated task
/// or one of that task's retry/continuation descendants. A parent that
/// finishes after correlation moves to a child is deliberately superseded.
pub async fn linear_agent_terminal_task_matches(
    executor: impl Executor<'_, Database = Postgres>,
    current_task_id: Uuid,
    terminal_task_id: Uuid,
) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"WITH RECURSIVE ancestors AS (
               SELECT id, parent_task_id
               FROM agent_task_queue
               WHERE id = $2
               UNION ALL
               SELECT parent.id, parent.parent_task_id
               FROM agent_task_queue AS parent
               JOIN ancestors AS child ON child.parent_task_id = parent.id
           )
           SELECT EXISTS (SELECT 1 FROM ancestors WHERE id = $1)"#,
    )
    .bind(current_task_id)
    .bind(terminal_task_id)
    .fetch_one(executor)
    .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn mark_linear_agent_session_terminal(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_session_id: &str,
    status: &str,
    last_event_id: &str,
    last_event_at_ms: Option<i64>,
    claim_owner: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_agent_session
           SET status = $4,
               last_event_id = $5,
               last_event_at_ms = COALESCE($6, last_event_at_ms),
               updated_at = now()
           WHERE workspace_id = $1
             AND connection_id = $2
             AND linear_session_id = $3
             AND status = $7
             AND ($6 IS NULL OR last_event_at_ms IS NULL OR $6 > last_event_at_ms OR last_event_id = $5)"#,
    )
    .bind(workspace_id)
    .bind(connection_id)
    .bind(linear_session_id)
    .bind(status)
    .bind(last_event_id)
    .bind(last_event_at_ms)
    .bind(format!("terminal_dispatching:{claim_owner}"))
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}
