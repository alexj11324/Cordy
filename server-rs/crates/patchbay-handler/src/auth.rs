//! Public passwordless and Google authentication endpoints.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{Duration, Utc};
use patchbay_db::models::User;
use patchbay_db::queries::{user, verification_code};
use rand::Rng;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::error_response;
use crate::state::HandlerState;

const SIGNUP_SOURCE_MAX_LEN: usize = 512;

#[derive(Debug, Clone)]
pub struct AuthSettings {
    allow_signup: bool,
    allowed_emails: Vec<String>,
    allowed_email_domains: Vec<String>,
    app_env: String,
    dev_verification_code: String,
    google_client_id: String,
    google_client_secret: String,
    google_redirect_uri: String,
    pub(crate) cookie_domain: String,
    pub(crate) frontend_origin: String,
}

impl AuthSettings {
    pub fn from_env() -> Self {
        Self {
            allow_signup: std::env::var("ALLOW_SIGNUP").as_deref() != Ok("false"),
            allowed_emails: split_env("ALLOWED_EMAILS"),
            allowed_email_domains: split_env("ALLOWED_EMAIL_DOMAINS"),
            app_env: env_trimmed("APP_ENV"),
            dev_verification_code: env_trimmed("PATCHBAY_DEV_VERIFICATION_CODE"),
            google_client_id: env_trimmed("GOOGLE_CLIENT_ID"),
            google_client_secret: env_trimmed("GOOGLE_CLIENT_SECRET"),
            google_redirect_uri: env_trimmed("GOOGLE_REDIRECT_URI"),
            cookie_domain: env_trimmed("COOKIE_DOMAIN"),
            frontend_origin: env_trimmed("FRONTEND_ORIGIN"),
        }
    }

    pub fn from_config(config: &patchbay_config::Config) -> Self {
        Self {
            allow_signup: config.auth.allow_signup.as_deref().map(str::trim) != Some("false"),
            allowed_emails: split_value(config.auth.allowed_emails.as_deref()),
            allowed_email_domains: split_value(config.auth.allowed_email_domains.as_deref()),
            app_env: option_trimmed(config.server.app_env.as_deref()),
            dev_verification_code: option_trimmed(config.auth.dev_verification_code.as_deref()),
            google_client_id: option_trimmed(config.auth.google_client_id.as_deref()),
            google_client_secret: option_trimmed(config.auth.google_client_secret.as_deref()),
            google_redirect_uri: option_trimmed(config.auth.google_redirect_uri.as_deref()),
            cookie_domain: option_trimmed(config.auth.cookie_domain.as_deref()),
            frontend_origin: option_trimmed(config.urls.frontend_origin.as_deref()),
        }
    }

    fn signup_allowed(&self, email: &str, is_new: bool) -> bool {
        if !is_new {
            return true;
        }
        let email = email.to_lowercase();
        let domain = email.split_once('@').map(|(_, value)| value).unwrap_or("");
        if contains_case_insensitive(&self.allowed_emails, &email)
            || contains_case_insensitive(&self.allowed_email_domains, domain)
        {
            return true;
        }
        self.allow_signup && self.allowed_emails.is_empty() && self.allowed_email_domains.is_empty()
    }

    fn is_dev_code(&self, code: &str) -> bool {
        !self.app_env.eq_ignore_ascii_case("production")
            && is_six_digit_code(&self.dev_verification_code)
            && constant_time_eq(code.as_bytes(), self.dev_verification_code.as_bytes())
    }

    pub(crate) fn cookie_attributes(&self) -> (Option<String>, bool) {
        (
            patchbay_auth::cookie::cookie_domain(Some(&self.cookie_domain)),
            patchbay_auth::cookie::is_secure_cookie(Some(&self.frontend_origin)),
        )
    }
}

pub fn public_router(
    auth_limit: patchbay_middleware::ratelimit::RateLimitState,
    verify_limit: patchbay_middleware::ratelimit::RateLimitState,
) -> Router<HandlerState> {
    let general = Router::new()
        .route("/auth/send-code", post(send_code))
        .route("/auth/google", post(google_login))
        .route_layer(axum::middleware::from_fn_with_state(
            auth_limit,
            patchbay_middleware::ratelimit::rate_limit,
        ));
    let verify = Router::new()
        .route("/auth/verify-code", post(verify_code))
        .route_layer(axum::middleware::from_fn_with_state(
            verify_limit,
            patchbay_middleware::ratelimit::rate_limit,
        ));
    general.merge(verify)
}

