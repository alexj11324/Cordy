//! Actor-scoped authorization decision explanations.

use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use patchbay_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::error_response;
use crate::issue::TaskAuthorizationContext;
use crate::state::HandlerState;
use patchbay_authorization::{
    Action, AuthorizationContext, AuthorizationRequest, Principal, PrincipalType, Resource,
    ResourceType, WorkspaceRole,
};

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/authorization/decisions/{decision_id}", get(explain))
        .route(
            "/api/authorization/provider-grants",
            get(list_provider_grants).post(create_provider_grant),
        )
        .route(
            "/api/authorization/provider-grants/{grant_id}",
            delete(revoke_provider_grant),
        )
        .route(
            "/api/authorization/provider-leases/validate",
            post(validate_provider_lease),
        )
}

#[derive(Debug, Deserialize)]
struct ValidateProviderLeaseRequest {
    runtime_id: Uuid,
    provider: String,
    model: String,
    requested_max_tokens: u64,
}

async fn load_provider_budget(
    pool: &PgPool,
    workspace_id: Uuid,
    lease_id: Uuid,
    task_id: Uuid,
    runtime_id: Uuid,
) -> anyhow::Result<u64> {
    let contexts = sqlx::query_scalar::<_, Value>(
        r#"SELECT context
FROM authorization_audit_event
WHERE workspace_id = $1
  AND principal_type = 'task_run' AND principal_id = $2
  AND action = 'credential.use' AND resource_type = 'provider_identity'
  AND resource_id = $3 AND decision = 'allow'
  AND context->>'lease_id' = $4
  AND context->>'provider_budget_reservation' = 'true'"#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(runtime_id)
    .bind(lease_id.to_string())
    .fetch_all(pool)
    .await?;
    sum_provider_token_reservations(&contexts)
}

fn sum_provider_token_reservations(contexts: &[Value]) -> anyhow::Result<u64> {
    contexts.iter().try_fold(0_u64, |reserved, context| {
        let tokens = context
            .get("provider_request_tokens")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("provider token reservation is invalid"))?;
        reserved
            .checked_add(tokens)
            .ok_or_else(|| anyhow::anyhow!("provider token reservations overflowed"))
    })
}

