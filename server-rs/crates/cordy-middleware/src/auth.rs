//! Auth middleware — port of `server/internal/middleware/auth.go`.
//!
//! Validates JWT tokens or Personal Access Tokens. Token sources (in
//! priority order):
//!  1. `Authorization: Bearer <token>` header (PAT or JWT)
//!  2. `cordy_auth` HttpOnly cookie (JWT) — requires a valid CSRF token for
//!     state-changing requests
//!
//! Identity is injected as request headers for downstream handlers,
//! mirroring the Go contract exactly:
//! - `X-User-ID` (all paths), `X-User-Email` (JWT only)
//! - `X-Agent-ID` / `X-Task-ID` / `X-Workspace-ID` (mat_ task tokens)
//! - `X-Actor-Source` — server-set only; any client-supplied value is
//!   stripped before the auth branches run.

use std::collections::HashSet;

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use cordy_auth::cookie::{verify_csrf_signature, AUTH_COOKIE_NAME};
use cordy_auth::disabled_users::{is_temporarily_disabled_user, TEMPORARILY_DISABLED_USER_ERROR};
use cordy_auth::jwt::{hash_token, jwt_secret};
use cordy_auth::pat_cache::{ttl_for_expiry, PatCache};
use cordy_db::queries::user;
use cordy_db::queries::{personal_access_token, task_token};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone)]
pub struct AuthState {
    pub pool: sqlx::PgPool,
    pub pat_cache: PatCache,
    pub cloud_pat_verifier: Option<cordy_auth::cloud_pat::CloudPatVerifier>,
}

pub(crate) enum CloudAuthError {
    Invalid,
    Unavailable,
}

pub(crate) async fn verify_cloud_pat(
    pool: &sqlx::PgPool,
    verifier: Option<&cordy_auth::cloud_pat::CloudPatVerifier>,
    token: &str,
) -> Result<String, CloudAuthError> {
    let Some(verifier) = verifier else {
        return Err(CloudAuthError::Invalid);
    };
    let cancel = CancellationToken::new();
    let verified = verifier
        .verify(token, &cancel)
        .await
        .map_err(|error| match error {
            cordy_auth::cloud_pat::CloudPatError::Invalid => CloudAuthError::Invalid,
            cordy_auth::cloud_pat::CloudPatError::Unavailable => CloudAuthError::Unavailable,
        })?;
    if !verified.owner_already_validated {
        let owner_id =
            Uuid::parse_str(&verified.identity.owner_id).map_err(|_| CloudAuthError::Invalid)?;
        match user::get_user(pool, owner_id).await {
            Ok(Some(_)) => {
                verifier
                    .cache_validated(token, &verified.identity, &cancel)
                    .await
            }
            Ok(None) => return Err(CloudAuthError::Invalid),
            Err(error) => {
                tracing::warn!(%error, "cloud PAT owner lookup failed");
                return Err(CloudAuthError::Unavailable);
            }
        }
    }
    Ok(verified.identity.owner_id)
}

fn err_response(status: StatusCode, msg: &'static str) -> (StatusCode, &'static str) {
    // Body shape matches Go's writeError/http.Error JSON payloads.
    let body = match msg {
        "account disabled" => r#"{"error":"account disabled"}"#,
        other => return (status, other),
    };
    (status, body)
}

pub(crate) fn reject_disabled(user_id: &str, email: &str, auth_path: &str) -> bool {
    if is_temporarily_disabled_user(user_id, email) {
        tracing::warn!(
            user_id = %user_id,
            auth_path = %auth_path,
            "auth: temporarily disabled user rejected"
        );
        true
    } else {
        false
    }
}

/// Returns the bearer token and whether it came from a cookie.
/// Priority: Authorization header > cordy_auth cookie. An Authorization
/// header WITHOUT the Bearer prefix falls through to the cookie, matching
/// Go's TrimPrefix identity check.
fn extract_token(req: &Request) -> Option<(String, bool)> {
    let headers = req.headers();
    if let Some(auth_header) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if !auth_header.is_empty() {
            if let Some(token) = auth_header.strip_prefix("Bearer ") {
                return Some((token.to_string(), false));
            }
        }
    }
    let cookie_header = headers.get(header::COOKIE).and_then(|v| v.to_str().ok())?;
    let value = cookie_value(cookie_header, AUTH_COOKIE_NAME)?;
    if value.is_empty() {
        return None;
    }
    Some((value.to_string(), true))
}

