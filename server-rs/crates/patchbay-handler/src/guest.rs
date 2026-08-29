//! Server-backed guest sessions and formal-account migration.
//!
//! A guest is a normal persisted user for the duration of the session. The
//! only difference is the `is_guest` capability flag and the opaque bearer
//! token checked by middleware. This module never puts the long-lived guest
//! token into a browser URL; only a short-lived, one-time transfer token is
//! handed to the formal web login flow.

use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use chrono::{Duration, Utc};
use patchbay_db::queries::{guest as guest_queries, user};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::auth::{LoginResponse, UserResponse};
use crate::error::{error_code_response, error_response};
use crate::state::HandlerState;

const GUEST_NAME: &str = "Guest";
const TRANSFER_TTL: Duration = Duration::minutes(10);

pub fn public_router(
    auth_limit: patchbay_middleware::ratelimit::RateLimitState,
) -> Router<HandlerState> {
    Router::new()
        .route("/auth/guest", post(create_guest))
        .route_layer(axum::middleware::from_fn_with_state(
            auth_limit,
            patchbay_middleware::ratelimit::rate_limit,
        ))
}

pub fn authenticated_router() -> Router<HandlerState> {
    Router::new()
        .route("/auth/guest/transfer", post(create_transfer))
        .route("/auth/guest/claim", post(claim_guest))
}

#[derive(Debug, Serialize)]
struct TransferResponse {
    transfer_token: String,
}

#[derive(Debug, Default, Deserialize)]
struct ClaimRequest {
    #[serde(default)]
    transfer_token: String,
}

#[derive(Debug, Serialize)]
struct ClaimResponse {
    migrated_workspace_ids: Vec<String>,
}

async fn create_guest(State(state): State<HandlerState>) -> Response {
    let user_id = Uuid::now_v7();
    let session_id = Uuid::now_v7();
    let token = match patchbay_auth::jwt::generate_guest_token() {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(%error, "guest auth: failed to generate session token");
            return error_code_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "guest_unavailable",
                "guest session unavailable",
            );
        }
    };
    let email = format!("guest+{}@guest.patchbay.invalid", user_id.simple());
    let token_hash = patchbay_auth::jwt::hash_token(&token);
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, "guest auth: failed to start session transaction");
            return error_code_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "guest_unavailable",
                "guest session unavailable",
            );
        }
    };
    let guest_user = match user::create_guest_user(&mut *tx, user_id, GUEST_NAME, &email).await {
        Ok(Some(user)) => user,
        Ok(None) | Err(_) => {
            return error_code_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "guest_unavailable",
                "guest session unavailable",
            );
        }
    };
    if let Err(error) = guest_queries::create_guest_session(
        &mut *tx,
        session_id,
        user_id,
        &token_hash,
    )
    .await
    {
        tracing::error!(%error, "guest auth: failed to persist session");
        return error_code_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "guest_unavailable",
            "guest session unavailable",
        );
    }
    if let Err(error) = tx.commit().await {
        tracing::error!(%error, "guest auth: failed to commit session");
        return error_code_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "guest_unavailable",
            "guest session unavailable",
        );
    }
    Json(LoginResponse {
        token,
        user: UserResponse::from_user(&state, &guest_user),
    })
    .into_response()
}

async fn create_transfer(State(state): State<HandlerState>, headers: HeaderMap) -> Response {
    let guest_user_id = match authenticated_user_id(&headers) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let guest_user = match user::get_user(&state.pool, guest_user_id).await {
        Ok(Some(value)) => value,
        Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "user not found"),
    };
    if !guest_user.is_guest {
        return error_code_response(
            StatusCode::CONFLICT,
            "guest_session_required",
            "this account is already signed in",
        );
    }
    let session = match guest_queries::find_active_by_user_id(&state.pool, guest_user_id).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            return error_code_response(
                StatusCode::CONFLICT,
                "guest_session_unavailable",
                "guest session is no longer active",
            )
        }
        Err(error) => {
            tracing::warn!(%error, "guest auth: failed to load active session");
            return error_code_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "guest_unavailable",
                "guest session unavailable",
            );
        }
    };
    let transfer_token = match patchbay_auth::jwt::generate_guest_transfer_token() {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(%error, "guest auth: failed to generate transfer token");
            return error_code_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "guest_unavailable",
                "guest transfer unavailable",
            );
        }
    };
    if let Err(error) = guest_queries::create_transfer(
        &state.pool,
        Uuid::now_v7(),
        session.id,
        guest_user_id,
        &patchbay_auth::jwt::hash_token(&transfer_token),
        Utc::now() + TRANSFER_TTL,
    )
    .await
    {
        tracing::error!(%error, "guest auth: failed to persist transfer token");
        return error_code_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "guest_unavailable",
            "guest transfer unavailable",
        );
    }
    Json(TransferResponse { transfer_token }).into_response()
}

