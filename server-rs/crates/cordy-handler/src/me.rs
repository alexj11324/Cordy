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
use structured_email_address::{Config as EmailConfig, EmailAddress};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/me", get(get_me).patch(update_me))
        .route("/api/me/onboarding", axum::routing::patch(patch_onboarding))
        .route(
            "/api/me/onboarding/complete",
            axum::routing::post(complete_onboarding),
        )
        .route(
            "/api/me/onboarding/cloud-waitlist",
            axum::routing::post(join_cloud_waitlist),
        )
}

const MAX_PROFILE_DESCRIPTION_LEN: usize = 2_000;
const MAX_CLOUD_WAITLIST_EMAIL_LEN: usize = 254;
const MAX_CLOUD_WAITLIST_REASON_LEN: usize = 500;
const PATCH_ONBOARDING_BODY_LIMIT: usize = 16 * 1024;
const QUESTIONNAIRE_SCHEMA_VERSION: i64 = 2;

#[derive(Debug, Deserialize)]
struct UpdateMeRequest {
    name: Option<String>,
    avatar_url: Option<String>,
    language: Option<String>,
    profile_description: Option<String>,
    timezone: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PatchOnboardingRequest {
    questionnaire: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
struct JoinCloudWaitlistRequest {
    #[serde(default)]
    email: String,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Default, Deserialize)]
struct QuestionnaireAnswers {
    #[serde(default, deserialize_with = "deserialize_string_or_slice")]
    source: Vec<String>,
    #[serde(default)]
    source_other: String,
    #[serde(default)]
    source_skipped: bool,
    #[serde(default)]
    role: String,
    #[serde(default)]
    role_other: String,
    #[serde(default)]
    role_skipped: bool,
    #[serde(default, deserialize_with = "deserialize_string_or_slice")]
    use_case: Vec<String>,
    #[serde(default)]
    use_case_other: String,
    #[serde(default)]
    use_case_skipped: bool,
    #[serde(default)]
    version: i64,
}

impl QuestionnaireAnswers {
    fn source_resolved(&self) -> bool {
        !self.source.is_empty() || self.source_skipped
    }

    fn complete(&self) -> bool {
        self.version == QUESTIONNAIRE_SCHEMA_VERSION
            && (!self.role.is_empty() || self.role_skipped)
            && (!self.use_case.is_empty() || self.use_case_skipped)
    }
}

#[derive(Debug, Default, Deserialize)]
struct CompleteOnboardingRequest {
    #[serde(default)]
    completion_path: String,
    #[serde(default)]
    workspace_id: String,
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

impl UserResponse {
    /// Resolves the durable object URL at read time, matching Go's
    /// `resolveAvatarURLPtr` used by `userToResponse`. Persisted user rows
    /// keep the raw object URL; private object storage receives the same
    /// signed capability endpoint as other user-facing resources.
    fn resolve_avatar_url(&mut self, state: &HandlerState) {
        self.avatar_url = self
            .avatar_url
            .take()
            .map(|raw| crate::avatar::resolve_url(state, &raw));
    }
}

fn resolved_user_response(state: &HandlerState, user: &User) -> UserResponse {
    let mut response = UserResponse::from(user);
    response.resolve_avatar_url(state);
    response
}

async fn get_me(State(state): State<HandlerState>, headers: HeaderMap) -> Response {
    let user_id = match authenticated_user_id(&headers) {
        Some(user_id) => user_id,
        None => return error_response(StatusCode::UNAUTHORIZED, "user not authenticated"),
    };
    match user::get_user(&state.pool, user_id).await {
        Ok(Some(user)) => Json(resolved_user_response(&state, &user)).into_response(),
        Ok(None) | Err(_) => error_response(StatusCode::NOT_FOUND, "user not found"),
    }
}

async fn update_me(State(state): State<HandlerState>, headers: HeaderMap, body: Bytes) -> Response {
    if is_machine_actor_source(&headers) {
        return error_response(
            StatusCode::FORBIDDEN,
            "this endpoint is only available to human actors",
        );
    }
    let user_id = match authenticated_user_id(&headers) {
        Some(user_id) => user_id,
        None => return error_response(StatusCode::UNAUTHORIZED, "user not authenticated"),
    };
    let request: UpdateMeRequest = match decode_json_body(&body) {
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
        Ok(Some(user)) => Json(resolved_user_response(&state, &user)).into_response(),
        Ok(None) | Err(_) => {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update user")
        }
    }
}

async fn patch_onboarding(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let user_id = match authenticated_user_id(&headers) {
        Some(user_id) => user_id,
        None => return error_response(StatusCode::UNAUTHORIZED, "user not authenticated"),
    };
    if body.len() > PATCH_ONBOARDING_BODY_LIMIT {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    }
    let request: PatchOnboardingRequest = match decode_json_body(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };

    let before_user = user::get_user(&state.pool, user_id).await.ok().flatten();
    let before_raw = before_user
        .as_ref()
        .map(|user| user.onboarding_questionnaire.clone())
        .unwrap_or_else(|| serde_json::json!({}));
    let before: QuestionnaireAnswers =
        serde_json::from_value(before_raw.clone()).unwrap_or_default();
    let first_touch = before_raw.is_null()
        || before_raw
            .as_object()
            .is_some_and(serde_json::Map::is_empty);

    let updated =
        match user::patch_user_onboarding(&state.pool, request.questionnaire.as_ref(), user_id)
            .await
        {
            Ok(Some(user)) => user,
            Ok(None) | Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to update onboarding",
                )
            }
        };

