//! Durable Linear OAuth, binding, inbox/outbox and conflict queries.
//!
//! The module intentionally contains no network calls.  Handlers and workers
//! can therefore commit local state and retry remote GraphQL work without
//! holding a database transaction open across the network.

use crate::models::{
    LinearConnection, LinearIssueLink, LinearOAuthState, LinearProjectBinding,
    LinearRelationLink, LinearStatusBinding, LinearSyncConflict, LinearSyncInbox,
    LinearSyncOutbox,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

pub async fn get_connection_for_workspace(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Option<LinearConnection>> {
    Ok(sqlx::query_as::<_, LinearConnection>(
        "SELECT * FROM linear_connection WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?)
}

/// Active connections are polled by the durable Linear sync worker. Keeping
/// this query in the DB crate makes restart/retry behavior independent from
/// the HTTP process that accepted the webhook.
pub async fn list_active_connections(
    executor: impl Executor<'_, Database = Postgres>,
) -> anyhow::Result<Vec<LinearConnection>> {
    Ok(sqlx::query_as::<_, LinearConnection>(
        "SELECT * FROM linear_connection WHERE status = 'active' ORDER BY updated_at, id",
    )
    .fetch_all(executor)
    .await?)
}

pub async fn upsert_connection(
    executor: impl Executor<'_, Database = Postgres>,
    connection: &LinearConnectionInput<'_>,
) -> anyhow::Result<LinearConnection> {
    Ok(sqlx::query_as::<_, LinearConnection>(
        r#"INSERT INTO linear_connection
           (id, workspace_id, organization_id, organization_name, actor_id,
            access_token_encrypted, refresh_token_encrypted, token_expires_at,
            scopes, status, created_by_id)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'active',$10)
           ON CONFLICT (workspace_id) DO UPDATE SET
             organization_id = EXCLUDED.organization_id,
             organization_name = EXCLUDED.organization_name,
             actor_id = EXCLUDED.actor_id,
             access_token_encrypted = EXCLUDED.access_token_encrypted,
             refresh_token_encrypted = EXCLUDED.refresh_token_encrypted,
             token_expires_at = EXCLUDED.token_expires_at,
             scopes = EXCLUDED.scopes,
             status = 'active',
             updated_at = now()
           RETURNING *"#,
    )
    .bind(connection.id)
    .bind(connection.workspace_id)
    .bind(connection.organization_id)
    .bind(connection.organization_name)
    .bind(connection.actor_id)
    .bind(connection.access_token_encrypted)
    .bind(connection.refresh_token_encrypted)
    .bind(connection.token_expires_at)
    .bind(connection.scopes)
    .bind(connection.created_by_id)
    .fetch_one(executor)
    .await?)
}

pub struct LinearConnectionInput<'a> {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub organization_id: &'a str,
    pub organization_name: Option<&'a str>,
    pub actor_id: Option<&'a str>,
    pub access_token_encrypted: &'a str,
    pub refresh_token_encrypted: &'a str,
    pub token_expires_at: Option<DateTime<Utc>>,
    pub scopes: &'a Value,
    pub created_by_id: Uuid,
}

pub async fn insert_oauth_state(
    executor: impl Executor<'_, Database = Postgres>,
    state: &OAuthStateInput<'_>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO linear_oauth_state
           (id,state_hash,workspace_id,user_id,code_verifier_encrypted,redirect_uri,expires_at)
           VALUES ($1,$2,$3,$4,$5,$6,$7)"#,
    )
    .bind(state.id)
    .bind(state.state_hash)
    .bind(state.workspace_id)
    .bind(state.user_id)
    .bind(state.code_verifier_encrypted)
    .bind(state.redirect_uri)
    .bind(state.expires_at)
    .execute(executor)
    .await?;
    Ok(())
}

pub struct OAuthStateInput<'a> {
    pub id: Uuid,
    pub state_hash: &'a str,
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub code_verifier_encrypted: &'a str,
    pub redirect_uri: &'a str,
    pub expires_at: DateTime<Utc>,
}

pub async fn consume_oauth_state(
    executor: impl Executor<'_, Database = Postgres>,
    state_hash: &str,
) -> anyhow::Result<Option<LinearOAuthState>> {
    Ok(sqlx::query_as::<_, LinearOAuthState>(
        r#"UPDATE linear_oauth_state
           SET consumed_at = now()
           WHERE state_hash = $1 AND consumed_at IS NULL AND expires_at > now()
           RETURNING *"#,
    )
    .bind(state_hash)
    .fetch_optional(executor)
    .await?)
}

