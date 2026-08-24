//! Authenticated user profile endpoints.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono_tz::Tz;
use cordy_db::models::User;
use cordy_db::queries::user;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new().route("/api/me", get(get_me).patch(update_me))
}

const MAX_PROFILE_DESCRIPTION_LEN: usize = 2_000;

#[derive(Debug, Deserialize)]
struct UpdateMeRequest {
    name: Option<String>,
    avatar_url: Option<String>,
    language: Option<String>,
    profile_description: Option<String>,
    timezone: Option<String>,
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

impl UserResponse {
    fn from_user(state: &HandlerState, user: &User) -> Self {
        Self {
            id: user.id.to_string(),
            name: user.name.clone(),
            email: user.email.clone(),
            avatar_url: user
                .avatar_url
                .as_deref()
                .map(|url| crate::avatar::resolve_url(state, url)),
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
    let user_id = match authenticated_user_id(&headers) {
        Some(user_id) => user_id,
        None => return error_response(StatusCode::UNAUTHORIZED, "user not authenticated"),
    };
    match user::get_user(&state.pool, user_id).await {
        Ok(Some(user)) => Json(UserResponse::from_user(&state, &user)).into_response(),
        Ok(None) | Err(_) => error_response(StatusCode::NOT_FOUND, "user not found"),
    }
}

async fn update_me(State(state): State<HandlerState>, headers: HeaderMap, body: Bytes) -> Response {
    let user_id = match authenticated_user_id(&headers) {
        Some(user_id) => user_id,
        None => return error_response(StatusCode::UNAUTHORIZED, "user not authenticated"),
    };
    let request: UpdateMeRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let current_user = match user::get_user(&state.pool, user_id).await {
        Ok(Some(user)) => user,
        Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "user not found"),
    };

    let name = match request.name {
        Some(name) => {
            let name = name.trim().to_string();
            if name.is_empty() {
                return error_response(StatusCode::BAD_REQUEST, "name is required");
            }
            name
        }
        None => current_user.name.clone(),
    };
    let avatar_url = request.avatar_url.map(|value| value.trim().to_string());
    let language = match request.language {
        Some(language) => {
            let language = language.trim().to_string();
            if !matches!(language.as_str(), "en" | "zh-Hans" | "ko" | "ja") {
                return error_response(StatusCode::BAD_REQUEST, "unsupported language");
            }
            Some(language)
        }
        None => None,
    };
    let profile_description = match request.profile_description {
        Some(description) => {
            let description = description.trim().to_string();
            if description.chars().count() > MAX_PROFILE_DESCRIPTION_LEN {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "profile_description exceeds 2000 characters",
                );
            }
            Some(description)
        }
        None => None,
    };
    let timezone = match request.timezone {
        Some(timezone) => {
            let timezone = timezone.trim().to_string();
            if !timezone.is_empty() && timezone.parse::<Tz>().is_err() {
                return error_response(StatusCode::BAD_REQUEST, "invalid timezone");
            }
            Some(timezone)
        }
        None => None,
    };

    match user::update_user(
        &state.pool,
        current_user.id,
        &name,
        avatar_url.as_deref(),
        language.as_deref(),
        profile_description.as_deref(),
        timezone.as_deref(),
    )
    .await
    {
        Ok(Some(user)) => Json(UserResponse::from_user(&state, &user)).into_response(),
        Ok(None) | Err(_) => {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update user")
        }
    }
}

fn authenticated_user_id(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-user-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use super::*;
    use crate::attachment_storage::{AttachmentStorage, StoredObject};

    struct PrivateStorage;

    #[async_trait]
    impl AttachmentStorage for PrivateStorage {
        async fn upload(
            &self,
            _key: &str,
            _body: Vec<u8>,
            _content_type: &str,
            _filename: &str,
        ) -> anyhow::Result<String> {
            unreachable!()
        }

        async fn get(&self, _key: &str, _range: Option<&str>) -> anyhow::Result<StoredObject> {
            unreachable!()
        }

        async fn delete(&self, _key: &str) -> anyhow::Result<()> {
            unreachable!()
        }

        fn key_from_url(&self, raw: &str) -> Option<String> {
            raw.strip_prefix("https://objects.example/")
                .map(str::to_string)
        }

        fn object_url(&self, key: &str) -> String {
            format!("https://objects.example/{key}")
        }
    }

    fn state() -> HandlerState {
        let mut download = crate::state::AttachmentDownloadSettings::default();
        download.public_url = "https://api.example".into();
        HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        )
        .with_attachment_storage(Arc::new(PrivateStorage), download)
    }

    #[tokio::test]
    async fn user_response_matches_go_nullable_and_timestamp_contract() {
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
        let value = serde_json::to_value(UserResponse::from_user(&state(), &user)).unwrap();
        assert_eq!(value["avatar_url"], serde_json::Value::Null);
        assert_eq!(value["onboarded_at"], serde_json::Value::Null);
        assert_eq!(value["onboarding_questionnaire"], serde_json::json!({}));
        assert_eq!(value["created_at"], "2026-08-23T12:34:56Z");
        assert_eq!(value["updated_at"], "2026-08-23T12:35:00Z");
    }

    #[tokio::test]
    async fn user_response_hides_private_avatar_object_url() {
        let user = User {
            avatar_url: Some("https://objects.example/users/u/avatar.png".into()),
            cloud_waitlist_email: None,
            cloud_waitlist_reason: None,
            created_at: "2026-08-23T12:34:56Z".parse().unwrap(),
            email: "alex@example.com".into(),
            id: Uuid::nil(),
            language: None,
            name: "Alex".into(),
            onboarded_at: None,
            onboarding_questionnaire: serde_json::json!({}),
            profile_description: String::new(),
            starter_content_state: None,
            timezone: None,
            updated_at: "2026-08-23T12:35:00Z".parse().unwrap(),
        };

        let response = UserResponse::from_user(&state(), &user);
        let avatar_url = response.avatar_url.unwrap();
        assert!(avatar_url.starts_with("https://api.example/api/avatars/"));
        assert!(!avatar_url.contains("objects.example"));
    }

    #[test]
    fn update_request_preserves_omitted_and_explicit_empty_fields() {
        let omitted: UpdateMeRequest = serde_json::from_str("{}").unwrap();
        assert!(omitted.avatar_url.is_none());
        assert!(omitted.timezone.is_none());

        let explicit: UpdateMeRequest =
            serde_json::from_str(r#"{"avatar_url":"", "timezone":""}"#).unwrap();
        assert_eq!(explicit.avatar_url.as_deref(), Some(""));
        assert_eq!(explicit.timezone.as_deref(), Some(""));
    }

    #[test]
    fn supported_languages_and_iana_timezones_match_go_validation() {
        for language in ["en", "zh-Hans", "ko", "ja"] {
            assert!(matches!(language, "en" | "zh-Hans" | "ko" | "ja"));
        }
        assert!("America/New_York".parse::<Tz>().is_ok());
        assert!("not/a-timezone".parse::<Tz>().is_err());
    }

    #[test]
    fn profile_description_limit_counts_unicode_characters() {
        assert_eq!(
            "界".repeat(MAX_PROFILE_DESCRIPTION_LEN).chars().count(),
            2_000
        );
        assert_eq!(
            "界".repeat(MAX_PROFILE_DESCRIPTION_LEN + 1).chars().count(),
            2_001
        );
    }
}
