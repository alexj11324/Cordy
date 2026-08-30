//! Auth middleware.
//!
//! Validates JWT tokens or Personal Access Tokens. Token sources (in
//! priority order):
//!  1. `Authorization: Bearer <token>` header (PAT or JWT)
//!  2. `patchbay_auth` HttpOnly cookie (JWT) — requires a valid CSRF token for
//!     state-changing requests
//!
//! Identity is injected as request headers for downstream handlers,
//! mirroring the Go contract exactly:
//! - `X-User-ID` (all paths), `X-User-Email` (JWT only)
//! - `X-Agent-ID` / `X-Task-ID` / `X-Workspace-ID` (mat_ task tokens)
//! - `X-Actor-Source` / `X-Agent-ID` / `X-Task-ID` — server-set only; any
//!   client-supplied values are stripped before the auth branches run.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use ipnetwork::IpNetwork;
use patchbay_auth::cookie::{verify_csrf_signature, AUTH_COOKIE_NAME, LEGACY_AUTH_COOKIE_NAME};
use patchbay_auth::disabled_users::{
    is_temporarily_disabled_user, TEMPORARILY_DISABLED_USER_ERROR,
};
use patchbay_auth::jwt::{hash_token, jwt_secret};
use patchbay_auth::pat_cache::{ttl_for_expiry, PatCache};
use patchbay_db::queries::user;
use patchbay_db::queries::{guest, personal_access_token, task_token};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const IDENTITY_PROXY_MARKER_HEADER: &str = "x-patchbay-identity-proxy-token";
const LEGACY_IDENTITY_PROXY_MARKER_HEADER: &str = "x-cordy-identity-proxy-token"; // legacy-brand-compat
const IDENTITY_PROXY_MIN_SECRET_BYTES: usize = 32;

/// Explicit trust boundary for an upstream that authenticates a user and
/// stamps `X-User-ID`. Both the direct peer and a private marker must match:
/// a CIDR alone is insufficient because reverse proxies commonly forward
/// client-supplied headers unchanged.
#[derive(Clone, Default)]
pub struct IdentityProxyTrust {
    trusted_peers: Arc<[IpNetwork]>,
    marker: Option<Arc<[u8]>>,
}

#[derive(Debug, Eq, PartialEq)]
struct ProxyIdentity {
    user_id: String,
    email: String,
}

impl IdentityProxyTrust {
    pub fn from_env() -> Self {
        let cidrs = std::env::var("PATCHBAY_IDENTITY_TRUSTED_PROXIES").unwrap_or_default();
        let marker = std::env::var("PATCHBAY_IDENTITY_PROXY_SECRET").unwrap_or_default();
        match Self::configured(&cidrs, &marker) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "identity proxy trust disabled");
                Self::default()
            }
        }
    }

    fn configured(cidrs: &str, marker: &str) -> Result<Self, &'static str> {
        if cidrs.trim().is_empty() && marker.is_empty() {
            return Ok(Self::default());
        }
        if cidrs.trim().is_empty() || marker.len() < IDENTITY_PROXY_MIN_SECRET_BYTES {
            return Err(
                "PATCHBAY_IDENTITY_TRUSTED_PROXIES and a 32-byte PATCHBAY_IDENTITY_PROXY_SECRET are both required",
            );
        }
        let trusted_peers = cidrs
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .parse::<IpNetwork>()
                    .map_err(|_| "PATCHBAY_IDENTITY_TRUSTED_PROXIES contains an invalid CIDR")
            })
            .collect::<Result<Vec<_>, _>>()?;
        if trusted_peers.is_empty() {
            return Err("PATCHBAY_IDENTITY_TRUSTED_PROXIES must contain at least one CIDR");
        }
        Ok(Self {
            trusted_peers: trusted_peers.into(),
            marker: Some(Arc::from(marker.as_bytes())),
        })
    }

    fn take_identity(&self, req: &mut Request) -> Option<ProxyIdentity> {
        let trusted_peer = req
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|info| info.0.ip())
            .is_some_and(|ip| {
                self.trusted_peers
                    .iter()
                    .any(|network| network.contains(ip))
            });
        let marker_matches = self.marker.as_deref().is_some_and(|expected| {
            req.headers()
                .get(IDENTITY_PROXY_MARKER_HEADER)
                .or_else(|| req.headers().get(LEGACY_IDENTITY_PROXY_MARKER_HEADER))
                .is_some_and(|actual| constant_time_eq(actual.as_bytes(), expected))
        });
        let identity = (trusted_peer && marker_matches)
            .then(|| ProxyIdentity {
                user_id: header_string(req, "x-user-id"),
                email: header_string(req, "x-user-email"),
            })
            .filter(|identity| !identity.user_id.is_empty());

        // These headers are owned by authentication. Always remove the
        // request copies, then re-stamp only a verified proxy identity below.
        req.headers_mut().remove("x-user-id");
        req.headers_mut().remove("x-user-email");
        req.headers_mut().remove(IDENTITY_PROXY_MARKER_HEADER);
        req.headers_mut()
            .remove(LEGACY_IDENTITY_PROXY_MARKER_HEADER);
        if let Some(identity) = identity.as_ref() {
            set_header(req, "x-user-id", &identity.user_id);
            if !identity.email.is_empty() {
                set_header(req, "x-user-email", &identity.email);
            }
        }
        identity
    }
}