pub async fn list_project_bindings(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<LinearProjectBinding>> {
    Ok(sqlx::query_as::<_, LinearProjectBinding>(
        "SELECT * FROM linear_project_binding WHERE workspace_id = $1 ORDER BY created_at, id",
    )
    .bind(workspace_id)
    .fetch_all(executor)
    .await?)
}

pub async fn get_project_binding(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    id: Uuid,
) -> anyhow::Result<Option<LinearProjectBinding>> {
    Ok(sqlx::query_as::<_, LinearProjectBinding>(
        "SELECT * FROM linear_project_binding WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(id)
    .fetch_optional(executor)
    .await?)
}

pub async fn upsert_project_binding(
    executor: impl Executor<'_, Database = Postgres>,
    binding: &ProjectBindingInput,
) -> anyhow::Result<LinearProjectBinding> {
    Ok(sqlx::query_as::<_, LinearProjectBinding>(
        r#"INSERT INTO linear_project_binding
           (id,workspace_id,connection_id,patchbay_project_id,linear_project_id,
            default_linear_team_id,sync_mode,status)
           VALUES ($1,$2,$3,$4,$5,$6,$7,'active')
           ON CONFLICT (workspace_id,linear_project_id) DO UPDATE SET
             connection_id = EXCLUDED.connection_id,
             patchbay_project_id = EXCLUDED.patchbay_project_id,
             default_linear_team_id = EXCLUDED.default_linear_team_id,
             sync_mode = EXCLUDED.sync_mode,
             status = 'active', updated_at = now()
           RETURNING *"#,
    )
    .bind(binding.id)
    .bind(binding.workspace_id)
    .bind(binding.connection_id)
    .bind(binding.patchbay_project_id)
    .bind(binding.linear_project_id)
    .bind(binding.default_linear_team_id)
    .bind(binding.sync_mode)
    .fetch_one(executor)
    .await?)
}

pub struct ProjectBindingInput {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub connection_id: Uuid,
    pub patchbay_project_id: Option<Uuid>,
    pub linear_project_id: String,
    pub default_linear_team_id: Option<String>,
    pub sync_mode: String,
}

pub async fn list_status_bindings(
    executor: impl Executor<'_, Database = Postgres>,
    project_binding_id: Uuid,
) -> anyhow::Result<Vec<LinearStatusBinding>> {
    Ok(sqlx::query_as::<_, LinearStatusBinding>(
        "SELECT * FROM linear_status_binding WHERE project_binding_id = $1 ORDER BY patchbay_status, id",
    )
    .bind(project_binding_id)
    .fetch_all(executor)
    .await?)
}

pub struct StatusBindingInput {
    pub id: Uuid,
    pub project_binding_id: Uuid,
    pub patchbay_status: String,
    pub linear_status_id: String,
}

pub async fn upsert_status_binding(
    executor: impl Executor<'_, Database = Postgres>,
    binding: &StatusBindingInput,
) -> anyhow::Result<LinearStatusBinding> {
    Ok(sqlx::query_as::<_, LinearStatusBinding>(
        r#"INSERT INTO linear_status_binding
           (id, project_binding_id, patchbay_status, linear_status_id)
           VALUES ($1,$2,$3,$4)
           ON CONFLICT (project_binding_id, patchbay_status) DO UPDATE SET
             linear_status_id = EXCLUDED.linear_status_id,
             updated_at = now()
           RETURNING *"#,
    )
    .bind(binding.id)
    .bind(binding.project_binding_id)
    .bind(&binding.patchbay_status)
    .bind(&binding.linear_status_id)
    .fetch_one(executor)
    .await?)
}

pub async fn list_agent_bindings(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<crate::models::LinearAgentBinding>> {
    Ok(sqlx::query_as::<_, crate::models::LinearAgentBinding>(
        "SELECT * FROM linear_agent_binding WHERE workspace_id = $1 ORDER BY label_name, id",
    )
    .bind(workspace_id)
    .fetch_all(executor)
    .await?)
}

pub struct AgentBindingInput {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub linear_label_group_id: String,
    pub linear_label_id: String,
    pub label_name: String,
}

pub async fn upsert_agent_binding(
    executor: impl Executor<'_, Database = Postgres>,
    binding: &AgentBindingInput,
) -> anyhow::Result<crate::models::LinearAgentBinding> {
    Ok(sqlx::query_as::<_, crate::models::LinearAgentBinding>(
        r#"INSERT INTO linear_agent_binding
           (id, workspace_id, agent_id, linear_label_group_id, linear_label_id, label_name)
           VALUES ($1,$2,$3,$4,$5,$6)
           ON CONFLICT (workspace_id, agent_id) DO UPDATE SET
             linear_label_group_id = EXCLUDED.linear_label_group_id,
             linear_label_id = EXCLUDED.linear_label_id,
             label_name = EXCLUDED.label_name,
             updated_at = now()
           RETURNING *"#,
    )
    .bind(binding.id)
    .bind(binding.workspace_id)
    .bind(binding.agent_id)
    .bind(&binding.linear_label_group_id)
    .bind(&binding.linear_label_id)
    .bind(&binding.label_name)
    .fetch_one(executor)
    .await?)
}

pub async fn get_issue_link(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<Option<LinearIssueLink>> {
    Ok(sqlx::query_as::<_, LinearIssueLink>(
        "SELECT * FROM linear_issue_link WHERE workspace_id = $1 AND issue_id = $2",
    )
    .bind(workspace_id)
    .bind(issue_id)
    .fetch_optional(executor)
    .await?)
}

pub async fn get_issue_link_by_linear_id(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    linear_issue_id: &str,
) -> anyhow::Result<Option<LinearIssueLink>> {
    Ok(sqlx::query_as::<_, LinearIssueLink>(
        "SELECT * FROM linear_issue_link WHERE workspace_id = $1 AND linear_issue_id = $2",
    )
    .bind(workspace_id)
    .bind(linear_issue_id)
    .fetch_optional(executor)
    .await?)
}

pub async fn get_project_binding_by_linear_id(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    linear_project_id: &str,
) -> anyhow::Result<Option<LinearProjectBinding>> {
    Ok(sqlx::query_as::<_, LinearProjectBinding>(
        "SELECT * FROM linear_project_binding WHERE workspace_id = $1 AND linear_project_id = $2 AND status = 'active'",
    )
    .bind(workspace_id)
    .bind(linear_project_id)
    .fetch_optional(executor)
    .await?)
}

pub struct IssueLinkInput {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub issue_id: Uuid,
    pub linear_issue_id: String,
    pub linear_identifier: Option<String>,
    pub project_binding_id: Uuid,
    pub remote_updated_at: Option<DateTime<Utc>>,
    pub remote_snapshot: Value,
}

/// Creates or refreshes the immutable-ID link used by both sync directions.
/// The conflict target is provided by the concurrent unique indexes added by
/// the Linear migrations; no foreign keys are required.
pub async fn upsert_issue_link(
    executor: impl Executor<'_, Database = Postgres>,
    link: &IssueLinkInput,
) -> anyhow::Result<LinearIssueLink> {
    Ok(sqlx::query_as::<_, LinearIssueLink>(
        r#"INSERT INTO linear_issue_link
           (id,workspace_id,issue_id,linear_issue_id,linear_identifier,
            project_binding_id,remote_updated_at,remote_snapshot,status)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'active')
           ON CONFLICT (workspace_id,issue_id) DO UPDATE SET
             linear_issue_id = EXCLUDED.linear_issue_id,
             linear_identifier = EXCLUDED.linear_identifier,
             project_binding_id = EXCLUDED.project_binding_id,
             remote_updated_at = EXCLUDED.remote_updated_at,
             remote_snapshot = EXCLUDED.remote_snapshot,
             status = 'active', updated_at = now()
           RETURNING *"#,
    )
    .bind(link.id)
    .bind(link.workspace_id)
    .bind(link.issue_id)
    .bind(&link.linear_issue_id)
    .bind(&link.linear_identifier)
    .bind(link.project_binding_id)
    .bind(link.remote_updated_at)
    .bind(&link.remote_snapshot)
    .fetch_one(executor)
    .await?)
}

pub async fn mark_issue_link_pushed(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    remote_updated_at: Option<DateTime<Utc>>,
    remote_snapshot: &Value,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "UPDATE linear_issue_link SET last_pushed_at = now(), remote_updated_at = COALESCE($2, remote_updated_at), remote_snapshot = $3, updated_at = now() WHERE id = $1 AND status = 'active'",
    )
    .bind(id)
    .bind(remote_updated_at)
    .bind(remote_snapshot)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn mark_issue_link_pulled(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    remote_updated_at: Option<DateTime<Utc>>,
    remote_snapshot: &Value,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "UPDATE linear_issue_link SET last_pulled_at = now(), remote_updated_at = COALESCE($2, remote_updated_at), remote_snapshot = $3, updated_at = now() WHERE id = $1 AND status = 'active'",
    )
    .bind(id)
    .bind(remote_updated_at)
    .bind(remote_snapshot)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn resolve_agent_for_linear_label(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    linear_label_id: &str,
) -> anyhow::Result<Option<Uuid>> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT agent_id FROM linear_agent_binding WHERE workspace_id = $1 AND linear_label_id = $2",
    )
    .bind(workspace_id)
    .bind(linear_label_id)
    .fetch_optional(executor)
    .await?)
}

