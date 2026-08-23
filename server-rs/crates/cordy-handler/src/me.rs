//! Authenticated user profile endpoints.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::models::User;
use cordy_db::queries::user;
use serde::Serialize;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new().route("/api/me", get(get_me))
}

#[derive(Debug, Serialize)]
struct UserResponse {
    id: String,
    name: String,
    email: String,
    avatar_url: Option<String>,
    language: Option<String>,
    timezone: Option<String>,
    onboarded_at: Option<String>,
    onboarding_questionnaire: serde_json::Value,
    starter_content_state: Option<String>,
    profile_description: String,
    created_at: String,
    updated_at: String,
}

impl From<&User> for UserResponse {
    fn from(user: &User) -> Self {
        Self {
            id: user.id.to_string(),
            name: user.name.clone(),
            email: user.email.clone(),
            // Current Rust handler state has no storage signer. This is the
            // same raw-URL branch the Go handler uses when Storage/CFSigner
            // are nil; private URL signing lands with the storage slice.
            avatar_url: user.avatar_url.clone(),
            language: user.language.clone(),
            timezone: user.timezone.clone(),
            onboarded_at: user.onboarded_at.map(crate::timefmt::rfc3339),
            onboarding_questionnaire: user.onboarding_questionnaire.clone(),
            starter_content_state: user.starter_content_state.clone(),
            profile_description: user.profile_description.clone(),
            created_at: crate::timefmt::rfc3339(user.created_at),
            updated_at: crate::timefmt::rfc3339(user.updated_at),
        }
    }
}

async fn get_me(State(state): State<HandlerState>, headers: HeaderMap) -> Response {
    let user_id = match headers
        .get("x-user-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
    {
        Some(user_id) => user_id,
        None => return error_response(StatusCode::UNAUTHORIZED, "user not authenticated"),
    };
    match user::get_user(&state.pool, user_id).await {
        Ok(Some(user)) => Json(UserResponse::from(&user)).into_response(),
        Ok(None) | Err(_) => error_response(StatusCode::NOT_FOUND, "user not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_response_matches_go_nullable_and_timestamp_contract() {
        let user = User {
            avatar_url: None,
            cloud_waitlist_email: None,
            cloud_waitlist_reason: None,
            created_at: "2026-08-23T12:34:56.123Z".parse().unwrap(),
            email: "alex@example.com".into(),
            id: Uuid::parse_str("018f946a-1234-7890-abcd-1234567890ab").unwrap(),
            language: None,
            name: "Alex".into(),
            onboarded_at: None,
            onboarding_questionnaire: serde_json::json!({}),
            profile_description: String::new(),
            starter_content_state: None,
            timezone: None,
            updated_at: "2026-08-23T12:35:00.999Z".parse().unwrap(),
        };
        let value = serde_json::to_value(UserResponse::from(&user)).unwrap();
        assert_eq!(value["avatar_url"], serde_json::Value::Null);
        assert_eq!(value["onboarded_at"], serde_json::Value::Null);
        assert_eq!(value["onboarding_questionnaire"], serde_json::json!({}));
        assert_eq!(value["created_at"], "2026-08-23T12:34:56Z");
        assert_eq!(value["updated_at"], "2026-08-23T12:35:00Z");
    }
}