async fn validate_provider_lease(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Json(request): Json<ValidateProviderLeaseRequest>,
) -> Response {
    let Some(task_auth) = TaskAuthorizationContext::from_headers(&headers) else {
        return error_response(StatusCode::FORBIDDEN, "task capability lease required");
    };
    if task_auth.device_id != Some(request.runtime_id)
        || task_auth.on_behalf_of_user_id != Some(context.member.user_id)
    {
        return error_response(StatusCode::FORBIDDEN, "provider lease identity mismatch");
    }
    let row = sqlx::query(
        r#"SELECT runtime.owner_id, runtime.provider, runtime.workspace_id,
       task.agent_id, task.runtime_id
FROM agent_task_queue task
JOIN agent_runtime runtime ON runtime.id = task.runtime_id
WHERE task.id = $1 AND runtime.id = $2"#,
    )
    .bind(task_auth.task_id)
    .bind(request.runtime_id)
    .fetch_optional(&state.pool)
    .await;
    let row = match row {
        Ok(Some(row)) => row,
        Ok(None) => return error_response(StatusCode::FORBIDDEN, "provider lease task mismatch"),
        Err(error) => {
            tracing::error!(%error, "load provider lease task failed");
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "authorization unavailable");
        }
    };
    let provider: String = row.try_get("provider").unwrap_or_default();
    let workspace_id: Uuid = match row.try_get("workspace_id") {
        Ok(value) => value,
        Err(_) => {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "authorization unavailable")
        }
    };
    let agent_id: Uuid = match row.try_get("agent_id") {
        Ok(value) => value,
        Err(_) => {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "authorization unavailable")
        }
    };
    let owner_id: Option<Uuid> = row.try_get("owner_id").unwrap_or(None);
    if workspace_id != context.member.workspace_id
        || provider != request.provider
        || task_auth.via_agent_id != Some(agent_id)
    {
        return error_response(StatusCode::FORBIDDEN, "provider lease scope mismatch");
    }
    let team_ids = match patchbay_db::queries::team::list_teams_by_member(
        &state.pool,
        workspace_id,
        "member",
        context.member.user_id,
    )
    .await
    {
        Ok(teams) => teams.into_iter().map(|team| team.id).collect(),
        Err(error) => {
            tracing::error!(%error, "load provider lease teams failed");
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "authorization unavailable");
        }
    };
    let workspace_role = match context.member.role.as_str() {
        "owner" => WorkspaceRole::Owner,
        "admin" => WorkspaceRole::Admin,
        "member" => WorkspaceRole::Member,
        _ => WorkspaceRole::Guest,
    };
    let authorization_request =
        |requested_max_tokens: u64, provider_budget_reservation: bool| AuthorizationRequest {
            principal: Principal {
                principal_type: PrincipalType::TaskRun,
                id: Some(task_auth.task_id),
            },
            action: Action::new(Action::CREDENTIAL_USE),
            resource: Resource {
                resource_type: ResourceType::new(ResourceType::PROVIDER_IDENTITY),
                id: Some(request.runtime_id),
                workspace_id,
                owner_id,
                attributes: json!({
                    "private": true,
                    "provider": provider.clone(),
                    "provider_action": "provider.invoke",
                    "model": request.model.clone(),
                    "requested_max_tokens": requested_max_tokens,
                    "provider_request_tokens": request.requested_max_tokens,
                    "provider_budget_reservation": provider_budget_reservation,
                }),
            },
            context: AuthorizationContext {
                workspace_role: Some(workspace_role),
                on_behalf_of_user_id: task_auth.on_behalf_of_user_id,
                via_agent_id: task_auth.via_agent_id,
                device_id: task_auth.device_id,
                task_id: Some(task_auth.task_id),
                lease_id: Some(task_auth.lease_id),
                team_ids: team_ids.clone(),
                ..Default::default()
            },
            delegation_chain: Vec::new(),
        };
    // The first allow audit is the durable reservation. It commits before the
    // cumulative pass, so concurrent requests cannot both miss one another:
    // whichever final check runs second observes both reservations. Replays
    // after a daemon restart likewise remain charged to this lease.
    let initial_decision = match state
        .authorization
        .authorize(authorization_request(request.requested_max_tokens, true))
        .await
    {
        Ok(decision) => decision,
        Err(error) => {
            tracing::error!(%error, "validate provider lease failed closed");
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "authorization unavailable");
        }
    };
    let decision = if initial_decision.is_allowed() && owner_id != Some(context.member.user_id) {
        let cumulative_requested_tokens = match load_provider_budget(
            &state.pool,
            workspace_id,
            task_auth.lease_id,
            task_auth.task_id,
            request.runtime_id,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(%error, "load provider token budget failed");
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "authorization unavailable",
                );
            }
        };
        match state
            .authorization
            .authorize(authorization_request(cumulative_requested_tokens, false))
            .await
        {
            Ok(decision) => decision,
            Err(error) => {
                tracing::error!(%error, "validate cumulative provider budget failed closed");
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "authorization unavailable",
                );
            }
        }
    } else {
        initial_decision
    };
    match decision {
        decision if decision.is_allowed() => Json(json!({
            "allowed": true,
            "decision_id": decision.audit_id,
            "policy_version": decision.policy_version,
        }))
        .into_response(),
        decision => (
            StatusCode::FORBIDDEN,
            Json(json!({
                "allowed": false,
                "decision": decision.effect,
                "decision_id": decision.audit_id,
                "reason": decision.reason,
            })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct CreateProviderGrantRequest {
    grantee_type: String,
    grantee_id: Uuid,
    runtime_id: Uuid,
    #[serde(default)]
    allowed_actions: Vec<String>,
    #[serde(default)]
    models: Vec<String>,
    max_tokens: Option<u64>,
    expires_at: DateTime<Utc>,
    task_id: Option<Uuid>,
    #[serde(default = "default_effect")]
    effect: String,
}

fn default_effect() -> String {
    "allow".to_string()
}

#[derive(Debug, Serialize)]
struct ProviderGrantResponse {
    id: Uuid,
    workspace_id: Uuid,
    created_by: Option<Uuid>,
    grantee_type: String,
    grantee_id: Option<Uuid>,
    runtime_id: Option<Uuid>,
    provider: String,
    effect: String,
    conditions: Value,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

async fn create_provider_grant(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<CreateProviderGrantRequest>,
) -> Response {
    let actor_id = context.member.user_id;
    let workspace_id = context.member.workspace_id;
    if !matches!(
        request.grantee_type.as_str(),
        "user" | "team" | "agent_definition"
    ) {
        return error_response(StatusCode::BAD_REQUEST, "invalid provider grant grantee");
    }
    if !matches!(
        request.effect.as_str(),
        "allow" | "deny" | "require_approval"
    ) {
        return error_response(StatusCode::BAD_REQUEST, "invalid provider grant effect");
    }
    if request.allowed_actions.len() != 1
        || request.allowed_actions.first().map(String::as_str) != Some("provider.invoke")
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "provider grants currently support only provider.invoke",
        );
    }
    let now = Utc::now();
    if request.expires_at <= now || request.expires_at > now + Duration::days(30) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "provider grant expiry must be within the next 30 days",
        );
    }
    let mut models = request
        .models
        .into_iter()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    if models.is_empty() && request.max_tokens.is_none() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "provider grant requires models or a token budget",
        );
    }
    if request.max_tokens.is_some_and(|tokens| tokens == 0) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "provider token budget must be positive",
        );
    }

    let runtime = match sqlx::query(
        "SELECT owner_id, provider FROM agent_runtime WHERE id = $1 AND workspace_id = $2",
    )
    .bind(request.runtime_id)
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "runtime not found"),
        Err(error) => {
            tracing::error!(%error, "load provider grant runtime failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to create grant");
        }
    };
    let owner_id: Option<Uuid> = runtime.try_get("owner_id").unwrap_or(None);
    if owner_id != Some(actor_id) {
        return error_response(
            StatusCode::FORBIDDEN,
            "only the provider owner can grant use",
        );
    }
    let provider: String = runtime.try_get("provider").unwrap_or_default();
    if !matches!(provider.as_str(), "codex" | "claude") {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "provider credential broker is unavailable for this runtime",
        );
    }
    let grantee_exists = match request.grantee_type.as_str() {
        "user" => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM member WHERE workspace_id = $1 AND user_id = $2)",
        )
        .bind(workspace_id)
        .bind(request.grantee_id)
        .fetch_one(&state.pool)
        .await,
        "team" => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM team WHERE workspace_id = $1 AND id = $2 AND archived_at IS NULL)",
        )
        .bind(workspace_id)
        .bind(request.grantee_id)
        .fetch_one(&state.pool)
        .await,
        _ => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM agent WHERE workspace_id = $1 AND id = $2 AND archived_at IS NULL)",
        )
        .bind(workspace_id)
        .bind(request.grantee_id)
        .fetch_one(&state.pool)
        .await,
    };
    if !matches!(grantee_exists, Ok(true)) {
        return error_response(StatusCode::BAD_REQUEST, "provider grant grantee not found");
    }
    if let Some(task_id) = request.task_id {
        let valid_task = sqlx::query_scalar::<_, bool>(
            r#"SELECT EXISTS(
    SELECT 1 FROM agent_task_queue task
    JOIN agent ON agent.id = task.agent_id
    WHERE task.id = $1 AND agent.workspace_id = $2 AND task.runtime_id = $3
)"#,
        )
        .bind(task_id)
        .bind(workspace_id)
        .bind(request.runtime_id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(false);
        if !valid_task {
            return error_response(StatusCode::BAD_REQUEST, "provider grant task is invalid");
        }
    }
    let mut conditions = json!({
        "provider": provider,
        "provider_action": "provider.invoke",
        "device_id": request.runtime_id,
    });
    let map = conditions.as_object_mut().expect("object literal");
    if !models.is_empty() {
        map.insert("models".into(), json!(models));
    }
    if let Some(max_tokens) = request.max_tokens {
        map.insert("max_tokens".into(), json!(max_tokens));
    }
    if let Some(task_id) = request.task_id {
        map.insert("task_id".into(), json!(task_id));
    }
    let id = Uuid::now_v7();
    let inserted = sqlx::query(
        r#"INSERT INTO authorization_grant (
    id, workspace_id, principal_type, principal_id, action, resource_type,
    resource_id, effect, conditions, expires_at, created_by
) VALUES ($1,$2,$3,$4,'credential.use','provider_identity',$5,$6,$7,$8,$9)
RETURNING created_at"#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(&request.grantee_type)
    .bind(request.grantee_id)
    .bind(request.runtime_id)
    .bind(&request.effect)
    .bind(&conditions)
    .bind(request.expires_at)
    .bind(actor_id)
    .fetch_one(&state.pool)
    .await;
    let created_at: DateTime<Utc> = match inserted.and_then(|row| row.try_get("created_at")) {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "insert provider grant failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to create grant");
        }
    };
    (
        StatusCode::CREATED,
        Json(ProviderGrantResponse {
            id,
            workspace_id,
            created_by: Some(actor_id),
            grantee_type: request.grantee_type,
            grantee_id: Some(request.grantee_id),
            runtime_id: Some(request.runtime_id),
            provider,
            effect: request.effect,
            conditions,
            expires_at: Some(request.expires_at),
            revoked_at: None,
            created_at,
        }),
    )
        .into_response()
}