#[derive(Debug, Default, Deserialize)]
struct SendCodeRequest {
    #[serde(default, deserialize_with = "null_string_default")]
    email: String,
}

#[derive(Debug, Default, Deserialize)]
struct VerifyCodeRequest {
    #[serde(default, deserialize_with = "null_string_default")]
    email: String,
    #[serde(default, deserialize_with = "null_string_default")]
    code: String,
}

#[derive(Debug, Default, Deserialize)]
struct GoogleLoginRequest {
    #[serde(default, deserialize_with = "null_string_default")]
    code: String,
    #[serde(default, deserialize_with = "null_string_default")]
    redirect_uri: String,
}

#[derive(Debug, Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct GoogleUserInfo {
    #[serde(default)]
    email: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    picture: String,
}

#[derive(Debug, Serialize)]
struct LoginResponse {
    token: String,
    user: UserResponse,
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
    fn from_user(state: &HandlerState, value: &User) -> Self {
        Self {
            id: value.id.to_string(),
            name: value.name.clone(),
            email: value.email.clone(),
            avatar_url: value
                .avatar_url
                .as_deref()
                .map(|url| crate::avatar::resolve_url(state, url)),
            language: value.language.clone(),
            timezone: value.timezone.clone(),
            onboarded_at: value.onboarded_at.map(crate::timefmt::rfc3339),
            onboarding_questionnaire: value.onboarding_questionnaire.clone(),
            starter_content_state: value.starter_content_state.clone(),
            profile_description: value.profile_description.clone(),
            created_at: crate::timefmt::rfc3339(value.created_at),
            updated_at: crate::timefmt::rfc3339(value.updated_at),
        }
    }
}

async fn send_code(State(state): State<HandlerState>, body: Bytes) -> Response {
    let request: SendCodeRequest = match decode_first_json(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let email = request.email.trim().to_lowercase();
    if email.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "email is required");
    }
    if email.contains(['\r', '\n']) {
        return error_response(StatusCode::BAD_REQUEST, "invalid email");
    }
    if patchbay_auth::disabled_users::is_temporarily_disabled_user_email(&email) {
        return error_response(StatusCode::FORBIDDEN, "account disabled");
    }

    match user::get_user_by_email(&state.pool, &email).await {
        Ok(Some(existing)) => {
            if patchbay_auth::disabled_users::is_temporarily_disabled_user(
                &existing.id.to_string(),
                &existing.email,
            ) {
                return error_response(StatusCode::FORBIDDEN, "account disabled");
            }
        }
        Ok(None) => {
            if !state.auth_settings.signup_allowed(&email, true) {
                return error_response(
                    StatusCode::FORBIDDEN,
                    "user registration is disabled on this self-hosted instance",
                );
            }
        }
        Err(error) => {
            tracing::error!(%error, %email, "auth: failed to lookup user");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to lookup user");
        }
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, %email, "auth: failed to start send-code transaction");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to store verification code",
            );
        }
    };
    if let Err(error) = sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("auth-send-code:{email}"))
        .execute(&mut *tx)
        .await
    {
        tracing::error!(%error, %email, "auth: failed to lock send-code cooldown");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to store verification code",
        );
    }
    match verification_code::get_latest_code_by_email(&mut *tx, &email).await {
        Ok(Some(latest))
            if Utc::now().signed_duration_since(latest.created_at) < Duration::seconds(60) =>
        {
            return error_response(
                StatusCode::TOO_MANY_REQUESTS,
                "please wait before requesting another code",
            );
        }
        Ok(_) => {}
        Err(error) => {
            tracing::error!(%error, %email, "auth: failed to inspect send-code cooldown");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to store verification code",
            );
        }
    }

    let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000));
    match verification_code::create_verification_code(
        &mut *tx,
        &email,
        &code,
        Some(Utc::now() + Duration::minutes(10)),
    )
    .await
    {
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to store verification code",
            );
        }
    }
    if let Err(error) = tx.commit().await {
        tracing::error!(%error, %email, "auth: failed to commit send-code cooldown");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to store verification code",
        );
    }

    if let Err(error) = state
        .email_service
        .send_verification_code(&email, &code)
        .await
    {
        tracing::error!(%error, %email, "auth: failed to send verification code");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to send verification code",
        );
    }
    if let Err(error) = verification_code::delete_expired_verification_codes(&state.pool).await {
        tracing::debug!(%error, "auth: expired verification-code cleanup failed");
    }
    Json(serde_json::json!({"message": "Verification code sent"})).into_response()
}

