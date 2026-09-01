//! Persistence for native Linear Agent Session correlations.
//!
//! Provider session ids and Patchbay task ids are different identity domains.
//! Keeping their association here makes prompts resumable without changing
//! the runtime provider's own `agent_task_queue.session_id` contract.

use crate::models::LinearAgentSession;
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
           task_id = COALESCE(EXCLUDED.task_id, linear_agent_session.task_id),\
           action = EXCLUDED.action,\
           status = EXCLUDED.status,\
           prompt_context = COALESCE(EXCLUDED.prompt_context, linear_agent_session.prompt_context),\
           prompt_body = COALESCE(EXCLUDED.prompt_body, linear_agent_session.prompt_body),\
           requester_linear_user_id = COALESCE(EXCLUDED.requester_linear_user_id, linear_agent_session.requester_linear_user_id),\
           last_event_id = EXCLUDED.last_event_id,\
           last_event_at_ms = EXCLUDED.last_event_at_ms,\
           updated_at = now() \
         WHERE linear_agent_session.status NOT IN ('completed', 'failed', 'cancelled') \
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
         WHERE linear_agent_session.status NOT IN ('completed', 'failed', 'cancelled') \
           AND (linear_agent_session.status NOT IN ('dispatching', 'terminal_dispatching') \
             AND linear_agent_session.status NOT LIKE 'dispatching:%' \
             AND linear_agent_session.status NOT LIKE 'terminal_dispatching:%' \
             OR linear_agent_session.status = EXCLUDED.status \
             OR linear_agent_session.updated_at <= now() - interval '60 seconds') \
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
        .bind(action)
        .bind(prompt_context)
        .bind(prompt_body)
        .bind(requester_linear_user_id)
        .bind(last_event_id)
        .bind(last_event_at_ms)
        .bind(format!("dispatching:{claim_owner}"))
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
               last_event_at_ms = $5,
               updated_at = now()
           WHERE workspace_id = $1
             AND connection_id = $2
             AND linear_session_id = $3
             AND status NOT IN ('completed', 'failed', 'cancelled')
             AND status <> 'dispatching'
             AND status NOT LIKE 'dispatching:%'
             AND (status NOT LIKE 'terminal_dispatching:%' OR status = $6 OR updated_at <= now() - interval '60 seconds')
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
               WHERE id = $4
               UNION ALL
               SELECT parent.id, parent.parent_task_id
               FROM agent_task_queue AS parent
               JOIN task_chain AS child ON child.parent_task_id = parent.id
           )
           INSERT INTO linear_sync_inbox
           (id, connection_id, delivery_id, event_type, payload)
           SELECT $1,
                  session.connection_id,
                  $2,
                  'linear.agentSession.terminal',
                  $3 || jsonb_build_object(
                      'agentSession', jsonb_build_object(
                          'id', session.linear_session_id,
                          'issue', jsonb_build_object('id', session.linear_issue_id)
                      )
                  )
           FROM linear_agent_session AS session
           WHERE session.task_id IN (SELECT id FROM task_chain)
           ORDER BY session.updated_at DESC, session.id DESC
           LIMIT 1
           ON CONFLICT (connection_id, delivery_id) DO NOTHING
           RETURNING id"#,
    )
    .bind(Uuid::now_v7())
    .bind(delivery_id)
    .bind(payload)
    .bind(task_id)
    .fetch_optional(&mut *executor)
    .await?;
    Ok(row.is_some())
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

pub async fn mark_linear_agent_session_terminal(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_session_id: &str,
    status: &str,
    last_event_id: &str,
    last_event_at_ms: Option<i64>,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_agent_session
           SET status = $4,
               last_event_id = $5,
               last_event_at_ms = $6,
               updated_at = now()
           WHERE workspace_id = $1
             AND connection_id = $2
             AND linear_session_id = $3
             AND ($6 IS NULL OR last_event_at_ms IS NULL OR $6 > last_event_at_ms OR last_event_id = $5)"#,
    )
    .bind(workspace_id)
    .bind(connection_id)
    .bind(linear_session_id)
    .bind(status)
    .bind(last_event_id)
    .bind(last_event_at_ms)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}