async fn list_provider_grants(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, created_by, principal_type, principal_id,
       resource_id, effect, conditions, expires_at, revoked_at, created_at
FROM authorization_grant
WHERE workspace_id = $1 AND resource_type = 'provider_identity'
  AND (created_by = $2 OR (principal_type = 'user' AND principal_id = $2)
       OR (principal_type = 'team' AND principal_id IN (
           SELECT team_id FROM team_member WHERE member_type = 'member' AND member_id = $2
       )))
ORDER BY created_at DESC"#,
    )
    .bind(context.member.workspace_id)
    .bind(context.member.user_id)
    .fetch_all(&state.pool)
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "list provider grants failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list grants");
        }
    };
    let grants = rows
        .into_iter()
        .filter_map(|row| {
            let conditions: Value = row.try_get("conditions").ok()?;
            Some(ProviderGrantResponse {
                id: row.try_get("id").ok()?,
                workspace_id: row.try_get("workspace_id").ok()?,
                created_by: row.try_get("created_by").unwrap_or(None),
                grantee_type: row.try_get("principal_type").ok()?,
                grantee_id: row.try_get("principal_id").unwrap_or(None),
                runtime_id: row.try_get("resource_id").unwrap_or(None),
                provider: conditions
                    .get("provider")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                effect: row.try_get("effect").ok()?,
                conditions,
                expires_at: row.try_get("expires_at").unwrap_or(None),
                revoked_at: row.try_get("revoked_at").unwrap_or(None),
                created_at: row.try_get("created_at").ok()?,
            })
        })
        .collect::<Vec<_>>();
    Json(json!({"grants": grants})).into_response()
}

