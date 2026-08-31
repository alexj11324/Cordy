//! Short-lived, PKCE-bound credentials for the desktop login deep link.
//!
//! The browser must not place a reusable Patchbay bearer in a custom-protocol
//! URL: another application can claim the scheme before Electron does. The
//! browser therefore mints a one-time code bound to a verifier held by the
//! initiating desktop renderer. Electron redeems that code over HTTPS and
//! receives the normal session bearer only after the binding is proven.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Json, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use base64::Engine;
use patchbay_db::queries::user;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const CODE_PREFIX: &str = "pbd_";
const CODE_BYTES: usize = 32;
const CODE_TTL_SECS: i64 = 5 * 60;
const CODE_TTL: Duration = Duration::from_secs(CODE_TTL_SECS as u64);
const REDIS_TIMEOUT: Duration = Duration::from_millis(250);
const REDIS_PREFIX: &str = "patchbay:auth:desktop:";
const ATTEMPT_REDIS_PREFIX: &str = "patchbay:auth:desktop-google-attempt:";
const ATTEMPT_PATH: &str = "/api/desktop-google/attempt";
const COMPLETE_PATH: &str = "/api/desktop-google/complete";
const BROKER_AUTH_HEADER: &str = "x-patchbay-desktop-broker-auth";
const BROKER_SECRET_HEX_BYTES: usize = 64;
const BROKER_SECRET_BYTES: usize = 32;
const REDIS_CONSUME_SCRIPT: &str = r#"
local value = redis.call("GET", KEYS[1])
if not value then return "" end
local separator = string.find(value, "|", 1, true)
if not separator then return "" end
if string.sub(value, 1, separator - 1) ~= ARGV[1] then return "__mismatch__" end
redis.call("DEL", KEYS[1])
return string.sub(value, separator + 1)
"#;
const REDIS_CONSUME_ATTEMPT_SCRIPT: &str = r#"
local value = redis.call("GET", KEYS[1])
if not value then return "" end
if value ~= ARGV[1] then return "__mismatch__" end
redis.call("DEL", KEYS[1])
return "1"
"#;

#[derive(Debug)]
pub struct DesktopHandoffStoreError;

#[derive(Clone)]
pub struct DesktopHandoffTokens {
    local: Arc<Mutex<HashMap<String, LocalGrant>>>,
    attempts: Arc<Mutex<HashMap<String, LocalAttempt>>>,
    redis: Option<patchbay_redis::RecoveringConnection>,
}

#[derive(Clone)]
pub struct DesktopHandoffIngressState {
    peer_limit: patchbay_middleware::ratelimit::RateLimitState,
    broker_secret: Option<[u8; BROKER_SECRET_BYTES]>,
}