/// Resolve a Linear human identity to the Patchbay user id used by
/// `issue.owner_id`. The binding table stores the member-row id so uniqueness
/// is enforced at the workspace membership boundary; the worker joins once
/// here and never matches by display name.
pub async fn resolve_patchbay_user_for_linear_user(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    linear_user_id: &str,
) -> anyhow::Result<Option<Uuid>> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"SELECT m.user_id
FROM linear_member_binding binding
JOIN member m ON m.id = binding.member_id
JOIN "user" u ON u.id = m.user_id
WHERE binding.workspace_id = $1
  AND binding.linear_user_id = $2
  AND binding.status = 'active'
  AND NOT u.is_guest"#,
    )
    .bind(workspace_id)
    .bind(linear_user_id)
    .fetch_optional(executor)
    .await?)
}

pub async fn list_relation_links(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<Vec<LinearRelationLink>> {
    Ok(sqlx::query_as::<_, LinearRelationLink>(
        "SELECT * FROM linear_relation_link WHERE workspace_id = $1 AND (from_issue_id = $2 OR to_issue_id = $2) AND status = 'active' ORDER BY created_at, id",
    )
    .bind(workspace_id)
    .bind(issue_id)
    .fetch_all(executor)
    .await?)
}

pub struct RelationLinkInput {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub from_issue_id: Uuid,
    pub to_issue_id: Uuid,
    pub linear_relation_id: Option<String>,
    pub relation_type: String,
    pub status: String,
}

pub async fn upsert_relation_link(
    executor: impl Executor<'_, Database = Postgres>,
    link: &RelationLinkInput,
) -> anyhow::Result<LinearRelationLink> {
    Ok(sqlx::query_as::<_, LinearRelationLink>(
        r#"INSERT INTO linear_relation_link
           (id, workspace_id, from_issue_id, to_issue_id, linear_relation_id, relation_type, status)
           VALUES ($1,$2,$3,$4,$5,$6,$7)
           ON CONFLICT (workspace_id, from_issue_id, to_issue_id, relation_type) DO UPDATE SET
             linear_relation_id = EXCLUDED.linear_relation_id,
             status = EXCLUDED.status,
             updated_at = now()
           RETURNING *"#,
    )
    .bind(link.id)
    .bind(link.workspace_id)
    .bind(link.from_issue_id)
    .bind(link.to_issue_id)
    .bind(&link.linear_relation_id)
    .bind(&link.relation_type)
    .bind(&link.status)
    .fetch_one(executor)
    .await?)
}

pub async fn tombstone_relation_link(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    from_issue_id: Uuid,
    to_issue_id: Uuid,
    relation_type: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "UPDATE linear_relation_link SET status = 'tombstone', updated_at = now() WHERE workspace_id = $1 AND from_issue_id = $2 AND to_issue_id = $3 AND relation_type = $4 AND status <> 'tombstone'",
    )
    .bind(workspace_id)
    .bind(from_issue_id)
    .bind(to_issue_id)
    .bind(relation_type)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn insert_sync_inbox(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    connection_id: Uuid,
    delivery_id: &str,
    event_type: &str,
    payload: &Value,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        r#"INSERT INTO linear_sync_inbox
           (id,connection_id,delivery_id,event_type,payload)
           VALUES ($1,$2,$3,$4,$5)
           ON CONFLICT (connection_id,delivery_id) DO NOTHING
           RETURNING id"#,
    )
    .bind(id)
    .bind(connection_id)
    .bind(delivery_id)
    .bind(event_type)
    .bind(payload)
    .fetch_optional(executor)
    .await?;
    Ok(row.is_some())
}