async fn verify_code(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request: VerifyCodeRequest = match decode_first_json(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let email = request.email.trim().to_lowercase();
    let code = request.code.trim();
    if email.is_empty() || code.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "email and code are required");
    }
    if patchbay_auth::disabled_users::is_temporarily_disabled_user_email(&email) {
        return error_response(StatusCode::FORBIDDEN, "account disabled");
    }
    let db_code = match verification_code::get_latest_verification_code(&state.pool, &email).await {
        Ok(Some(code)) => code,
        Ok(None) | Err(_) => {
            return error_response(StatusCode::BAD_REQUEST, "invalid or expired code");
        }
    };
    if !state.auth_settings.is_dev_code(code)
        && !constant_time_eq(code.as_bytes(), db_code.code.as_bytes())
    {
        if let Err(error) =
            verification_code::reserve_verification_code_attempt(&state.pool, db_code.id).await
        {
            tracing::warn!(%error, "auth: failed to reserve verification attempt");
        }
        return error_response(StatusCode::BAD_REQUEST, "invalid or expired code");
    }
    match verification_code::mark_verification_code_used(&state.pool, db_code.id).await {
        Ok(true) => {}
        Ok(false) => {
            return error_response(StatusCode::BAD_REQUEST, "invalid or expired code");
        }
        Err(error) => {
            tracing::warn!(%error, "auth: failed to consume verification code");
            return error_response(StatusCode::BAD_REQUEST, "invalid or expired code");
        }
    }
    complete_login(&state, &headers, &email, None).await
}

async fn google_login(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request: GoogleLoginRequest = match decode_first_json(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.code.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "code is required");
    }
    if state.auth_settings.google_client_id.is_empty()
        || state.auth_settings.google_client_secret.is_empty()
    {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Google login is not configured",
        );
    }
    let redirect_uri = if request.redirect_uri.is_empty() {
        state.auth_settings.google_redirect_uri.as_str()
    } else {
        request.redirect_uri.as_str()
    };
    let client = reqwest::Client::new();
    let token_response = match client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("code", request.code.as_str()),
            ("client_id", state.auth_settings.google_client_id.as_str()),
            (
                "client_secret",
                state.auth_settings.google_client_secret.as_str(),
            ),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::error!(%error, "auth: Google token exchange failed");
            return error_response(
                StatusCode::BAD_GATEWAY,
                "failed to exchange code with Google",
            );
        }
    };
    if !token_response.status().is_success() {
        let status = token_response.status();
        let response_body = token_response.text().await.unwrap_or_default();
        tracing::error!(%status, body = %response_body, "auth: Google token exchange rejected");
        return error_response(
            StatusCode::BAD_REQUEST,
            "failed to exchange code with Google",
        );
    }
    let token: GoogleTokenResponse = match token_response.json().await {
        Ok(token) => token,
        Err(_) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "failed to parse Google token response",
            );
        }
    };
    let google_user: GoogleUserInfo = match client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .bearer_auth(&token.access_token)
        .send()
        .await
    {
        Ok(response) => match response.json().await {
            Ok(user) => user,
            Err(_) => {
                return error_response(StatusCode::BAD_GATEWAY, "failed to parse Google user info");
            }
        },
        Err(error) => {
            tracing::error!(%error, "auth: Google userinfo fetch failed");
            return error_response(
                StatusCode::BAD_GATEWAY,
                "failed to fetch user info from Google",
            );
        }
    };
    if google_user.email.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Google account has no email");
    }
    let email = google_user.email.trim().to_lowercase();
    complete_login(&state, &headers, &email, Some(google_user)).await
}