async fn revoke_provider_grant(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(grant_id): Path<Uuid>,
) -> Response {
    let result = sqlx::query(
        r#"UPDATE authorization_grant grant
SET revoked_at = now(), revoked_by = $3, updated_at = now()
WHERE grant.id = $1 AND grant.workspace_id = $2
  AND grant.resource_type = 'provider_identity' AND grant.revoked_at IS NULL
  AND grant.created_by = $3"#,
    )
    .bind(grant_id)
    .bind(context.member.workspace_id)
    .bind(context.member.user_id)
    .execute(&state.pool)
    .await;
    match result {
        Ok(result) if result.rows_affected() == 1 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => error_response(StatusCode::NOT_FOUND, "provider grant not found"),
        Err(error) => {
            tracing::error!(%error, %grant_id, "revoke provider grant failed");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to revoke grant")
        }
    }
}

async fn explain(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let decision_id = match Uuid::parse_str(&raw_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid decision id"),
    };
    let event = match state
        .authorization
        .explain(decision_id, context.member.workspace_id)
        .await
    {
        Ok(Some(event)) => event,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "decision not found"),
        Err(error) => {
            tracing::error!(%error, %decision_id, "failed to explain authorization decision");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to explain authorization decision",
            );
        }
    };
    let actor_id = context.member.user_id;
    let actor_can_read = event.principal_id == Some(actor_id)
        || event.on_behalf_of_user_id == Some(actor_id)
        || context.member.role == "owner";
    if !actor_can_read {
        // Do not reveal that another principal's decision exists.
        return error_response(StatusCode::NOT_FOUND, "decision not found");
    }
    Json(event).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn provider_budget_is_loaded_from_durable_audit_rows() {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required for provider budget contract");
        let pool = PgPool::connect(&url)
            .await
            .expect("connect contract PostgreSQL");
        let workspace_id = Uuid::now_v7();
        let task_id = Uuid::now_v7();
        let runtime_id = Uuid::now_v7();
        let lease_id = Uuid::now_v7();
        let other_lease_id = Uuid::now_v7();
        sqlx::query(
            r#"INSERT INTO authorization_audit_event (
    id, workspace_id, principal_type, principal_id, action, resource_type,
    resource_id, decision, reason, policy_version, context
) VALUES
    ($1,$2,'task_run',$3,'credential.use','provider_identity',$4,'allow','reserved','phase1',$5),
    ($6,$2,'task_run',$3,'credential.use','provider_identity',$4,'allow','reserved','phase1',$7),
    ($8,$2,'task_run',$3,'credential.use','provider_identity',$4,'allow','other lease','phase1',$9)"#,
        )
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(task_id)
        .bind(runtime_id)
        .bind(json!({
            "lease_id": lease_id,
            "provider_request_tokens": 2_000,
            "provider_budget_reservation": true,
        }))
        .bind(Uuid::now_v7())
        .bind(json!({
            "lease_id": lease_id,
            "provider_request_tokens": 3_000,
            "provider_budget_reservation": true,
        }))
        .bind(Uuid::now_v7())
        .bind(json!({
            "lease_id": other_lease_id,
            "provider_request_tokens": 7_000,
            "provider_budget_reservation": true,
        }))
        .execute(&pool)
        .await
        .expect("persist provider budget reservations");

        let first_load =
            load_provider_budget(&pool, workspace_id, lease_id, task_id, runtime_id).await;
        let replacement_load =
            load_provider_budget(&pool, workspace_id, lease_id, task_id, runtime_id).await;
        sqlx::query("DELETE FROM authorization_audit_event WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("clean provider budget contract rows");

        assert_eq!(first_load.unwrap(), 5_000);
        assert_eq!(replacement_load.unwrap(), 5_000);
    }

    #[test]
    fn provider_token_reservations_are_cumulative_and_overflow_closed() {
        let contexts = vec![
            json!({"provider_request_tokens": 2_000}),
            json!({"provider_request_tokens": 3_000}),
        ];
        assert_eq!(sum_provider_token_reservations(&contexts).unwrap(), 5_000);
        assert!(sum_provider_token_reservations(&[json!({})]).is_err());
        assert!(sum_provider_token_reservations(&[
            json!({"provider_request_tokens": u64::MAX}),
            json!({"provider_request_tokens": 1}),
        ])
        .is_err());
    }
}