impl DesktopHandoffIngressState {
    pub fn new(
        peer_limit: patchbay_middleware::ratelimit::RateLimitState,
        broker_secret: &str,
    ) -> Self {
        Self {
            peer_limit,
            broker_secret: decode_broker_secret(broker_secret),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum BrokerCredential {
    Direct,
    Valid,
    Invalid,
    Unconfigured,
}

#[derive(Clone, Copy)]
struct RejectBrokerCredential;

#[derive(Clone)]
struct LocalGrant {
    user_id: Uuid,
    code_challenge: String,
    expires_at: Instant,
}

#[derive(Clone)]
struct LocalAttempt {
    code_challenge: String,
    started_at_ms: i64,
    generation: String,
    expires_at: Instant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesktopGoogleAttempt {
    started_at_ms: i64,
    generation: String,
}

impl Default for DesktopHandoffTokens {
    fn default() -> Self {
        Self {
            local: Arc::new(Mutex::new(HashMap::new())),
            attempts: Arc::new(Mutex::new(HashMap::new())),
            redis: None,
        }
    }
}

impl DesktopHandoffTokens {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_connection(mut self, connection: patchbay_redis::RecoveringConnection) -> Self {
        self.redis = Some(connection);
        self
    }

    pub async fn issue(
        &self,
        user_id: Uuid,
        code_challenge: &str,
    ) -> Result<String, DesktopHandoffStoreError> {
        for _ in 0..3 {
            let code = generate_code();
            let key = redis_key(&code);
            if let Some(connection) = self.redis.clone() {
                let value = format!("{code_challenge}|{user_id}");
                let mut connection = connection;
                let result = tokio::time::timeout(
                    REDIS_TIMEOUT,
                    redis::cmd("SET")
                        .arg(key)
                        .arg(value)
                        .arg("EX")
                        .arg(CODE_TTL_SECS)
                        .arg("NX")
                        .query_async::<Option<String>>(&mut connection),
                )
                .await
                .map_err(|_| DesktopHandoffStoreError)?
                .map_err(|_| DesktopHandoffStoreError)?;
                if result.is_some() {
                    return Ok(code);
                }
                continue;
            }

            let mut issued = self.local.lock().unwrap_or_else(|e| e.into_inner());
            sweep_local(&mut issued);
            issued.insert(
                hash_code(&code),
                LocalGrant {
                    user_id,
                    code_challenge: code_challenge.to_string(),
                    expires_at: Instant::now() + CODE_TTL,
                },
            );
            return Ok(code);
        }

        Err(DesktopHandoffStoreError)
    }

    pub async fn consume(
        &self,
        code: &str,
        code_challenge: &str,
    ) -> Result<Option<Uuid>, DesktopHandoffStoreError> {
        if !valid_code(code, CODE_PREFIX) {
            return Ok(None);
        }

        if let Some(connection) = self.redis.clone() {
            let mut connection = connection;
            let value = tokio::time::timeout(
                REDIS_TIMEOUT,
                redis::cmd("EVAL")
                    .arg(REDIS_CONSUME_SCRIPT)
                    .arg(1)
                    .arg(redis_key(code))
                    .arg(code_challenge)
                    .query_async::<String>(&mut connection),
            )
            .await
            .map_err(|_| DesktopHandoffStoreError)?
            .map_err(|_| DesktopHandoffStoreError)?;
            if value.is_empty() || value == "__mismatch__" {
                return Ok(None);
            }
            return Ok(Uuid::parse_str(&value).ok());
        }

        let mut issued = self.local.lock().unwrap_or_else(|e| e.into_inner());
        sweep_local(&mut issued);
        let key = hash_code(code);
        let Some(grant) = issued.get(&key) else {
            return Ok(None);
        };
        if grant.code_challenge != code_challenge {
            return Ok(None);
        }
        let Some(grant) = issued.remove(&key) else {
            return Ok(None);
        };
        if grant.expires_at <= Instant::now() {
            return Ok(None);
        }
        Ok(Some(grant.user_id))
    }

    /// Registers the renderer binding before any Google redirect. Repeating
    /// the exact state/challenge is idempotent so the Clerk sign-out reload
    /// retains the original server-side start time.
    pub async fn register_google_attempt(
        &self,
        state: &str,
        code_challenge: &str,
    ) -> Result<Option<DesktopGoogleAttempt>, DesktopHandoffStoreError> {
        let key = attempt_redis_key(state);
        let attempt = DesktopGoogleAttempt {
            started_at_ms: unix_epoch_ms()?,
            generation: generate_attempt_generation(),
        };
        if let Some(connection) = self.redis.clone() {
            let value = encode_attempt(code_challenge, &attempt);
            let mut connection = connection;
            let result = tokio::time::timeout(
                REDIS_TIMEOUT,
                redis::cmd("SET")
                    .arg(&key)
                    .arg(value)
                    .arg("EX")
                    .arg(CODE_TTL_SECS)
                    .arg("NX")
                    .query_async::<Option<String>>(&mut connection),
            )
            .await
            .map_err(|_| DesktopHandoffStoreError)?
            .map_err(|_| DesktopHandoffStoreError)?;
            if result.is_some() {
                return Ok(Some(attempt));
            }
            let existing = tokio::time::timeout(
                REDIS_TIMEOUT,
                redis::cmd("GET")
                    .arg(&key)
                    .query_async::<Option<String>>(&mut connection),
            )
            .await
            .map_err(|_| DesktopHandoffStoreError)?
            .map_err(|_| DesktopHandoffStoreError)?;
            return Ok(existing
                .as_deref()
                .and_then(|value| parse_attempt(value, code_challenge)));
        }

        let mut attempts = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
        sweep_local_attempts(&mut attempts);
        let key = hash_code(state);
        if let Some(existing) = attempts.get(&key) {
            return Ok(
                (existing.code_challenge == code_challenge).then(|| DesktopGoogleAttempt {
                    started_at_ms: existing.started_at_ms,
                    generation: existing.generation.clone(),
                }),
            );
        }
        attempts.insert(
            key,
            LocalAttempt {
                code_challenge: code_challenge.to_string(),
                started_at_ms: attempt.started_at_ms,
                generation: attempt.generation.clone(),
                expires_at: Instant::now() + CODE_TTL,
            },
        );
        Ok(Some(attempt))
    }

    pub async fn get_google_attempt(
        &self,
        state: &str,
        code_challenge: &str,
    ) -> Result<Option<DesktopGoogleAttempt>, DesktopHandoffStoreError> {
        if let Some(connection) = self.redis.clone() {
            let mut connection = connection;
            let existing = tokio::time::timeout(
                REDIS_TIMEOUT,
                redis::cmd("GET")
                    .arg(attempt_redis_key(state))
                    .query_async::<Option<String>>(&mut connection),
            )
            .await
            .map_err(|_| DesktopHandoffStoreError)?
            .map_err(|_| DesktopHandoffStoreError)?;
            return Ok(existing
                .as_deref()
                .and_then(|value| parse_attempt(value, code_challenge)));
        }

        let mut attempts = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
        sweep_local_attempts(&mut attempts);
        Ok(attempts
            .get(&hash_code(state))
            .filter(|attempt| attempt.code_challenge == code_challenge)
            .map(|attempt| DesktopGoogleAttempt {
                started_at_ms: attempt.started_at_ms,
                generation: attempt.generation.clone(),
            }))
    }

    pub async fn consume_google_attempt(
        &self,
        state: &str,
        code_challenge: &str,
        expected: &DesktopGoogleAttempt,
    ) -> Result<bool, DesktopHandoffStoreError> {
        if let Some(connection) = self.redis.clone() {
            let mut connection = connection;
            let expected = encode_attempt(code_challenge, expected);
            let value = tokio::time::timeout(
                REDIS_TIMEOUT,
                redis::cmd("EVAL")
                    .arg(REDIS_CONSUME_ATTEMPT_SCRIPT)
                    .arg(1)
                    .arg(attempt_redis_key(state))
                    .arg(expected)
                    .query_async::<String>(&mut connection),
            )
            .await
            .map_err(|_| DesktopHandoffStoreError)?
            .map_err(|_| DesktopHandoffStoreError)?;
            if value.is_empty() || value == "__mismatch__" {
                return Ok(false);
            }
            return Ok(value == "1");
        }

        let mut attempts = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
        sweep_local_attempts(&mut attempts);
        let key = hash_code(state);
        let Some(attempt) = attempts.get(&key) else {
            return Ok(false);
        };
        if attempt.code_challenge != code_challenge
            || attempt.started_at_ms != expected.started_at_ms
            || attempt.generation != expected.generation
        {
            return Ok(false);
        }
        Ok(attempts.remove(&key).is_some())
    }
}

fn generate_code() -> String {
    let mut raw = [0u8; CODE_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    format!(
        "{CODE_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
    )
}

fn generate_attempt_generation() -> String {
    let mut raw = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

fn hash_code(code: &str) -> String {
    hex::encode(Sha256::digest(code.as_bytes()))
}

fn redis_key(code: &str) -> String {
    format!("{REDIS_PREFIX}{}", hash_code(code))
}

fn attempt_redis_key(state: &str) -> String {
    format!("{ATTEMPT_REDIS_PREFIX}{}", hash_code(state))
}

fn unix_epoch_ms() -> Result<i64, DesktopHandoffStoreError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DesktopHandoffStoreError)?
        .as_millis();
    i64::try_from(millis).map_err(|_| DesktopHandoffStoreError)
}

fn encode_attempt(code_challenge: &str, attempt: &DesktopGoogleAttempt) -> String {
    format!(
        "{code_challenge}|{}|{}",
        attempt.started_at_ms, attempt.generation
    )
}

fn parse_attempt(value: &str, code_challenge: &str) -> Option<DesktopGoogleAttempt> {
    let mut parts = value.split('|');
    let stored_challenge = parts.next()?;
    let started_at_ms = parts.next()?.parse::<i64>().ok()?;
    let generation = parts.next()?;
    if stored_challenge != code_challenge || generation.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(DesktopGoogleAttempt {
        started_at_ms,
        generation: generation.to_string(),
    })
}

fn sweep_local(issued: &mut HashMap<String, LocalGrant>) {
    let now = Instant::now();
    issued.retain(|_, grant| grant.expires_at > now);
}

fn sweep_local_attempts(attempts: &mut HashMap<String, LocalAttempt>) {
    let now = Instant::now();
    attempts.retain(|_, attempt| attempt.expires_at > now);
}

fn valid_code(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() <= 256
        && value
            .bytes()
            .skip(prefix.len())
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn valid_pkce_value(value: &str) -> bool {
    (43..=128).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || byte == b'-'
                || byte == b'_'
                || byte == b'.'
                || byte == b'~'
        })
}

#[derive(Debug, Deserialize)]
struct AttemptRequest {
    code_challenge: String,
    state: String,
}

#[derive(Debug, Deserialize)]
struct RedeemRequest {
    code: String,
    code_verifier: String,
}

pub fn google_router() -> Router<HandlerState> {
    Router::new()
        .route(ATTEMPT_PATH, post(register_google_attempt))
        .route(COMPLETE_PATH, post(complete_google_attempt))
}

pub fn redeem_router() -> Router<HandlerState> {
    Router::new().route("/api/desktop-handoff/redeem", post(redeem))
}

pub async fn rate_limit_desktop_google(
    State(state): State<DesktopHandoffIngressState>,
    mut request: Request,
    next: Next,
) -> Response {
    let has_broker_header = request.headers().contains_key(BROKER_AUTH_HEADER);
    if has_broker_header
        && (request.method() != axum::http::Method::POST
            || !matches!(request.uri().path(), ATTEMPT_PATH | COMPLETE_PATH))
    {
        request.headers_mut().remove(BROKER_AUTH_HEADER);
        request.extensions_mut().insert(RejectBrokerCredential);
        return patchbay_middleware::ratelimit::rate_limit(State(state.peer_limit), request, next)
            .await;
    }

    match take_broker_credential(request.headers_mut(), state.broker_secret.as_ref()) {
        BrokerCredential::Direct => {
            patchbay_middleware::ratelimit::rate_limit(State(state.peer_limit), request, next).await
        }
        BrokerCredential::Valid => next.run(request).await,
        BrokerCredential::Invalid => {
            request.extensions_mut().insert(RejectBrokerCredential);
            patchbay_middleware::ratelimit::rate_limit(State(state.peer_limit), request, next).await
        }
        BrokerCredential::Unconfigured => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth broker credential is not configured",
        ),
    }
}

pub async fn reject_invalid_broker_credential(mut request: Request, next: Next) -> Response {
    if request
        .extensions_mut()
        .remove::<RejectBrokerCredential>()
        .is_some()
    {
        return error_response(StatusCode::FORBIDDEN, "invalid auth broker credential");
    }
    next.run(request).await
}

pub async fn reject_broker_credential_on_redeem(mut request: Request, next: Next) -> Response {
    if request.headers_mut().remove(BROKER_AUTH_HEADER).is_some() {
        return error_response(StatusCode::FORBIDDEN, "invalid auth broker credential");
    }
    next.run(request).await
}

fn take_broker_credential(
    headers: &mut HeaderMap,
    expected_secret: Option<&[u8; BROKER_SECRET_BYTES]>,
) -> BrokerCredential {
    let secret_values = headers.get_all(BROKER_AUTH_HEADER);
    let secret_count = secret_values.iter().count();

    let result = if secret_count == 0 {
        BrokerCredential::Direct
    } else if expected_secret.is_none() {
        BrokerCredential::Unconfigured
    } else if secret_count != 1 {
        BrokerCredential::Invalid
    } else {
        let provided = secret_values
            .iter()
            .next()
            .and_then(|value| value.to_str().ok())
            .and_then(decode_broker_secret);
        match (provided, expected_secret) {
            (Some(provided), Some(expected)) if constant_time_eq(&provided, expected) => {
                BrokerCredential::Valid
            }
            _ => BrokerCredential::Invalid,
        }
    };

    headers.remove(BROKER_AUTH_HEADER);
    result
}

fn decode_broker_secret(value: &str) -> Option<[u8; BROKER_SECRET_BYTES]> {
    if value.len() != BROKER_SECRET_HEX_BYTES {
        return None;
    }
    let mut decoded = [0_u8; BROKER_SECRET_BYTES];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        decoded[index] = (decode_hex_nibble(pair[0])? << 4) | decode_hex_nibble(pair[1])?;
    }
    Some(decoded)
}

fn decode_hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
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

async fn register_google_attempt(
    State(state): State<HandlerState>,
    Json(request): Json<AttemptRequest>,
) -> Response {
    if !valid_pkce_value(&request.code_challenge) || !valid_pkce_value(&request.state) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid desktop Google OAuth binding",
        );
    }
    match state
        .desktop_handoff_tokens
        .register_google_attempt(&request.state, &request.code_challenge)
        .await
    {
        Ok(Some(_)) => Json(serde_json::json!({ "registered": true })).into_response(),
        Ok(None) => error_response(
            StatusCode::CONFLICT,
            "desktop Google OAuth binding is already in use",
        ),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "desktop Google OAuth is temporarily unavailable",
        ),
    }
}