async fn complete_login(
    state: &HandlerState,
    headers: &HeaderMap,
    email: &str,
    google_profile: Option<GoogleUserInfo>,
) -> Response {
    if patchbay_auth::disabled_users::is_temporarily_disabled_user_email(email) {
        return error_response(StatusCode::FORBIDDEN, "account disabled");
    }
    let (mut current, is_new) = match user::get_user_by_email(&state.pool, email).await {
        Ok(Some(user)) => {
            if patchbay_auth::disabled_users::is_temporarily_disabled_user(
                &user.id.to_string(),
                &user.email,
            ) {
                return error_response(StatusCode::FORBIDDEN, "account disabled");
            }
            (user, false)
        }
        Ok(None) => {
            if !state.auth_settings.signup_allowed(email, true) {
                return error_response(
                    StatusCode::FORBIDDEN,
                    "user registration is disabled on this self-hosted instance",
                );
            }
            let name = email.split_once('@').map(|(name, _)| name).unwrap_or(email);
            match user::create_user(&state.pool, name, email, None).await {
                Ok(Some(user)) => (user, true),
                Ok(None) | Err(_) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to create user",
                    );
                }
            }
        }
        Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to create user");
        }
    };

    if is_new {
        let mut event = patchbay_analytics::signup(
            &current.id.to_string(),
            &current.email,
            &signup_source(headers),
        );
        if google_profile.is_some() {
            event
                .properties
                .get_or_insert_default()
                .insert("auth_method".into(), serde_json::json!("google"));
        }
        patchbay_metrics::business_events::record_event(
            Some(state.analytics.as_ref()),
            state.business_metrics.as_deref(),
            &event,
        );
    }
    if let Some(profile) = google_profile {
        let email_prefix = email.split_once('@').map(|(name, _)| name).unwrap_or(email);
        let new_name = if !profile.name.is_empty() && current.name == email_prefix {
            profile.name.as_str()
        } else {
            current.name.as_str()
        };
        let avatar = if !profile.picture.is_empty() && current.avatar_url.is_none() {
            Some(profile.picture.as_str())
        } else {
            None
        };
        if new_name != current.name || avatar.is_some() {
            if let Ok(Some(updated)) =
                user::update_user(&state.pool, current.id, new_name, avatar, None, None, None).await
            {
                current = updated;
            }
        }
    }
    let token = match patchbay_auth::jwt::issue_user_jwt(
        &current.id.to_string(),
        &current.email,
        &current.name,
    ) {
        Ok(token) => token,
        Err(error) => {
            tracing::warn!(%error, email, "auth: failed to issue login JWT");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to generate token",
            );
        }
    };
    let (domain, secure) = state.auth_settings.cookie_attributes();
    let cookies =
        match patchbay_auth::cookie::set_auth_cookie_values(&token, domain.as_deref(), secure) {
            Ok(cookies) => cookies,
            Err(error) => {
                tracing::warn!(%error, "auth: failed to set auth cookies");
                return Json(LoginResponse {
                    token,
                    user: UserResponse::from_user(state, &current),
                })
                .into_response();
            }
        };
    let mut response = Json(LoginResponse {
        token,
        user: UserResponse::from_user(state, &current),
    })
    .into_response();
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    if let Some(signer) = state.attachment_download.cloudfront_signer.as_ref() {
        match signer.signed_cookie_headers(crate::cloudfront::cloudfront_cookie_expiry(Utc::now()))
        {
            Ok(cookies) => {
                for cookie in cookies {
                    if let Ok(value) = HeaderValue::from_str(&cookie) {
                        response.headers_mut().append(header::SET_COOKIE, value);
                    }
                }
            }
            Err(error) => tracing::warn!(%error, "auth: failed to sign CloudFront cookies"),
        }
    }
    response
}

fn env_trimmed(name: &str) -> String {
    std::env::var(name).unwrap_or_default().trim().to_string()
}

fn split_env(name: &str) -> Vec<String> {
    split_value(Some(&env_trimmed(name)))
}