fn cookie_value<'a>(cookies: &'a str, name: &str) -> Option<&'a str> {
    for pair in cookies.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            if k == name && !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn set_header(req: &mut Request, name: &'static str, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        req.headers_mut().insert(name, v);
    }
}

/// Auth middleware entrypoint — use via
/// `axum::middleware::from_fn_with_state(state, auth_middleware)`.
pub async fn auth_middleware(
    State(state): State<AuthState>,
    mut req: Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    // X-Actor-Source is server-set only — any client-supplied value is
    // untrusted and discarded before the auth branches run. Only the mat_
    // branch below re-sets it. This prevents a client from sending a normal
    // mul_ PAT plus a forged `X-Actor-Source: member` to convince downstream
    // handlers that its request came from a non-task-token path.
    req.headers_mut().remove("x-actor-source");

    // When the Next.js / Clerk frontend has already authenticated the request
    // and forwarded it with X-User-ID set, trust it directly — no JWT/PAT
    // verification needed.
    if req
        .headers()
        .get("x-user-id")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !v.is_empty())
    {
        return Ok(next.run(req).await);
    }

    let Some((token, from_cookie)) = extract_token(&req) else {
        tracing::debug!(path = ?req.uri().path(), "auth: no token found");
        return Err((
            StatusCode::UNAUTHORIZED,
            r#"{"error":"missing authorization"}"#,
        ));
    };

    // Cookie-based auth requires CSRF validation for state-changing methods.
    if from_cookie && !safe_method(req.method()) && !csrf_ok(&req, &token) {
        tracing::debug!(path = ?req.uri().path(), "auth: CSRF validation failed");
        return Err((
            StatusCode::FORBIDDEN,
            r#"{"error":"CSRF validation failed"}"#,
        ));
    }

    let hash = hash_token(&token);

    // Agent task token: "mat_" prefix. Minted by the server at task-claim
    // time and injected by the daemon into the agent process. Authoritative
    // for actor identity — the bound ids are written into request headers
    // here, OVERRIDING whatever the client sent (MUL-2600).
    if token.starts_with("mat_") {
        let Some(tt) = task_token::get_task_token_by_hash(&state.pool, &hash)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "auth: task token lookup failed");
                None
            })
        else {
            tracing::warn!(path = ?req.uri().path(), "auth: invalid task token");
            return Err((StatusCode::UNAUTHORIZED, r#"{"error":"invalid token"}"#));
        };

        let user_id = tt.user_id.to_string();
        if reject_disabled(&user_id, "", "task_token") {
            return Err(err_response(
                StatusCode::FORBIDDEN,
                TEMPORARILY_DISABLED_USER_ERROR,
            ));
        }
        set_header(&mut req, "x-user-id", &user_id);
        set_header(&mut req, "x-agent-id", &tt.agent_id.to_string());
        set_header(&mut req, "x-task-id", &tt.task_id.to_string());
        set_header(&mut req, "x-workspace-id", &tt.workspace_id.to_string());
        // The only value this header may carry — strip anything else a
        // client tried to send (done above).
        set_header(&mut req, "x-actor-source", "task_token");
        return Ok(next.run(req).await);
    }

    // Cloud Node PAT: "mcn_" prefix. Verified by the Cordy Cloud Fleet
    // service — never against the local personal_access_tokens table. When
    // the verifier is unconfigured we reject at this branch rather than
    // treating the token as a JWT/PAT — failing closed avoids a
    // misconfigured prod silently downgrading auth.
    if token.starts_with(cordy_auth::cloud_pat::CLOUD_PAT_PREFIX) {
        let owner_id = match verify_cloud_pat(
            &state.pool,
            state.cloud_pat_verifier.as_ref(),
            &token,
        )
        .await
        {
            Ok(owner_id) => owner_id,
            Err(CloudAuthError::Invalid) => {
                tracing::warn!(path = ?req.uri().path(), "auth: cloud PAT rejected");
                return Err((StatusCode::UNAUTHORIZED, r#"{"error":"invalid token"}"#));
            }
            Err(CloudAuthError::Unavailable) => {
                tracing::warn!(path = ?req.uri().path(), "auth: cloud PAT verifier unavailable");
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    r#"{"error":"cloud pat verifier unavailable"}"#,
                ));
            }
        };
        if reject_disabled(&owner_id, "", "cloud_pat") {
            return Err(err_response(
                StatusCode::FORBIDDEN,
                TEMPORARILY_DISABLED_USER_ERROR,
            ));
        }
        set_header(&mut req, "x-user-id", &owner_id);
        set_header(&mut req, "x-actor-source", "cloud_pat");
        return Ok(next.run(req).await);
    }

    // PAT: tokens starting with "mul_".
    if token.starts_with("mul_") {
        // Cache hit: skip both the DB SELECT and the last_used_at UPDATE —
        // last_used_at is bumped at most once per TTL window per token.
        if let Some(user_id) = state.pat_cache.get(&hash).await {
            if reject_disabled(&user_id, "", "pat_cache") {
                return Err(err_response(
                    StatusCode::FORBIDDEN,
                    TEMPORARILY_DISABLED_USER_ERROR,
                ));
            }
            set_header(&mut req, "x-user-id", &user_id);
            return Ok(next.run(req).await);
        }

        let Some(pat) =
            personal_access_token::get_personal_access_token_by_hash(&state.pool, &hash)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "auth: PAT lookup failed");
                    None
                })
        else {
            tracing::warn!(path = ?req.uri().path(), "auth: invalid PAT");
            return Err((StatusCode::UNAUTHORIZED, r#"{"error":"invalid token"}"#));
        };

        let user_id = pat.user_id.to_string();
        if reject_disabled(&user_id, "", "pat") {
            return Err(err_response(
                StatusCode::FORBIDDEN,
                TEMPORARILY_DISABLED_USER_ERROR,
            ));
        }
        set_header(&mut req, "x-user-id", &user_id);

        // Clamp cache TTL to the token's remaining lifetime so a PAT expiring
        // in <AuthCacheTTL can't continue passing auth on a cache hit after
        // expires_at.
        let ttl = ttl_for_expiry(chrono::Utc::now(), pat.expires_at);
        state.pat_cache.set(&hash, &user_id, ttl).await;

        // Cache miss = first request in this TTL window. Refresh
        // last_used_at asynchronously; subsequent hits skip the write.
        let pool = state.pool.clone();
        let pat_id = pat.id;
        tokio::spawn(async move {
            if let Err(e) =
                personal_access_token::update_personal_access_token_last_used(&pool, pat_id).await
            {
                tracing::warn!(error = %e, "auth: failed to refresh PAT last_used_at");
            }
        });

        return Ok(next.run(req).await);
    }

    // JWT (HS256). Matches golang-jwt v5 Parse semantics: exp validated when
    // present but not required, no leeway, aud unchecked.
    let claims = decode_jwt_claims(&token);
    let Some(claims) = claims else {
        tracing::warn!(path = ?req.uri().path(), "auth: invalid token");
        return Err((StatusCode::UNAUTHORIZED, r#"{"error":"invalid token"}"#));
    };

    let sub = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(sub) = sub else {
        tracing::warn!(path = ?req.uri().path(), "auth: invalid claims");
        return Err((StatusCode::UNAUTHORIZED, r#"{"error":"invalid claims"}"#));
    };
    let email = claims.get("email").and_then(|v| v.as_str()).unwrap_or("");

    if reject_disabled(sub, email, "jwt") {
        return Err(err_response(
            StatusCode::FORBIDDEN,
            TEMPORARILY_DISABLED_USER_ERROR,
        ));
    }
    set_header(&mut req, "x-user-id", sub);
    if !email.is_empty() {
        set_header(&mut req, "x-user-email", email);
    }

    Ok(next.run(req).await)
}

fn safe_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// CSRF gate for cookie-sourced requests: checks the X-CSRF-Token header
/// against the auth cookie via HMAC signature (see cordy_auth::cookie).
fn csrf_ok(req: &Request, auth_token: &str) -> bool {
    let csrf_header = req
        .headers()
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if csrf_header.is_empty() {
        return false;
    }
    verify_csrf_signature(auth_token, csrf_header)
}

/// HS256-only decode with Go-compatible claim requirements.
pub fn decode_jwt_claims(token: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};

    let mut validation = Validation::new(Algorithm::HS256);
    // golang-jwt v5 validates exp when present but does not require it;
    // jsonwebtoken defaults to requiring ["exp"].
    validation.required_spec_claims = HashSet::new();
    // Go applies no clock skew by default.
    validation.leeway = 0;

    let data = decode::<serde_json::Value>(
        token,
        &DecodingKey::from_secret(jwt_secret().as_bytes()),
        &validation,
    )
    .ok()?;
    data.claims.as_object().cloned()
}
