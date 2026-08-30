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

use axum::extract::{Json, State};
use axum::http::{header, HeaderMap, StatusCode};
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
local separator = string.find(value, "|", 1, true)
if not separator then return "" end
if string.sub(value, 1, separator - 1) ~= ARGV[1] then return "__mismatch__" end
redis.call("DEL", KEYS[1])
return string.sub(value, separator + 1)
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
struct LocalGrant {
    user_id: Uuid,
    code_challenge: String,
    expires_at: Instant,
}

#[derive(Clone)]
struct LocalAttempt {
    code_challenge: String,
    started_at_ms: i64,
    expires_at: Instant,
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
    ) -> Result<Option<i64>, DesktopHandoffStoreError> {
        let key = attempt_redis_key(state);
        let started_at_ms = unix_epoch_ms()?;
        if let Some(connection) = self.redis.clone() {
            let value = format!("{code_challenge}|{started_at_ms}");
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
                return Ok(Some(started_at_ms));
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
                (existing.code_challenge == code_challenge).then_some(existing.started_at_ms)
            );
        }
        attempts.insert(
            key,
            LocalAttempt {
                code_challenge: code_challenge.to_string(),
                started_at_ms,
                expires_at: Instant::now() + CODE_TTL,
            },
        );
        Ok(Some(started_at_ms))
    }

    pub async fn get_google_attempt(
        &self,
        state: &str,
        code_challenge: &str,
    ) -> Result<Option<i64>, DesktopHandoffStoreError> {
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
            .map(|attempt| attempt.started_at_ms))
    }

    pub async fn consume_google_attempt(
        &self,
        state: &str,
        code_challenge: &str,
    ) -> Result<Option<i64>, DesktopHandoffStoreError> {
        if let Some(connection) = self.redis.clone() {
            let mut connection = connection;
            let value = tokio::time::timeout(
                REDIS_TIMEOUT,
                redis::cmd("EVAL")
                    .arg(REDIS_CONSUME_ATTEMPT_SCRIPT)
                    .arg(1)
                    .arg(attempt_redis_key(state))
                    .arg(code_challenge)
                    .query_async::<String>(&mut connection),
            )
            .await
            .map_err(|_| DesktopHandoffStoreError)?
            .map_err(|_| DesktopHandoffStoreError)?;
            if value.is_empty() || value == "__mismatch__" {
                return Ok(None);
            }
            return Ok(value.parse::<i64>().ok());
        }

        let mut attempts = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
        sweep_local_attempts(&mut attempts);
        let key = hash_code(state);
        let Some(attempt) = attempts.get(&key) else {
            return Ok(None);
        };
        if attempt.code_challenge != code_challenge {
            return Ok(None);
        }
        Ok(attempts.remove(&key).map(|attempt| attempt.started_at_ms))
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

fn parse_attempt(value: &str, code_challenge: &str) -> Option<i64> {
    let (stored_challenge, started_at_ms) = value.split_once('|')?;
    (stored_challenge == code_challenge)
        .then(|| started_at_ms.parse::<i64>().ok())
        .flatten()
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

pub fn public_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/desktop-google/attempt", post(register_google_attempt))
        .route(
            "/api/desktop-google/complete",
            post(complete_google_attempt),
        )
        .route("/api/desktop-handoff/redeem", post(redeem))
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
    let started_at_ms = match state
        .desktop_handoff_tokens
        .get_google_attempt(&request.state, &request.code_challenge)
        .await
    {
        Ok(Some(started_at_ms)) => started_at_ms,
        Ok(None) => {
            return error_response(
                StatusCode::CONFLICT,
                "fresh Google authorization is required",
            )
        }
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "desktop Google OAuth is temporarily unavailable",
            )
        }
    };
    let identity = match verifier
        .verify_fresh_google_session(token, started_at_ms)
        .await
    {
        Ok(identity) => identity,
        Err(crate::clerk_auth::ClerkAuthError::Invalid) => {
            return error_response(
                StatusCode::CONFLICT,
                "fresh Google authorization is required",
            )
        }
        Err(crate::clerk_auth::ClerkAuthError::Unavailable) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Clerk login is temporarily unavailable",
            )
        }
    };
    let current = match user::get_user_by_email(&state.pool, &identity.email).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            return error_response(
                StatusCode::CONFLICT,
                "Patchbay session exchange is incomplete",
            )
        }
        Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to resolve user")
        }
    };
    if patchbay_auth::disabled_users::is_temporarily_disabled_user(
        &current.id.to_string(),
        &current.email,
    ) {
        return error_response(StatusCode::FORBIDDEN, "account disabled");
    }
    match state
        .desktop_handoff_tokens
        .consume_google_attempt(&request.state, &request.code_challenge)
        .await
    {
        Ok(Some(consumed_at_ms)) if consumed_at_ms == started_at_ms => {}
        Ok(Some(_)) | Ok(None) => {
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
        let started_at = store
            .register_google_attempt(&state, &challenge)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            store
                .register_google_attempt(&state, &challenge)
                .await
                .unwrap(),
            Some(started_at)
        );
        assert_eq!(
            store
                .register_google_attempt(&state, &"x".repeat(43))
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .consume_google_attempt(&state, &challenge)
                .await
                .unwrap(),
            Some(started_at)
        );
        assert_eq!(
            store
                .consume_google_attempt(&state, &challenge)
                .await
                .unwrap(),
            None
        );
    }

    #[test]
    fn validates_only_url_safe_handoff_values() {
        assert!(valid_code("pbd_abc-123", CODE_PREFIX));
        assert!(!valid_code("jwt-token", CODE_PREFIX));
        assert!(valid_pkce_value(&"a".repeat(43)));
        assert!(!valid_pkce_value("short"));
    }
}