    if first_touch
        && request
            .questionnaire
            .as_ref()
            .is_some_and(|value| !value.is_null() && value != &serde_json::json!({}))
    {
        let platform = headers
            .get("x-client-platform")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        record_metric_event(
            &state,
            &cordy_analytics::onboarding_started(&user_id.to_string(), platform),
        );
    }

    let after: QuestionnaireAnswers =
        serde_json::from_value(updated.onboarding_questionnaire.clone()).unwrap_or_default();
    if after.complete() && !before.complete() {
        record_metric_event(
            &state,
            &cordy_analytics::onboarding_questionnaire_submitted(
                &user_id.to_string(),
                after.source.clone(),
                &after.role,
                after.use_case.clone(),
                after.source_skipped,
                after.role_skipped,
                after.use_case_skipped,
                !after.source_other.is_empty(),
                !after.role_other.is_empty(),
                !after.use_case_other.is_empty(),
            ),
        );
    }
    if after.version == QUESTIONNAIRE_SCHEMA_VERSION
        && after.source_resolved()
        && !before.source_resolved()
    {
        record_metric_event(
            &state,
            &cordy_analytics::onboarding_source_submitted(
                &user_id.to_string(),
                after.source.clone(),
                after.source_skipped,
                !after.source_other.is_empty(),
            ),
        );
    }

    Json(resolved_user_response(&state, &updated)).into_response()
}

async fn complete_onboarding(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let user_id = match authenticated_user_id(&headers) {
        Some(user_id) => user_id,
        None => return error_response(StatusCode::UNAUTHORIZED, "user not authenticated"),
    };
    let mut request = if body.iter().all(u8::is_ascii_whitespace) {
        CompleteOnboardingRequest::default()
    } else {
        match decode_json_body::<Option<CompleteOnboardingRequest>>(&body) {
            Ok(Some(request)) => request,
            Ok(None) => CompleteOnboardingRequest::default(),
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
        }
    };
    if !request.workspace_id.is_empty() {
        request.workspace_id = match Uuid::parse_str(request.workspace_id.trim()) {
            Ok(workspace_id) => workspace_id.to_string(),
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace_id"),
        };
    }

    let before = match user::get_user(&state.pool, user_id).await {
        Ok(Some(user)) => user,
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to complete onboarding",
            )
        }
    };
    let first_completion = before.onboarded_at.is_none();
    let updated = match user::mark_user_onboarded(&state.pool, user_id).await {
        Ok(Some(user)) => user,
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to complete onboarding",
            )
        }
    };

    if first_completion {
        let completion_path = match request.completion_path.as_str() {
            cordy_analytics::ONBOARDING_PATH_FULL
            | cordy_analytics::ONBOARDING_PATH_RUNTIME_SKIPPED
            | cordy_analytics::ONBOARDING_PATH_CLOUD_WAITLIST
            | cordy_analytics::ONBOARDING_PATH_SKIP_EXISTING
            | cordy_analytics::ONBOARDING_PATH_INVITE_ACCEPT => request.completion_path.as_str(),
            _ => cordy_analytics::ONBOARDING_PATH_UNKNOWN,
        };
        let onboarded_at = updated
            .onboarded_at
            .map(crate::timefmt::rfc3339)
            .unwrap_or_default();
        record_metric_event(
            &state,
            &cordy_analytics::onboarding_completed(
                &user_id.to_string(),
                &request.workspace_id,
                completion_path,
                &onboarded_at,
                updated.cloud_waitlist_email.is_some(),
            ),
        );
    }

    Json(resolved_user_response(&state, &updated)).into_response()
}

