//! Workspace VCS connection management. Secrets are write-only: the access
//! token is never serialized and each webhook secret is returned only by the
//! create/rotate response that minted it.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use patchbay_db::models::VcsConnection;
use patchbay_db::queries::vcs;
use patchbay_middleware::workspace::WorkspaceContext;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::Url;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const WEBHOOK_PREFIX: &str = "/api/webhooks/vcs/";

pub fn member_router() -> Router<HandlerState> {
    Router::new().route("/api/workspaces/{id}/vcs/connections", get(list))
}

pub fn admin_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/workspaces/{id}/vcs/connections", post(connect))
        .route(
            "/api/workspaces/{id}/vcs/connections/{connection_id}",
            delete(remove),
        )
        .route(
            "/api/workspaces/{id}/vcs/connections/{connection_id}/rotate-webhook",
            post(rotate),
        )
}

#[derive(Serialize)]
struct ConnectionResponse {
    id: String,
    workspace_id: String,
    provider: String,
    instance_url: String,
    account_login: String,
    webhook_url: String,
    webhook_path: String,
    created_at: String,
}

#[derive(Serialize)]
struct ConnectResponse {
    #[serde(flatten)]
    connection: ConnectionResponse,
    webhook_secret: String,
}

fn public_url() -> String {
    std::env::var("PATCHBAY_PUBLIC_URL")
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_string()
}

fn response(row: VcsConnection) -> ConnectionResponse {
    let id = row.id.to_string();
    let webhook_path = format!("{WEBHOOK_PREFIX}{id}");
    let base = public_url();
    ConnectionResponse {
        id,
        workspace_id: row.workspace_id.to_string(),
        provider: row.provider,
        instance_url: row.instance_url,
        account_login: row.account_login,
        webhook_url: if base.is_empty() {
            String::new()
        } else {
            format!("{base}{webhook_path}")
        },
        webhook_path,
        created_at: crate::timefmt::rfc3339(row.created_at),
    }
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace id"))
}

fn require_available(state: &HandlerState) -> Result<(), Response> {
    if !state.vcs_integration_enabled {
        return Err(error_response(
            StatusCode::NOT_FOUND,
            "vcs integration is not available on this deployment",
        ));
    }
    if state.vcs_secret_box.is_none() {
        return Err(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "vcs integration not configured (PATCHBAY_VCS_SECRET_KEY unset)",
        ));
    }
    Ok(())
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let can_manage = matches!(context.member.role.as_str(), "owner" | "admin");
    if !state.vcs_integration_enabled {
        return Json(json!({
            "connections": [], "available": false, "configured": false, "can_manage": false
        }))
        .into_response();
    }
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match vcs::list_vcs_connections_by_workspace(&state.pool, workspace_id).await {
        Ok(rows) => Json(json!({
            "connections": rows.into_iter().map(response).collect::<Vec<_>>(),
            "available": true,
            "configured": state.vcs_secret_box.is_some(),
            "can_manage": can_manage,
        }))
        .into_response(),
        Err(error) => {
            tracing::error!(%error, %workspace_id, "failed to list VCS connections");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list connections",
            )
        }
    }
}

#[derive(Deserialize)]
struct ConnectRequest {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    instance_url: String,
    #[serde(default)]
    access_token: String,
}

fn seal(state: &HandlerState, plaintext: &str) -> Result<String, Response> {
    let secret_box = state.vcs_secret_box.as_ref().ok_or_else(|| {
        error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "vcs integration not configured",
        )
    })?;
    secret_box
        .seal(plaintext.as_bytes())
        .map(|value| STANDARD.encode(value))
        .map_err(|_| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to encrypt secret",
            )
        })
}

fn new_webhook_secret() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

async fn connect(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<ConnectRequest>,
) -> Response {
    if let Err(response) = require_available(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(provider) = patchbay_vcs::for_kind(request.provider.trim()) else {
        return error_response(StatusCode::BAD_REQUEST, "unsupported provider");
    };
    let instance_url = patchbay_vcs::normalize_instance_url(&request.instance_url);
    let token = request.access_token.trim();
    if instance_url.is_empty() || token.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "instance_url and access_token are required",
        );
    }
    let parsed = Url::parse(&instance_url);
    if !matches!(parsed.as_ref().map(Url::scheme), Ok("http" | "https"))
        || parsed.as_ref().ok().and_then(Url::host_str).is_none()
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "instance_url must be an absolute http(s) URL",
        );
    }
    let account = match provider.validate_token(&instance_url, token).await {
        Ok(account) => account,
        Err(error)
            if error
                .downcast_ref::<patchbay_vcs::UnauthorizedError>()
                .is_some() =>
        {
            return error_response(
                StatusCode::BAD_REQUEST,
                "the provider rejected the access token",
            )
        }
        Err(error) => {
            tracing::warn!(%error, %instance_url, "VCS token validation failed");
            return error_response(
                StatusCode::BAD_GATEWAY,
                "could not reach the provider instance",
            );
        }
    };
    let webhook_secret = new_webhook_secret();
    let access_token_encrypted = match seal(&state, token) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let webhook_secret_encrypted = match seal(&state, &webhook_secret) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match vcs::upsert_vcs_connection(
        &state.pool,
        workspace_id,
        provider.kind().0,
        &instance_url,
        &account.login,
        &access_token_encrypted,
        &webhook_secret_encrypted,
        context.member.user_id,
    )
    .await
    {
        Ok(Some(row)) => {
            let output = response(row);
            state.bus.publish(&patchbay_events::Event {
                event_type: patchbay_protocol::EVENT_VCS_CONNECTION_CREATED.into(),
                workspace_id: workspace_id.to_string(),
                actor_type: "system".into(),
                payload: json!({"id": output.id}),
                ..Default::default()
            });
            Json(ConnectResponse {
                connection: output,
                webhook_secret,
            })
            .into_response()
        }
        Ok(None) | Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to save connection",
        ),
    }
}