pub async fn enqueue_sync_outbox(
    executor: impl Executor<'_, Database = Postgres>,
    item: &SyncOutboxInput<'_>,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        r#"INSERT INTO linear_sync_outbox
           (id,workspace_id,connection_id,issue_id,correlation_id,operation,payload)
           VALUES ($1,$2,$3,$4,$5,$6,$7)
           ON CONFLICT (workspace_id,correlation_id) DO NOTHING
           RETURNING id"#,
    )
    .bind(item.id)
    .bind(item.workspace_id)
    .bind(item.connection_id)
    .bind(item.issue_id)
    .bind(item.correlation_id)
    .bind(item.operation)
    .bind(item.payload)
    .fetch_optional(executor)
    .await?;
    Ok(row.is_some())
}

pub struct SyncOutboxInput<'a> {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub connection_id: Uuid,
    pub issue_id: Option<Uuid>,
    pub correlation_id: Uuid,
    pub operation: &'a str,
    pub payload: &'a Value,
}

pub async fn list_pending_outbox(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    limit: i64,
) -> anyhow::Result<Vec<LinearSyncOutbox>> {
    Ok(sqlx::query_as::<_, LinearSyncOutbox>(
        "SELECT * FROM linear_sync_outbox WHERE workspace_id = $1 AND sent_at IS NULL AND available_at <= now() ORDER BY available_at, id LIMIT $2",
    )
    .bind(workspace_id)
    .bind(limit)
    .fetch_all(executor)
    .await?)
}

