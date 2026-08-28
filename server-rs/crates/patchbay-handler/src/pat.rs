//! User-scoped personal access token endpoints.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{Duration, Utc};
use patchbay_auth::jwt::{generate_pat_token, hash_token};
use patchbay_db::models::PersonalAccessToken;
use patchbay_db::queries::personal_access_token as pat_q;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const RENEW_THRESHOLD_DAYS: i64 = 7;
const RENEW_EXTENSION_DAYS: i64 = 90;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/tokens", get(list).post(create))
        .route("/api/tokens/", get(list).post(create))
        .route("/api/tokens/current/renew", axum::routing::post(renew))
        .route("/api/tokens/{id}", axum::routing::delete(revoke))
}

fn user_id(headers: &HeaderMap) -> Result<Uuid, Response> {
    headers
        .get("x-user-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| error_response(StatusCode::UNAUTHORIZED, "user not authenticated"))
}

#[derive(Serialize)]
struct TokenResponse {
    id: Uuid,
    name: String,
    token_prefix: String,
    expires_at: Option<String>,
    last_used_at: Option<String>,
    created_at: String,
}

impl From<PersonalAccessToken> for TokenResponse {
    fn from(token: PersonalAccessToken) -> Self {
        Self {
            id: token.id,
            name: token.name,
            token_prefix: token.token_prefix,
            expires_at: token.expires_at.map(crate::timefmt::rfc3339),
            last_used_at: token.last_used_at.map(crate::timefmt::rfc3339),
            created_at: crate::timefmt::rfc3339(token.created_at),
        }
    }
}

#[derive(Deserialize)]
struct CreateRequest {
    name: String,
    expires_in_days: Option<i64>,
}

async fn create(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Json(request): Json<CreateRequest>,
) -> Response {
    let user_id = match user_id(&headers) {
        Ok(id) => id,
        Err(response) => return response,
    };
    if request.name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "name is required");
    }
    let raw = match generate_pat_token() {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "failed to generate PAT");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to generate token",
            );
        }
    };
    let expires_at = request
        .expires_in_days
        .filter(|days| *days > 0)
        .map(|days| Utc::now() + Duration::days(days));
    let prefix = raw.chars().take(12).collect::<String>();
    match pat_q::create_personal_access_token(
        &state.pool,
        user_id,
        &request.name,
        &hash_token(&raw),
        &prefix,
        expires_at,
    )
    .await
    {
        Ok(Some(token)) => {
            let response = TokenResponse::from(token);
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "id": response.id, "name": response.name, "token_prefix": response.token_prefix,
                    "expires_at": response.expires_at, "last_used_at": response.last_used_at,
                    "created_at": response.created_at, "token": raw,
                })),
            )
                .into_response()
        }
        Ok(None) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to create token"),
        Err(error) => {
            tracing::warn!(%error, "failed to create PAT");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to create token")
        }
    }
}

async fn list(State(state): State<HandlerState>, headers: HeaderMap) -> Response {
    let user_id = match user_id(&headers) {
        Ok(id) => id,
        Err(response) => return response,
    };
    match pat_q::list_personal_access_tokens_by_user(&state.pool, user_id).await {
        Ok(tokens) => Json(
            tokens
                .into_iter()
                .map(TokenResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to list PATs");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list tokens")
        }
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

async fn renew(State(state): State<HandlerState>, headers: HeaderMap) -> Response {
    let user_id = match user_id(&headers) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let Some(raw) = bearer(&headers).filter(|value| value.starts_with("pby_")) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "only personal access tokens can be renewed",
        );
    };
    let hash = hash_token(raw);
    let token = match pat_q::get_personal_access_token_by_hash(&state.pool, &hash).await {
        Ok(Some(token)) if token.user_id == user_id => token,
        Ok(Some(_)) => {
            return error_response(StatusCode::UNAUTHORIZED, "token does not belong to caller")
        }
        Ok(None) => return error_response(StatusCode::UNAUTHORIZED, "token is no longer valid"),
        Err(error) => {
            tracing::warn!(%error, "failed to lookup PAT");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to look up token");
        }
    };
    let Some(expires_at) = token.expires_at else {
        return Json(serde_json::json!({ "expires_at": "", "renewed": false })).into_response();
    };
    let now = Utc::now();
    if expires_at - now > Duration::days(RENEW_THRESHOLD_DAYS) {
        return Json(serde_json::json!({ "expires_at": crate::timefmt::rfc3339(expires_at), "renewed": false }))
            .into_response();
    }
    let new_expiry = now + Duration::days(RENEW_EXTENSION_DAYS);
    match pat_q::extend_personal_access_token_expiry(
        &state.pool,
        Some(new_expiry),
        token.id,
        Some(now + Duration::days(RENEW_THRESHOLD_DAYS)),
    )
    .await
    {
        Ok(Some(expiry)) => {
            Json(serde_json::json!({ "expires_at": expiry.map(crate::timefmt::rfc3339).unwrap_or_default(), "renewed": true })).into_response()
        }
        Ok(None) => match pat_q::get_personal_access_token_by_hash(&state.pool, &hash).await {
            Ok(Some(current)) => {
                Json(serde_json::json!({ "expires_at": current.expires_at.map(crate::timefmt::rfc3339).unwrap_or_default(), "renewed": false }))
                    .into_response()
            }
            _ => error_response(StatusCode::UNAUTHORIZED, "token is no longer valid"),
        },
        Err(error) => {
            tracing::warn!(%error, "failed to renew PAT");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to renew token")
        }
    }
}

async fn revoke(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user_id = match user_id(&headers) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let Ok(id) = Uuid::parse_str(&id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid token id");
    };
    match pat_q::revoke_personal_access_token(&state.pool, id, user_id).await {
        Ok(Some(hash)) => {
            state.pat_cache.invalidate(&hash).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to revoke PAT");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to revoke token")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_response_matches_go_timestamp_contract() {
        let created_at = chrono::DateTime::parse_from_rfc3339("2026-08-23T12:34:56.789Z")
            .unwrap()
            .with_timezone(&Utc);
        let response = TokenResponse::from(PersonalAccessToken {
            created_at,
            expires_at: Some(created_at),
            id: Uuid::nil(),
            last_used_at: None,
            name: "local".into(),
            revoked: false,
            token_hash: "hidden".into(),
            token_prefix: "pby_example".into(),
            user_id: Uuid::nil(),
        });
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["created_at"], "2026-08-23T12:34:56Z");
        assert_eq!(value["expires_at"], "2026-08-23T12:34:56Z");
        assert!(value["last_used_at"].is_null());
        assert!(value.get("token_hash").is_none());
        assert!(value.get("user_id").is_none());
    }
}