async fn complete_google_attempt(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Json(request): Json<AttemptRequest>,
) -> Response {
    if !valid_pkce_value(&request.code_challenge) || !valid_pkce_value(&request.state) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid desktop Google OAuth binding",
        );
    }
    let Some(token) = bearer_token(&headers) else {
        return error_response(StatusCode::UNAUTHORIZED, "Clerk session is required");
    };
    let Some(verifier) = state.clerk_auth.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Clerk login is not configured",
        );
    };
    let attempt = match state
        .desktop_handoff_tokens
        .get_google_attempt(&request.state, &request.code_challenge)
        .await
    {
        Ok(Some(attempt)) => attempt,
        Ok(None) => {
            return error_response(StatusCode::CONFLICT, "fresh authentication is required")
        }
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "desktop Google OAuth is temporarily unavailable",
            )
        }
    };
    let identity = match verifier
        .verify_fresh_session(token, attempt.started_at_ms)
        .await
    {
        Ok(identity) => identity,
        Err(crate::clerk_auth::ClerkAuthError::Invalid) => {
            return error_response(StatusCode::CONFLICT, "fresh authentication is required")
        }
        Err(crate::clerk_auth::ClerkAuthError::Unavailable) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Clerk login is temporarily unavailable",
            )
        }
    };
    let current = match crate::auth::resolve_or_create_login_user(
        &state,
        &headers,
        &identity.email,
        Some(crate::auth::LoginProfile {
            name: identity.name,
            picture: identity.avatar_url.unwrap_or_default(),
            auth_method: "clerk",
        }),
    )
    .await
    {
        Ok(current) => current,
        Err(response) => return response,
    };
    match state
        .desktop_handoff_tokens
        .consume_google_attempt(&request.state, &request.code_challenge, &attempt)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return error_response(
                StatusCode::CONFLICT,
                "desktop Google OAuth attempt was already used",
            )
        }
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "desktop Google OAuth is temporarily unavailable",
            )
        }
    }
    match state
        .desktop_handoff_tokens
        .issue(current.id, &request.code_challenge)
        .await
    {
        Ok(code) => Json(serde_json::json!({ "code": code })).into_response(),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "desktop handoff is temporarily unavailable",
        ),
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty()).then(|| token.trim())
}