async fn claim_guest(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Json(request): Json<ClaimRequest>,
) -> Response {
    let formal_user_id = match authenticated_user_id(&headers) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let formal_user = match user::get_user(&state.pool, formal_user_id).await {
        Ok(Some(value)) => value,
        Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "user not found"),
    };
    if formal_user.is_guest {
        return error_code_response(
            StatusCode::FORBIDDEN,
            "formal_login_required",
            "formal login required",
        );
    }
    let transfer_token = request.transfer_token.trim();
    if !transfer_token.starts_with("pgt_") {
        return guest_transfer_error();
    }
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, "guest auth: failed to start claim transaction");
            return error_code_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "guest_unavailable",
                "guest transfer unavailable",
            );
        }
    };
    let transfer = match guest_queries::lock_transfer_by_hash(
        &mut *tx,
        &patchbay_auth::jwt::hash_token(transfer_token),
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) | Err(_) => return guest_transfer_error(),
    };
    let session = match guest_queries::lock_by_id(&mut *tx, transfer.guest_session_id).await {
        Ok(Some(value)) => value,
        Ok(None) | Err(_) => return guest_transfer_error(),
    };
    if session.user_id != transfer.guest_user_id {
        return guest_transfer_error();
    }
    if transfer.consumed_at.is_some() {
        if transfer.claimed_user_id == Some(formal_user_id) && session.status == "claimed" {
            // The migration and token issuance happen in separate requests.
            // A browser reload after a successful claim must be able to retry
            // the formal Desktop-token handoff without replaying the transfer.
            return Json(ClaimResponse {
                migrated_workspace_ids: Vec::new(),
            })
            .into_response();
        }
        return guest_transfer_error();
    }
    if session.status != "active" || transfer.expires_at <= Utc::now() {
        return guest_transfer_error();
    }
    let guest_user = match user::get_user_for_update(&mut *tx, transfer.guest_user_id).await {
        Ok(Some(value)) => value,
        Ok(None) | Err(_) => return guest_transfer_error(),
    };
    if !guest_user.is_guest || guest_user.id == formal_user_id {
        return guest_transfer_error();
    }
    let formal_user = match user::get_user_for_update(&mut *tx, formal_user_id).await {
        Ok(Some(value)) => value,
        Ok(None) | Err(_) => return guest_transfer_error(),
    };
    if formal_user.is_guest {
        return error_code_response(
            StatusCode::FORBIDDEN,
            "formal_login_required",
            "formal login required",
        );
    }
    let workspace_ids = match migrate_guest_data(&mut tx, guest_user.id, formal_user.id).await {
        Ok(ids) => ids,
        Err(error) => {
            tracing::error!(%error, "guest auth: guest data migration rolled back");
            return error_code_response(
                StatusCode::CONFLICT,
                "guest_transfer_failed",
                "guest data could not be migrated",
            );
        }
    };
    if !matches!(
        guest_queries::consume_transfer(&mut *tx, transfer.id, formal_user_id).await,
        Ok(true)
    ) || !matches!(
        guest_queries::claim_session(&mut *tx, session.id, formal_user_id).await,
        Ok(true)
    ) {
        return guest_transfer_error();
    }
    if let Err(error) = tx.commit().await {
        tracing::error!(%error, "guest auth: failed to commit guest claim");
        return error_code_response(
            StatusCode::CONFLICT,
            "guest_transfer_failed",
            "guest data could not be migrated",
        );
    }
    Json(ClaimResponse {
        migrated_workspace_ids: workspace_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect(),
    })
    .into_response()
}