fn split_value(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn option_trimmed(value: Option<&str>) -> String {
    value.unwrap_or_default().trim().to_string()
}

fn contains_case_insensitive(values: &[String], expected: &str) -> bool {
    values
        .iter()
        .any(|value| value.eq_ignore_ascii_case(expected))
}

fn is_six_digit_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn decode_first_json<T>(body: &[u8]) -> Result<T, serde_json::Error>
where
    T: Default + DeserializeOwned,
{
    let mut decoder = serde_json::Deserializer::from_slice(body);
    Ok(Option::<T>::deserialize(&mut decoder)?.unwrap_or_default())
}

fn null_string_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

fn signup_source(headers: &HeaderMap) -> String {
    let Some(raw_cookie) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
    else {
        return String::new();
    };
    let Some(value) = raw_cookie.split(';').find_map(|cookie| {
        let (name, value) = cookie.trim().split_once('=')?;
        (name == "patchbay_signup_source" || name == "cordy_signup_source") // legacy-brand-compat
            .then_some(value)
    }) else {
        return String::new();
    };
    let encoded = format!("value={value}");
    let decoded = url::form_urlencoded::parse(encoded.as_bytes())
        .find_map(|(key, value)| (key == "value").then(|| value.into_owned()))
        .unwrap_or_default();
    if decoded.len() > SIGNUP_SOURCE_MAX_LEN {
        String::new()
    } else {
        decoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn state() -> HandlerState {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        HandlerState::new(pool, patchbay_auth::pat_cache::PatCache::disabled(), None)
    }

    #[tokio::test]
    async fn public_validation_matches_go_errors_without_db_access() {
        let state = state();
        let app = public_router(
            state.auth_rate_limit.clone(),
            state.auth_verify_rate_limit.clone(),
        )
        .with_state(state);
        for (path, body, status, message) in [
            (
                "/auth/send-code",
                "{}",
                StatusCode::BAD_REQUEST,
                "email is required",
            ),
            (
                "/auth/verify-code",
                "{}",
                StatusCode::BAD_REQUEST,
                "email and code are required",
            ),
            (
                "/auth/google",
                r#"{"code":""}"#,
                StatusCode::BAD_REQUEST,
                "code is required",
            ),
            (
                "/auth/send-code",
                "null true",
                StatusCode::BAD_REQUEST,
                "email is required",
            ),
            (
                "/auth/send-code",
                r#"{"email":"victim@example.com\r\nRCPT TO:<attacker@example.com>"}"#,
                StatusCode::BAD_REQUEST,
                "invalid email",
            ),
            (
                "/auth/verify-code",
                r#"{"email":null,"code":null} []"#,
                StatusCode::BAD_REQUEST,
                "email and code are required",
            ),
            (
                "/auth/google",
                r#"{"code":null,"redirect_uri":null} {}"#,
                StatusCode::BAD_REQUEST,
                "code is required",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(Request::post(path).body(Body::from(body)).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{path}");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"],
                message
            );
        }
    }

    #[test]
    fn signup_gating_matches_go_precedence() {
        let mut settings = AuthSettings::from_env();
        settings.allow_signup = false;
        settings.allowed_emails = vec!["boss@example.com".into()];
        settings.allowed_email_domains = vec!["company.com".into()];
        assert!(settings.signup_allowed("existing@other.com", false));
        assert!(settings.signup_allowed("BOSS@example.com", true));
        assert!(settings.signup_allowed("user@company.com", true));
        assert!(!settings.signup_allowed("new@other.com", true));
    }

    #[test]
    fn production_config_never_accepts_the_dev_verification_code() {
        let mut config = patchbay_config::Config::default();
        config.server.app_env = Some(" Production ".into());
        config.auth.dev_verification_code = Some(" 123456 ".into());
        config.auth.allow_signup = Some(" false ".into());
        let settings = AuthSettings::from_config(&config);
        assert!(!settings.is_dev_code("123456"));
        assert!(!settings.signup_allowed("new@example.com", true));
    }

    #[test]
    fn signup_source_decodes_and_caps_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static(
                "x=1; patchbay_signup_source=%7B%22utm_source%22%3A%22docs%22%7D",
            ),
        );
        assert_eq!(signup_source(&headers), r#"{"utm_source":"docs"}"#);
    }
}