async fn redeem(State(state): State<HandlerState>, Json(request): Json<RedeemRequest>) -> Response {
    if !valid_code(&request.code, CODE_PREFIX) || !valid_pkce_value(&request.code_verifier) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid desktop handoff");
    }
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(request.code_verifier.as_bytes()));
    let user_id = match state
        .desktop_handoff_tokens
        .consume(&request.code, &challenge)
        .await
    {
        Ok(Some(user_id)) => user_id,
        Ok(None) => return error_response(StatusCode::UNAUTHORIZED, "invalid desktop handoff"),
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "desktop handoff is temporarily unavailable",
            )
        }
    };
    let current = match user::get_user(&state.pool, user_id).await {
        Ok(Some(user)) => user,
        Ok(None) | Err(_) => return error_response(StatusCode::UNAUTHORIZED, "user not found"),
    };
    if patchbay_auth::disabled_users::is_temporarily_disabled_user(
        &current.id.to_string(),
        &current.email,
    ) {
        return error_response(StatusCode::FORBIDDEN, "account disabled");
    }
    match patchbay_auth::jwt::issue_user_jwt(&current.id.to_string(), &current.email, &current.name)
    {
        Ok(token) => Json(serde_json::json!({ "token": token })).into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to generate token",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn handler_state() -> HandlerState {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        HandlerState::new(pool, patchbay_auth::pat_cache::PatCache::disabled(), None)
    }

    struct RejectFresh;

    #[async_trait]
    impl crate::clerk_auth::ClerkSessionVerifier for RejectFresh {
        async fn verify(
            &self,
            _token: &str,
        ) -> Result<crate::clerk_auth::ClerkIdentity, crate::clerk_auth::ClerkAuthError> {
            Err(crate::clerk_auth::ClerkAuthError::Invalid)
        }

        async fn verify_fresh_session(
            &self,
            _token: &str,
            _not_before_ms: i64,
        ) -> Result<crate::clerk_auth::ClerkIdentity, crate::clerk_auth::ClerkAuthError> {
            Err(crate::clerk_auth::ClerkAuthError::Invalid)
        }
    }

    #[tokio::test]
    async fn local_codes_are_bound_to_challenge_and_single_use() {
        let store = DesktopHandoffTokens::new();
        let user_id = Uuid::new_v4();
        let code = store.issue(user_id, "challenge").await.unwrap();

        assert_eq!(store.consume(&code, "wrong").await.unwrap(), None);
        assert_eq!(
            store.consume(&code, "challenge").await.unwrap(),
            Some(user_id)
        );
        assert_eq!(store.consume(&code, "challenge").await.unwrap(), None);
    }

    #[tokio::test]
    async fn local_google_attempts_preserve_start_time_and_are_single_use() {
        let store = DesktopHandoffTokens::new();
        let state = "s".repeat(43);
        let challenge = "c".repeat(43);
        let attempt = store
            .register_google_attempt(&state, &challenge)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            store
                .register_google_attempt(&state, &challenge)
                .await
                .unwrap(),
            Some(attempt.clone())
        );
        assert_eq!(
            store
                .register_google_attempt(&state, &"x".repeat(43))
                .await
                .unwrap(),
            None
        );
        assert!(store
            .consume_google_attempt(&state, &challenge, &attempt)
            .await
            .unwrap());
        assert!(!store
            .consume_google_attempt(&state, &challenge, &attempt)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn stale_completion_cannot_consume_a_re_registered_google_attempt() {
        let store = DesktopHandoffTokens::new();
        let state = "s".repeat(43);
        let challenge = "c".repeat(43);
        let attempt = store
            .register_google_attempt(&state, &challenge)
            .await
            .unwrap()
            .unwrap();
        let replacement = DesktopGoogleAttempt {
            // Even an ABA replacement registered in the same millisecond has
            // a distinct server-generated generation.
            started_at_ms: attempt.started_at_ms,
            generation: generate_attempt_generation(),
        };
        store
            .attempts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                hash_code(&state),
                LocalAttempt {
                    code_challenge: challenge.clone(),
                    started_at_ms: replacement.started_at_ms,
                    generation: replacement.generation.clone(),
                    expires_at: Instant::now() + CODE_TTL,
                },
            );

        assert!(!store
            .consume_google_attempt(&state, &challenge, &attempt)
            .await
            .unwrap());
        assert_eq!(
            store.get_google_attempt(&state, &challenge).await.unwrap(),
            Some(replacement.clone())
        );
        assert!(store
            .consume_google_attempt(&state, &challenge, &replacement)
            .await
            .unwrap());
    }

    #[test]
    fn validates_only_url_safe_handoff_values() {
        assert!(valid_code("pbd_abc-123", CODE_PREFIX));
        assert!(!valid_code("jwt-token", CODE_PREFIX));
        assert!(valid_pkce_value(&"a".repeat(43)));
        assert!(!valid_pkce_value("short"));
    }

    #[test]
    fn broker_credential_is_strict_single_value_and_removed_before_handler() {
        let secret = "a".repeat(BROKER_SECRET_HEX_BYTES);
        let expected = decode_broker_secret(&secret);
        let mut direct = HeaderMap::new();
        assert_eq!(
            take_broker_credential(&mut direct, expected.as_ref()),
            BrokerCredential::Direct
        );

        let mut trusted = HeaderMap::new();
        trusted.insert(BROKER_AUTH_HEADER, secret.parse().unwrap());
        assert_eq!(
            take_broker_credential(&mut trusted, expected.as_ref()),
            BrokerCredential::Valid
        );
        assert!(!trusted.contains_key(BROKER_AUTH_HEADER));

        let mut duplicate = HeaderMap::new();
        duplicate.append(BROKER_AUTH_HEADER, secret.parse().unwrap());
        duplicate.append(BROKER_AUTH_HEADER, secret.parse().unwrap());
        assert_eq!(
            take_broker_credential(&mut duplicate, expected.as_ref()),
            BrokerCredential::Invalid
        );

        let mut wrong = HeaderMap::new();
        wrong.insert(BROKER_AUTH_HEADER, "b".repeat(64).parse().unwrap());
        assert_eq!(
            take_broker_credential(&mut wrong, expected.as_ref()),
            BrokerCredential::Invalid
        );

        let mut unconfigured = HeaderMap::new();
        unconfigured.insert(BROKER_AUTH_HEADER, "b".repeat(64).parse().unwrap());
        assert_eq!(
            take_broker_credential(&mut unconfigured, None),
            BrokerCredential::Unconfigured
        );
    }

    #[tokio::test]
    async fn broker_assertion_applies_only_to_google_attempt_and_completion() {
        let state = handler_state();
        let limiter = patchbay_middleware::ratelimit::RateLimitState::disabled(20, 60);
        let secret = "a".repeat(BROKER_SECRET_HEX_BYTES);
        let app = google_router()
            .route_layer(axum::middleware::from_fn(reject_invalid_broker_credential))
            .route_layer(axum::middleware::from_fn_with_state(
                DesktopHandoffIngressState::new(limiter.clone(), &secret),
                rate_limit_desktop_google,
            ))
            .merge(
                redeem_router()
                    .route_layer(axum::middleware::from_fn(
                        reject_broker_credential_on_redeem,
                    ))
                    .route_layer(axum::middleware::from_fn_with_state(
                        limiter,
                        patchbay_middleware::ratelimit::rate_limit,
                    )),
            )
            .with_state(state);

        let invalid_assertion = app
            .clone()
            .oneshot(
                HttpRequest::post("/api/desktop-google/attempt")
                    .header(BROKER_AUTH_HEADER, "b".repeat(BROKER_SECRET_HEX_BYTES))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid_assertion.status(), StatusCode::FORBIDDEN);

        let redeem_ignores_assertion = app
            .clone()
            .oneshot(
                HttpRequest::post("/api/desktop-handoff/redeem")
                    .header(BROKER_AUTH_HEADER, secret)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"code":"bad","code_verifier":"bad"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(redeem_ignores_assertion.status(), StatusCode::FORBIDDEN);

        let wrong_method = app
            .oneshot(
                HttpRequest::get("/api/desktop-google/attempt")
                    .header(BROKER_AUTH_HEADER, "a".repeat(BROKER_SECRET_HEX_BYTES))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong_method.status(), StatusCode::FORBIDDEN);

        let unconfigured = google_router()
            .route_layer(axum::middleware::from_fn_with_state(
                DesktopHandoffIngressState::new(
                    patchbay_middleware::ratelimit::RateLimitState::disabled(20, 60),
                    "",
                ),
                rate_limit_desktop_google,
            ))
            .with_state(handler_state())
            .oneshot(
                HttpRequest::post("/api/desktop-google/attempt")
                    .header(BROKER_AUTH_HEADER, "a".repeat(BROKER_SECRET_HEX_BYTES))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unconfigured.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn completion_requires_registered_attempt_and_fresh_clerk_session() {
        let state_value = "s".repeat(43);
        let challenge = "c".repeat(43);
        let mut state = handler_state();
        state.clerk_auth = Some(Arc::new(RejectFresh));
        state
            .desktop_handoff_tokens
            .register_google_attempt(&state_value, &challenge)
            .await
            .unwrap();
        let app = google_router().with_state(state);

        let wrong_attempt = app
            .clone()
            .oneshot(
                HttpRequest::post("/api/desktop-google/complete")
                    .header(header::AUTHORIZATION, "Bearer clerk-session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "state": "x".repeat(43),
                            "code_challenge": challenge,
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong_attempt.status(), StatusCode::CONFLICT);

        let stale_session = app
            .oneshot(
                HttpRequest::post("/api/desktop-google/complete")
                    .header(header::AUTHORIZATION, "Bearer clerk-session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "state": state_value,
                            "code_challenge": challenge,
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale_session.status(), StatusCode::CONFLICT);
        let body = stale_session
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"],
            "fresh authentication is required"
        );
    }
}