async fn join_cloud_waitlist(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let user_id = match authenticated_user_id(&headers) {
        Some(user_id) => user_id,
        None => return error_response(StatusCode::UNAUTHORIZED, "user not authenticated"),
    };
    let request: JoinCloudWaitlistRequest = match decode_json_body(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };

    let email = request.email.trim().to_lowercase();
    if email.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "email is required");
    }
    if email.len() > MAX_CLOUD_WAITLIST_EMAIL_LEN {
        return error_response(StatusCode::BAD_REQUEST, "email is too long");
    }
    if !valid_email_address(&email) {
        return error_response(StatusCode::BAD_REQUEST, "email is invalid");
    }

    let reason = request.reason.trim();
    if reason.len() > MAX_CLOUD_WAITLIST_REASON_LEN {
        return error_response(StatusCode::BAD_REQUEST, "reason is too long");
    }
    let reason = (!reason.is_empty()).then_some(reason);

    let updated = match user::join_cloud_waitlist(&state.pool, user_id, Some(&email), reason).await
    {
        Ok(Some(user)) => user,
        Ok(None) | Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to join waitlist")
        }
    };

    record_metric_event(
        &state,
        &cordy_analytics::cloud_waitlist_joined(&user_id.to_string(), reason.is_some()),
    );
    Json(resolved_user_response(&state, &updated)).into_response()
}

fn valid_email_address(value: &str) -> bool {
    let config = EmailConfig::builder()
        .allow_display_name()
        .allow_domain_literal()
        .allow_single_label_domain()
        .build();
    if EmailAddress::parse_with(value, &config).is_err() {
        return false;
    }

    // structured-email-address accepts RFC 5322 comments inside an addr-spec,
    // while Go's net/mail.ParseAddress only accepts comments in the display
    // name or after a bare address. Validate the bare prefix separately so the
    // migration keeps the existing wire contract.
    if let (Some(start), Some(end)) = (first_unquoted(value, '<'), first_unquoted(value, '>')) {
        return start < end && first_unquoted(&value[start + 1..end], '(').is_none();
    }
    match first_unquoted(value, '(') {
        Some(comment_start) => {
            let address = value[..comment_start].trim_end();
            !address.is_empty() && EmailAddress::parse_with(address, &config).is_ok()
        }
        None => true,
    }
}

fn first_unquoted(value: &str, needle: char) -> Option<usize> {
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quoted {
            escaped = true;
            continue;
        }
        if ch == '"' {
            quoted = !quoted;
            continue;
        }
        if !quoted && ch == needle {
            return Some(index);
        }
    }
    None
}

fn record_metric_event(state: &HandlerState, event: &cordy_analytics::Event) {
    if let Some(metrics) = state.business_metrics.as_deref() {
        metrics.inc_for_event(event);
    }
}

fn deserialize_string_or_slice<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::String(value)) if value.is_empty() => Ok(Vec::new()),
        Some(serde_json::Value::String(value)) => Ok(vec![value]),
        Some(serde_json::Value::Array(values)) => values
            .into_iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| serde::de::Error::custom("expected string"))
            })
            .collect(),
        Some(_) => Err(serde::de::Error::custom("expected string or array")),
    }
}

fn decode_json_body<T>(body: &[u8]) -> Result<T, serde_json::Error>
where
    T: serde::de::DeserializeOwned,
{
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    T::deserialize(&mut deserializer)
}