fn authenticated_user_id(headers: &HeaderMap) -> Result<Uuid, Response> {
    headers
        .get("x-user-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value.trim()).ok())
        .ok_or_else(|| error_response(StatusCode::UNAUTHORIZED, "user not authenticated"))
}

fn guest_transfer_error() -> Response {
    error_code_response(
        StatusCode::GONE,
        "guest_transfer_invalid",
        "guest transfer is invalid or expired",
    )
}

/// Rebinds the guest's real workspace membership and account-scoped records.
/// This function deliberately contains no destructive cleanup: if a unique
/// constraint or a relationship invariant would be violated, the caller's
/// transaction rolls back and the guest session remains usable for retry.
async fn migrate_guest_data(
    tx: &mut Transaction<'_, Postgres>,
    guest_user_id: Uuid,
    formal_user_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let member_rows = sqlx::query(
        r#"SELECT workspace_id
           FROM member
           WHERE user_id = $1
           ORDER BY workspace_id
           FOR UPDATE"#,
    )
    .bind(guest_user_id)
    .fetch_all(&mut **tx)
    .await?;
    let workspace_ids: Vec<Uuid> = member_rows
        .iter()
        .map(|row| row.try_get(0))
        .collect::<Result<_, _>>()?;

    // Workspace slugs are globally unique in the current schema. Keep the
    // collision rule explicit for older/self-hosted data: only the guest
    // workspace is renamed, and the first free suffix wins.
    for workspace_id in &workspace_ids {
        let Some(row) = sqlx::query("SELECT slug FROM workspace WHERE id = $1 FOR UPDATE")
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await?
        else {
            continue;
        };
        let base_slug: String = row.try_get(0)?;
        let mut candidate = base_slug.clone();
        let mut suffix = 2_u32;
        loop {
            let taken: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM workspace WHERE slug = $1 AND id <> $2)",
            )
            .bind(&candidate)
            .bind(workspace_id)
            .fetch_one(&mut **tx)
            .await?;
            if !taken {
                break;
            }
            candidate = format!("{base_slug}-{suffix}");
            suffix = suffix.saturating_add(1);
        }
        if candidate != base_slug {
            sqlx::query("UPDATE workspace SET slug = $2, updated_at = now() WHERE id = $1")
                .bind(workspace_id)
                .bind(candidate)
                .execute(&mut **tx)
                .await?;
        }
    }

    // Ownership and settings move to the formal account. Historical issue,
    // comment, task-author and activity fields are intentionally untouched.
    for query in [
        "UPDATE agent SET owner_id = $1 WHERE owner_id = $2",
        "UPDATE agent_runtime SET owner_id = $1 WHERE owner_id = $2",
        "UPDATE chat_session SET creator_id = $1 WHERE creator_id = $2",
        "UPDATE chat_pinned_agent SET user_id = $1 WHERE user_id = $2",
        "UPDATE pinned_item SET user_id = $1 WHERE user_id = $2",
        "UPDATE notification_preference SET user_id = $1 WHERE user_id = $2",
        "UPDATE issue_view SET owner_id = $1 WHERE owner_id = $2",
        "UPDATE issue_view_preference SET user_id = $1 WHERE user_id = $2",
        "UPDATE workspace_mcp_server SET created_by = $1 WHERE created_by = $2",
        "UPDATE workspace_share_link SET created_by = $1 WHERE created_by = $2",
        "UPDATE workspace_channel SET created_by = $1 WHERE created_by = $2",
        "UPDATE task_token SET user_id = $1 WHERE user_id = $2",
        "UPDATE client_usage_daily SET user_id = $1 WHERE user_id = $2",
        "UPDATE feedback SET user_id = $1 WHERE user_id = $2",
    ] {
        sqlx::query(query)
            .bind(formal_user_id)
            .bind(guest_user_id)
            .execute(&mut **tx)
            .await?;
    }

    // Member-targeted inbox rows carry the member id directly. Preserve their
    // read, archived, and unread state while moving the recipient to the new
    // formal member created below.
    sqlx::query(
        "UPDATE inbox_item SET recipient_id = $1 WHERE recipient_type = 'member' AND recipient_id = $2",
    )
    .bind(formal_user_id)
    .bind(guest_user_id)
    .execute(&mut **tx)
    .await?;

    // The guest workspace is not an existing formal workspace, so the normal
    // case is a single safe UPDATE. A formal membership collision causes a
    // unique-constraint error and therefore a complete transaction rollback.
    sqlx::query("UPDATE member SET user_id = $1 WHERE user_id = $2")
        .bind(formal_user_id)
        .bind(guest_user_id)
        .execute(&mut **tx)
        .await?;

    Ok(workspace_ids)
}