fn header_string(req: &Request, name: &'static str) -> String {
    req.headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .trim()
        .to_string()
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

#[derive(Clone)]
pub struct AuthState {
    pub pool: sqlx::PgPool,
    pub pat_cache: PatCache,
    pub cloud_pat_verifier: Option<patchbay_auth::cloud_pat::CloudPatVerifier>,
    pub side_effects: Arc<dyn AuthSideEffectSpawner>,
    pub identity_proxy: IdentityProxyTrust,
}

pub(crate) enum CloudAuthError {
    Invalid,
    Unavailable,
}

pub(crate) async fn verify_cloud_pat(
    pool: &sqlx::PgPool,
    verifier: Option<&patchbay_auth::cloud_pat::CloudPatVerifier>,
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
            patchbay_auth::cloud_pat::CloudPatError::Invalid => CloudAuthError::Invalid,
            patchbay_auth::cloud_pat::CloudPatError::Unavailable => CloudAuthError::Unavailable,
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

pub type AuthSideEffect = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// Narrow ownership seam supplied by the production server. Keeping it in
/// middleware avoids a service-layer dependency while ensuring auth writes
/// join the same bounded shutdown drain as other request side effects.
pub trait AuthSideEffectSpawner: Send + Sync {
    fn spawn(&self, task: AuthSideEffect);
}

impl<F> AuthSideEffectSpawner for F
where
    F: Fn(AuthSideEffect) + Send + Sync,
{
    fn spawn(&self, task: AuthSideEffect) {
        self(task);
    }
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
/// Priority: Authorization header > patchbay_auth cookie. An Authorization
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
    let value = cookie_value(cookie_header, AUTH_COOKIE_NAME)
        .or_else(|| cookie_value(cookie_header, LEGACY_AUTH_COOKIE_NAME))?;
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

fn clear_untrusted_task_identity(req: &mut Request) {
    for name in [
        "x-actor-source",
        "x-agent-id",
        "x-task-id",
        "x-capability-lease-id",
        "x-on-behalf-of-user-id",
        "x-device-id",
        "x-guest-user",
    ] {
        req.headers_mut().remove(name);
    }
}

fn stamp_task_identity(
    req: &mut Request,
    user_id: Uuid,
    agent_id: Uuid,
    task_id: Uuid,
    workspace_id: Uuid,
    lease_id: Uuid,
    on_behalf_of_user_id: Option<Uuid>,
    device_id: Option<Uuid>,
) {
    set_header(req, "x-user-id", &user_id.to_string());
    set_header(req, "x-agent-id", &agent_id.to_string());
    set_header(req, "x-task-id", &task_id.to_string());
    set_header(req, "x-workspace-id", &workspace_id.to_string());
    set_header(req, "x-capability-lease-id", &lease_id.to_string());
    if let Some(on_behalf_of_user_id) = on_behalf_of_user_id {
        set_header(
            req,
            "x-on-behalf-of-user-id",
            &on_behalf_of_user_id.to_string(),
        );
    }
    if let Some(device_id) = device_id {
        set_header(req, "x-device-id", &device_id.to_string());
    }
    set_header(req, "x-actor-source", "task_token");
}

/// Phase 1 task leases are accepted only by data-plane routes whose handlers
/// either bind mutations to the current task or consume the shared
/// authorizer. This default-deny boundary prevents the compatibility
/// `x-user-id` projection from turning a short-lived task lease into a JWT,
/// PAT, credential-management session, Agent secret read, or another durable
/// human control-plane authority.
fn task_token_route_allowed(method: &Method, path: &str, _workspace_id: Uuid) -> bool {
    if path.starts_with("/api/issues") {
        if method == Method::GET {
            let suffix = path.strip_prefix("/api/issues/").unwrap_or_default();
            return !suffix.is_empty() && !suffix.contains('/');
        }
        if method == Method::POST {
            return path == "/api/issues" || path.ends_with("/comments");
        }
        if method == Method::PUT {
            let suffix = path.strip_prefix("/api/issues/").unwrap_or_default();
            return !suffix.is_empty() && !suffix.contains('/');
        }
        return false;
    }
    if path.starts_with("/api/comments") {
        return false;
    }
    if path.starts_with("/api/tasks/") {
        return (method == Method::GET && path.ends_with("/messages"))
            || (method == Method::POST
                && (path.ends_with("/message-bus") || path.ends_with("/cancel")));
    }
    if method == Method::GET
        && ["/api/properties", "/api/labels"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
    {
        return true;
    }
    false
}

/// Auth middleware entrypoint — use via
/// `axum::middleware::from_fn_with_state(state, auth_middleware)`.
pub async fn auth_middleware(
    State(state): State<AuthState>,
    mut req: Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    // Task identity is server-owned as one atomic tuple. Strip every
    // client-supplied component before choosing an auth branch; only a
    // validated mat_ token may restore it below. Keeping X-Agent-ID or
    // X-Task-ID from a JWT/PAT/proxy request would let a downstream handler
    // reconstruct an agent actor from attacker-controlled headers.
    clear_untrusted_task_identity(&mut req);

    // A managed identity proxy may authenticate upstream, but only an
    // explicitly configured peer carrying the private marker can cross this
    // boundary. Every other request has its identity headers stripped before
    // the ordinary JWT/PAT/Cloud PAT branches inspect it.
    if let Some(identity) = state.identity_proxy.take_identity(&mut req) {
        if reject_disabled(&identity.user_id, &identity.email, "identity_proxy") {
            return Err(err_response(
                StatusCode::FORBIDDEN,
                TEMPORARILY_DISABLED_USER_ERROR,
            ));
        }
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

    // Guest tokens are opaque, server-backed bearer tokens. They are checked
    // against both the session table and the real user row instead of being
    // accepted as a self-describing/local-only identity.
    if token.starts_with("pbg_") {
        let session = match guest::find_active_by_token_hash(&state.pool, &hash).await {
            Ok(Some(session)) => session,
            Ok(None) => {
                tracing::warn!(path = ?req.uri().path(), "auth: invalid guest token");
                return Err((StatusCode::UNAUTHORIZED, r#"{"error":"invalid token"}"#));
            }
            Err(error) => {
                tracing::error!(%error, "auth: guest session lookup unavailable");
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    r#"{"error":"authentication temporarily unavailable"}"#,
                ));
            }
        };
        let guest_user = match user::get_user(&state.pool, session.user_id).await {
            Ok(Some(guest_user)) => guest_user,
            Ok(None) => {
                tracing::warn!(path = ?req.uri().path(), "auth: guest user not found");
                return Err((StatusCode::UNAUTHORIZED, r#"{"error":"invalid token"}"#));
            }
            Err(error) => {
                tracing::error!(%error, "auth: guest user lookup unavailable");
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    r#"{"error":"authentication temporarily unavailable"}"#,
                ));
            }
        };
        if !guest_user.is_guest {
            tracing::warn!(path = ?req.uri().path(), "auth: guest session points to formal user");
            return Err((StatusCode::UNAUTHORIZED, r#"{"error":"invalid token"}"#));
        }
        if reject_disabled(&guest_user.id.to_string(), &guest_user.email, "guest") {
            return Err(err_response(
                StatusCode::FORBIDDEN,
                TEMPORARILY_DISABLED_USER_ERROR,
            ));
        }
        set_header(&mut req, "x-user-id", &guest_user.id.to_string());
        set_header(&mut req, "x-guest-user", "true");
        return Ok(next.run(req).await);
    }

    // Agent task token: "mat_" prefix. Minted by the server at task-claim
    // time and injected by the daemon into the agent process. Authoritative
    // for actor identity — the bound ids are written into request headers
    // here, OVERRIDING whatever the client sent (PB-2600).
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
        if !task_token_route_allowed(req.method(), req.uri().path(), tt.workspace_id) {
            tracing::warn!(
                task_id = %tt.task_id,
                path = %req.uri().path(),
                method = %req.method(),
                "auth: capability lease rejected at human control-plane boundary"
            );
            return Err((
                StatusCode::FORBIDDEN,
                r#"{"error":"task capability is not valid for this route"}"#,
            ));
        }
        stamp_task_identity(
            &mut req,
            tt.user_id,
            tt.agent_id,
            tt.task_id,
            tt.workspace_id,
            tt.id,
            tt.on_behalf_of_user_id,
            tt.device_id,
        );
        return Ok(next.run(req).await);
    }

    // Cloud Node PAT: "mcn_" prefix. Verified by the Patchbay Cloud Fleet
    // service — never against the local personal_access_tokens table. When
    // the verifier is unconfigured we reject at this branch rather than
    // treating the token as a JWT/PAT — failing closed avoids a
    // misconfigured prod silently downgrading auth.
    if token.starts_with(patchbay_auth::cloud_pat::CLOUD_PAT_PREFIX) {
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

    // PAT: tokens starting with "pby_".
    if token.starts_with("pby_") {
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
        state.side_effects.spawn(Box::pin(async move {
            if let Err(e) =
                personal_access_token::update_personal_access_token_last_used(&pool, pat_id).await
            {
                tracing::warn!(error = %e, "auth: failed to refresh PAT last_used_at");
            }
        }));

        return Ok(next.run(req).await);
    }

    // JWT (HS256): exp is validated when present but is not required, with no
    // leeway and no audience requirement.
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

/// Blocks operations that require a formal account while leaving the normal
/// authenticated router available to real guest users. The user row is
/// checked here rather than trusting a client-provided marker; auth_middleware
/// strips that marker before it stamps the server-owned identity.
pub async fn require_formal_user(
    State(pool): State<sqlx::PgPool>,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    let Some(raw_user_id) = req.headers().get("x-user-id") else {
        return Err((
            StatusCode::UNAUTHORIZED,
            r#"{"error":"user not authenticated","code":"login_required"}"#,
        ));
    };
    let Ok(user_id) = raw_user_id
        .to_str()
        .ok()
        .and_then(|value| Uuid::parse_str(value.trim()).ok())
        .ok_or(())
    else {
        return Err((
            StatusCode::UNAUTHORIZED,
            r#"{"error":"user not authenticated","code":"login_required"}"#,
        ));
    };
    let user = user::get_user(&pool, user_id).await.map_err(|error| {
        tracing::warn!(%error, %user_id, "formal-account guard user lookup failed");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":"account status unavailable"}"#,
        )
    })?;
    let Some(user) = user else {
        return Err((
            StatusCode::UNAUTHORIZED,
            r#"{"error":"user not found","code":"login_required"}"#,
        ));
    };
    if user.is_guest {
        return Err((
            StatusCode::FORBIDDEN,
            r#"{"error":"formal login required","code":"login_required"}"#,
        ));
    }
    Ok(next.run(req).await)
}

fn safe_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// CSRF gate for cookie-sourced requests: checks the X-CSRF-Token header
/// against the auth cookie via HMAC signature (see patchbay_auth::cookie).
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

/// HS256-only decode with the service's claim requirements.
pub fn decode_jwt_claims(token: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};

    let mut validation = Validation::new(Algorithm::HS256);
    // Validate exp when present without requiring it; jsonwebtoken defaults
    // to requiring ["exp"].
    validation.required_spec_claims = HashSet::new();
    // The authentication contract applies no clock skew.
    validation.leeway = 0;

    let data = decode::<serde_json::Value>(
        token,
        &DecodingKey::from_secret(jwt_secret().as_bytes()),
        &validation,
    )
    .ok()?;
    data.claims.as_object().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use std::net::SocketAddr;

    const MARKER: &str = "0123456789abcdef0123456789abcdef";
    const VICTIM: &str = "018f03a0-c4d2-7a37-ae4d-5aa45de12f11";

    fn policy() -> IdentityProxyTrust {
        IdentityProxyTrust::configured("10.0.0.0/8", MARKER).expect("valid policy")
    }

    fn request(peer: Option<&str>, bearer: &str, marker: &str) -> Request<Body> {
        let mut request = Request::builder()
            .header("x-user-id", VICTIM)
            .header("x-user-email", "victim@example.com")
            .header(IDENTITY_PROXY_MARKER_HEADER, marker)
            .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
            .body(Body::empty())
            .expect("request");
        if let Some(peer) = peer {
            request.extensions_mut().insert(ConnectInfo(
                peer.parse::<SocketAddr>().expect("socket address"),
            ));
        }
        request
    }

    fn test_uuid(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    #[test]
    fn spoofed_identity_cannot_bypass_jwt_pat_or_cloud_pat_paths() {
        for bearer in ["header.payload.signature", "pby_secret", "mcn_secret"] {
            let mut request = request(Some("203.0.113.9:443"), bearer, MARKER);
            let expected_authorization = format!("Bearer {bearer}");
            assert_eq!(policy().take_identity(&mut request), None, "{bearer}");
            assert!(request.headers().get("x-user-id").is_none(), "{bearer}");
            assert!(request.headers().get("x-user-email").is_none(), "{bearer}");
            assert!(
                request
                    .headers()
                    .get(IDENTITY_PROXY_MARKER_HEADER)
                    .is_none(),
                "{bearer}"
            );
            assert_eq!(
                request
                    .headers()
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok()),
                Some(expected_authorization.as_str()),
                "the real authentication branch must still receive its credential"
            );
        }
    }

    #[test]
    fn trusted_peer_and_private_marker_preserve_managed_identity() {
        let mut request = request(Some("10.2.3.4:8443"), "unused", MARKER);
        request
            .headers_mut()
            .insert("x-agent-id", test_uuid(1).to_string().parse().unwrap());
        request
            .headers_mut()
            .insert("x-task-id", test_uuid(2).to_string().parse().unwrap());
        request
            .headers_mut()
            .insert("x-actor-source", "task_token".parse().unwrap());
        clear_untrusted_task_identity(&mut request);
        let identity = policy()
            .take_identity(&mut request)
            .expect("trusted identity");
        assert_eq!(identity.user_id, VICTIM);
        assert_eq!(identity.email, "victim@example.com");
        assert_eq!(
            request
                .headers()
                .get("x-user-id")
                .and_then(|value| value.to_str().ok()),
            Some(VICTIM)
        );
        assert!(request.headers().get("x-agent-id").is_none());
        assert!(request.headers().get("x-task-id").is_none());
        assert!(request.headers().get("x-actor-source").is_none());
        assert!(request
            .headers()
            .get(IDENTITY_PROXY_MARKER_HEADER)
            .is_none());
    }

    #[test]
    fn jwt_and_pat_paths_cannot_retain_forged_task_identity() {
        for bearer in ["header.payload.signature", "pby_secret"] {
            let mut request = request(Some("203.0.113.9:443"), bearer, MARKER);
            request
                .headers_mut()
                .insert("x-agent-id", test_uuid(3).to_string().parse().unwrap());
            request
                .headers_mut()
                .insert("x-task-id", test_uuid(4).to_string().parse().unwrap());
            request
                .headers_mut()
                .insert("x-actor-source", "task_token".parse().unwrap());

            clear_untrusted_task_identity(&mut request);

            assert!(request.headers().get("x-agent-id").is_none(), "{bearer}");
            assert!(request.headers().get("x-task-id").is_none(), "{bearer}");
            assert!(
                request.headers().get("x-actor-source").is_none(),
                "{bearer}"
            );
            let expected_authorization = format!("Bearer {bearer}");
            assert_eq!(
                request
                    .headers()
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok()),
                Some(expected_authorization.as_str())
            );
        }
    }

    #[test]
    fn authoritative_task_token_replaces_the_complete_actor_tuple() {
        let user_id = test_uuid(5);
        let agent_id = test_uuid(6);
        let task_id = test_uuid(7);
        let workspace_id = test_uuid(8);
        let lease_id = test_uuid(11);
        let on_behalf_of = test_uuid(12);
        let device_id = test_uuid(13);
        let mut request = request(Some("203.0.113.9:443"), "mat_secret", MARKER);
        request
            .headers_mut()
            .insert("x-agent-id", test_uuid(9).to_string().parse().unwrap());
        request
            .headers_mut()
            .insert("x-task-id", test_uuid(10).to_string().parse().unwrap());

        clear_untrusted_task_identity(&mut request);
        stamp_task_identity(
            &mut request,
            user_id,
            agent_id,
            task_id,
            workspace_id,
            lease_id,
            Some(on_behalf_of),
            Some(device_id),
        );

        for (name, expected) in [
            ("x-user-id", user_id),
            ("x-agent-id", agent_id),
            ("x-task-id", task_id),
            ("x-workspace-id", workspace_id),
            ("x-capability-lease-id", lease_id),
            ("x-on-behalf-of-user-id", on_behalf_of),
            ("x-device-id", device_id),
        ] {
            let expected = expected.to_string();
            assert_eq!(
                request
                    .headers()
                    .get(name)
                    .and_then(|value| value.to_str().ok()),
                Some(expected.as_str()),
                "{name}"
            );
        }
        assert_eq!(
            request
                .headers()
                .get("x-actor-source")
                .and_then(|value| value.to_str().ok()),
            Some("task_token")
        );
    }

    #[test]
    fn task_lease_cannot_mint_durable_or_human_control_plane_authority() {
        let workspace_id = test_uuid(14);
        let bound_workspace = format!("/api/workspaces/{workspace_id}");
        for (method, path) in [
            (Method::POST, "/api/cli-token"),
            (Method::POST, "/api/tokens"),
            (Method::GET, "/api/integrations/composio/connections"),
            (Method::GET, "/api/agents/private-agent/env"),
            (Method::POST, "/api/issues/quick-create"),
            (Method::POST, "/api/issues/issue-id/rerun"),
            (Method::DELETE, "/api/issues/issue-id"),
            (Method::POST, "/api/issues/batch-delete"),
            (Method::DELETE, "/api/comments/comment-id"),
            (
                Method::POST,
                "/api/runtimes/runtime-id/unbind-agents-and-delete",
            ),
            (Method::POST, "/api/cloud-runtime/nodes"),
            (Method::GET, "/api/workspaces"),
            (Method::GET, "/api/agent-task-snapshot"),
            (Method::GET, "/api/attachments/private-chat-file"),
            (Method::GET, "/api/chat/history"),
            (Method::GET, "/api/issues"),
            (Method::GET, "/api/issues/other/timeline"),
            (Method::POST, "/api/issues/query"),
            (Method::POST, "/api/issues/table/rows"),
            (Method::POST, "/api/issues/table/groups"),
            (Method::POST, "/api/issues/table/facets"),
            (Method::POST, "/api/upload-file"),
            (Method::GET, "/api/runtimes"),
            (Method::PATCH, "/api/runtimes/runtime-id"),
            (Method::GET, "/api/authorization/decisions/decision-id"),
            (Method::GET, bound_workspace.as_str()),
        ] {
            assert!(
                !task_token_route_allowed(&method, path, workspace_id),
                "{method} {path}"
            );
        }
    }

    #[test]
    fn task_lease_allowlist_is_limited_to_scoped_data_plane_routes() {
        let workspace_id = test_uuid(15);
        for (method, path) in [
            (Method::GET, "/api/issues/issue-id"),
            (Method::PUT, "/api/issues/issue-id"),
            (Method::POST, "/api/issues/issue-id/comments"),
            (Method::POST, "/api/issues"),
            (Method::POST, "/api/tasks/task-id/message-bus"),
        ] {
            assert!(
                task_token_route_allowed(&method, path, workspace_id),
                "{method} {path}"
            );
        }
        assert!(!task_token_route_allowed(
            &Method::GET,
            &format!("/api/workspaces/{}", test_uuid(16)),
            workspace_id,
        ));
    }

    #[test]
    fn source_and_marker_are_both_required() {
        for (peer, marker) in [
            (Some("203.0.113.9:443"), MARKER),
            (Some("10.2.3.4:443"), "wrong-marker"),
            (None, MARKER),
        ] {
            let mut request = request(peer, "pby_secret", marker);
            assert_eq!(policy().take_identity(&mut request), None);
            assert!(request.headers().get("x-user-id").is_none());
        }
    }

    #[test]
    fn trusted_identity_still_obeys_disabled_user_policy() {
        let mut request = Request::builder()
            .header("x-user-id", "514492f7-b30f-4147-bd33-c0e8ce5d6d4f")
            .header(IDENTITY_PROXY_MARKER_HEADER, MARKER)
            .body(Body::empty())
            .expect("request");
        request.extensions_mut().insert(ConnectInfo(
            "10.2.3.4:443"
                .parse::<SocketAddr>()
                .expect("socket address"),
        ));
        let identity = policy()
            .take_identity(&mut request)
            .expect("trusted identity");
        assert!(reject_disabled(
            &identity.user_id,
            &identity.email,
            "identity_proxy"
        ));
    }

    #[test]
    fn partial_or_weak_configuration_fails_closed() {
        assert!(IdentityProxyTrust::configured("", MARKER).is_err());
        assert!(IdentityProxyTrust::configured("10.0.0.0/8", "short").is_err());
        assert!(IdentityProxyTrust::configured("not-a-cidr", MARKER).is_err());
    }
}