pub async fn mark_outbox_sent(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "UPDATE linear_sync_outbox SET sent_at = now(), last_error = NULL WHERE id = $1 AND sent_at IS NULL",
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn mark_outbox_failed(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    error: &str,
    retry_after_seconds: i64,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "UPDATE linear_sync_outbox SET attempts = attempts + 1, last_error = $2, available_at = now() + make_interval(secs => $3::double precision) WHERE id = $1 AND sent_at IS NULL",
    )
    .bind(id)
    .bind(error)
    .bind(retry_after_seconds.max(1))
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list_pending_inbox(
    executor: impl Executor<'_, Database = Postgres>,
    connection_id: Uuid,
    limit: i64,
) -> anyhow::Result<Vec<LinearSyncInbox>> {
    Ok(sqlx::query_as::<_, LinearSyncInbox>(
        "SELECT * FROM linear_sync_inbox WHERE connection_id = $1 AND processed_at IS NULL ORDER BY received_at, id LIMIT $2",
    )
    .bind(connection_id)
    .bind(limit)
    .fetch_all(executor)
    .await?)
}

pub async fn mark_inbox_processed(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "UPDATE linear_sync_inbox SET processed_at = now(), last_error = NULL WHERE id = $1 AND processed_at IS NULL",
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn mark_inbox_failed(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    error: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "UPDATE linear_sync_inbox SET attempts = attempts + 1, last_error = $2 WHERE id = $1 AND processed_at IS NULL",
    )
    .bind(id)
    .bind(error)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn create_conflict(
    executor: impl Executor<'_, Database = Postgres>,
    conflict: &ConflictInput<'_>,
) -> anyhow::Result<LinearSyncConflict> {
    Ok(sqlx::query_as::<_, LinearSyncConflict>(
        r#"INSERT INTO linear_sync_conflict
           (id,workspace_id,issue_id,linear_issue_id,field,local_value,
            remote_value,local_revision,remote_updated_at,correlation_id)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
           RETURNING *"#,
    )
    .bind(conflict.id)
    .bind(conflict.workspace_id)
    .bind(conflict.issue_id)
    .bind(conflict.linear_issue_id)
    .bind(conflict.field)
    .bind(conflict.local_value)
    .bind(conflict.remote_value)
    .bind(conflict.local_revision)
    .bind(conflict.remote_updated_at)
    .bind(conflict.correlation_id)
    .fetch_one(executor)
    .await?)
}

pub async fn list_open_conflicts(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    limit: i64,
) -> anyhow::Result<Vec<LinearSyncConflict>> {
    Ok(sqlx::query_as::<_, LinearSyncConflict>(
        "SELECT * FROM linear_sync_conflict WHERE workspace_id = $1 AND status = 'open' ORDER BY created_at, id LIMIT $2",
    )
    .bind(workspace_id)
    .bind(limit.clamp(1, 500))
    .fetch_all(executor)
    .await?)
}

pub struct ConflictInput<'a> {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub issue_id: Option<Uuid>,
    pub linear_issue_id: Option<&'a str>,
    pub field: &'a str,
    pub local_value: Option<&'a Value>,
    pub remote_value: Option<&'a Value>,
    pub local_revision: Option<i64>,
    pub remote_updated_at: Option<DateTime<Utc>>,
    pub correlation_id: Option<Uuid>,
}