async fn remove(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, raw_id)): Path<(String, String)>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let connection_id = match Uuid::parse_str(&raw_id) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid connection id"),
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, %connection_id, "vcs: begin connection deletion failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to remove connection",
            );
        }
    };
    if let Err(error) =
        vcs::lock_vcs_connection(&mut *transaction, connection_id, workspace_id).await
    {
        tracing::warn!(%error, %connection_id, "vcs: lock connection failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to remove connection",
        );
    }
    if let Err(error) =
        vcs::lock_vcs_connection_work_products(&mut *transaction, connection_id, workspace_id).await
    {
        tracing::warn!(%error, %connection_id, "vcs: lock connection work products failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to remove connection",
        );
    }
    let affected_issue_ids = match vcs::list_issue_ids_for_vcs_connection_work_products(
        &mut *transaction,
        connection_id,
        workspace_id,
    )
    .await
    {
        Ok(issue_ids) => issue_ids,
        Err(error) => {
            tracing::warn!(%error, %connection_id, "vcs: list connection issue relations failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to remove connection",
            );
        }
    };
    if let Err(error) =
        vcs::delete_vcs_connection(&mut *transaction, connection_id, workspace_id).await
    {
        tracing::warn!(%error, %connection_id, "vcs: delete connection failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to remove connection",
        );
    }
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, %connection_id, "vcs: commit connection deletion failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to remove connection",
        );
    }
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::EVENT_VCS_CONNECTION_DELETED.into(),
        workspace_id: workspace_id.to_string(),
        actor_type: "system".into(),
        payload: json!({"id": raw_id}),
        ..Default::default()
    });
    for issue_id in affected_issue_ids {
        match patchbay_db::queries::issue::get_issue_in_workspace(
            &state.pool,
            issue_id,
            workspace_id,
        )
        .await
        {
            Ok(Some(issue)) => crate::vcs_webhook::maybe_complete_issue(&state, issue).await,
            Ok(None) => {}
            Err(error) => tracing::warn!(
                %error,
                %issue_id,
                %connection_id,
                "vcs: re-evaluate issue after connection deletion failed"
            ),
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn rotate(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, raw_id)): Path<(String, String)>,
) -> Response {
    if let Err(response) = require_available(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let connection_id = match Uuid::parse_str(&raw_id) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid connection id"),
    };
    match vcs::get_vcs_connection_by_id(&state.pool, connection_id).await {
        Ok(Some(row)) if row.workspace_id == workspace_id => {}
        _ => return error_response(StatusCode::NOT_FOUND, "vcs connection not found"),
    }
    let webhook_secret = new_webhook_secret();
    let encrypted = match seal(&state, &webhook_secret) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match vcs::rotate_vcs_connection_webhook_secret(
        &state.pool,
        connection_id,
        workspace_id,
        &encrypted,
    )
    .await
    {
        Ok(Some(row)) => Json(ConnectResponse {
            connection: response(row),
            webhook_secret,
        })
        .into_response(),
        _ => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to rotate webhook secret",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_shape_never_contains_stored_secrets() {
        let value = serde_json::to_value(ConnectionResponse {
            id: "id".into(),
            workspace_id: "ws".into(),
            provider: "gitlab".into(),
            instance_url: "https://git.example".into(),
            account_login: "alice".into(),
            webhook_url: String::new(),
            webhook_path: "/api/webhooks/vcs/id".into(),
            created_at: "now".into(),
        })
        .unwrap();
        assert!(value.get("access_token").is_none());
        assert!(value.get("webhook_secret").is_none());
    }

    #[test]
    fn webhook_secret_has_go_compatible_entropy_and_encoding() {
        let secret = new_webhook_secret();
        assert_eq!(secret.len(), 64);
        assert!(secret.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