fn authenticated_user_id(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-user-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn is_machine_actor_source(headers: &HeaderMap) -> bool {
    matches!(
        headers
            .get("x-actor-source")
            .and_then(|value| value.to_str().ok()),
        Some("task_token" | "cloud_pat")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

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

    #[tokio::test]
    async fn update_me_rejects_machine_credentials_before_database_access() {
        let state = HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        );
        let app = router().with_state(state);

        for actor_source in ["task_token", "cloud_pat"] {
            let response = app
                .clone()
                .oneshot(
                    Request::patch("/api/me")
                        .header("x-user-id", "018f946a-1234-7890-abcd-1234567890ab")
                        .header("x-actor-source", actor_source)
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"profile_description":"machine supplied"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{actor_source}");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                value["error"],
                "this endpoint is only available to human actors"
            );
        }
    }

    #[test]
    fn questionnaire_accepts_legacy_strings_and_current_arrays() {
        let legacy: QuestionnaireAnswers = serde_json::from_value(serde_json::json!({
            "source": "github",
            "role": "founder",
            "use_case": "coding",
            "version": 2
        }))
        .unwrap();
        assert_eq!(legacy.source, vec!["github"]);
        assert_eq!(legacy.use_case, vec!["coding"]);
        assert!(legacy.complete());

        let current: QuestionnaireAnswers = serde_json::from_value(serde_json::json!({
            "source": ["friend"],
            "role_skipped": true,
            "use_case": ["research", "coding"],
            "version": 2
        }))
        .unwrap();
        assert!(current.source_resolved());
        assert!(current.complete());
    }

    #[test]
    fn questionnaire_completion_is_scoped_to_schema_v2() {
        let answers: QuestionnaireAnswers = serde_json::from_value(serde_json::json!({
            "role": "founder",
            "use_case": ["coding"],
            "version": 3
        }))
        .unwrap();
        assert!(!answers.complete());
    }

    #[test]
    fn go_decoder_contract_accepts_first_json_value_and_legacy_null() {
        let update: UpdateMeRequest =
            decode_json_body(br#"{"name":"Alex"} {"ignored":true}"#).unwrap();
        assert_eq!(update.name.as_deref(), Some("Alex"));

        let complete: Option<CompleteOnboardingRequest> = decode_json_body(b"null").unwrap();
        assert!(complete.is_none());
    }

    #[test]
    fn cloud_waitlist_email_validation_accepts_go_mailbox_forms() {
        for email in [
            "alex@example.com",
            "alex+cordy@example.co.uk",
            "alex jiang <alex@example.com>",
            "alex (friend) <alex@example.com>",
            "alex@example.com (friend)",
            "\"alex jiang\"@example.com",
            "\"alex(friend)\"@example.com",
            "alex@[127.0.0.1]",
            "ü@example.com",
            "alex@例子.测试",
            "alex@localhost",
        ] {
            assert!(valid_email_address(email), "expected valid email: {email}");
        }
    }

    #[test]
    fn cloud_waitlist_email_validation_rejects_invalid_forms() {
        for email in [
            "invalid",
            "alex@",
            "@example.com",
            "alex example.com",
            "alex(comment)@example.com",
            "alex@example.com, bob@example.com",
            "alex..jiang@example.com",
        ] {
            assert!(
                !valid_email_address(email),
                "expected invalid email: {email}"
            );
        }
    }

    #[test]
    fn cloud_waitlist_request_matches_go_decoder_defaults() {
        let request: JoinCloudWaitlistRequest =
            decode_json_body(br#"{"email":" Alex@Example.COM ","reason":""} trailing"#).unwrap();
        assert_eq!(request.email.trim().to_lowercase(), "alex@example.com");
        assert!(request.reason.is_empty());

        let omitted: JoinCloudWaitlistRequest = decode_json_body(b"{}").unwrap();
        assert!(omitted.email.is_empty());
        assert!(omitted.reason.is_empty());
    }

    #[tokio::test]
    async fn cloud_waitlist_validation_matches_go_before_database_access() {
        let state = HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        );
        let app = router().with_state(state);
        let user_id = "018f946a-1234-7890-abcd-1234567890ab";
        let cases = vec![
            (String::new(), "invalid request body"),
            ("{}".to_string(), "email is required"),
            (r#"{"email":"invalid"}"#.to_string(), "email is invalid"),
            (
                format!(r#"{{"email":"{}@example.com"}}"#, "a".repeat(255)),
                "email is too long",
            ),
            (
                format!(
                    r#"{{"email":"alex@example.com","reason":"{}"}}"#,
                    "x".repeat(MAX_CLOUD_WAITLIST_REASON_LEN + 1)
                ),
                "reason is too long",
            ),
        ];

        for (body, expected_error) in cases {
            let response = app
                .clone()
                .oneshot(
                    Request::post("/api/me/onboarding/cloud-waitlist")
                        .header("x-user-id", user_id)
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["error"], expected_error);
        }
    }
}
