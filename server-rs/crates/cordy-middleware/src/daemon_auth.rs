//! Daemon auth middleware.
//!
//! Validates daemon auth tokens (`mdt_` prefix) or falls back to JWT/PAT
//! validation for backward compatibility with daemons that authenticate via
//! user tokens.
//!
//! Identity is injected as a [`DaemonContext`] request extension plus the same
//! `X-User-ID` header contract as the regular Auth middleware. The resolved
//! auth path ("daemon_token"/"pat"/"cloud_pat"/"jwt") rides along for
//! slow-log attribution.

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use cordy_auth::daemon_token_cache::{DaemonTokenCache, DaemonTokenIdentity};
use cordy_auth::disabled_users::is_temporarily_disabled_user;
use cordy_auth::jwt::hash_token;
use cordy_auth::pat_cache::{ttl_for_expiry, PatCache};
use cordy_db::queries::{daemon_token, personal_access_token};

use crate::auth::AuthSideEffectSpawner;

/// Cloud node PAT prefix. Fail-closed until the Cloud Fleet verifier lands
/// with the integrations port — mirrors Go when CORDY_CLOUD_FLEET_URL unset.
pub const DAEMON_WORKSPACE_HEADER: &str = "x-cordy-daemon-workspace-id";
pub const DAEMON_ID_HEADER: &str = "x-cordy-daemon-id";

/// Auth path labels exposed via [`DaemonContext`] for telemetry.
pub const DAEMON_AUTH_PATH_DAEMON_TOKEN: &str = "daemon_token";
pub const DAEMON_AUTH_PATH_PAT: &str = "pat";
pub const DAEMON_AUTH_PATH_CLOUD_PAT: &str = "cloud_pat";
pub const DAEMON_AUTH_PATH_JWT: &str = "jwt";

/// Daemon identity + auth path injected into request extensions.
#[derive(Clone)]
pub struct DaemonContext {
    /// Set only on the daemon_token path.
    pub workspace_id: Option<String>,
    /// Set only on the daemon_token path.
    pub daemon_id: Option<String>,
    pub auth_path: &'static str,
}

#[derive(Clone)]
pub struct DaemonAuthState {
    pub pool: sqlx::PgPool,
    /// Shared with the regular Auth middleware so a hot PAT used by both a
    /// human CLI and a daemon converges on one DB round-trip per TTL window.
    pub pat_cache: PatCache,
    pub daemon_cache: DaemonTokenCache,
    pub cloud_pat_verifier: Option<cordy_auth::cloud_pat::CloudPatVerifier>,
    pub side_effects: std::sync::Arc<dyn AuthSideEffectSpawner>,
}

fn reject_disabled(user_id: &str, email: &str, auth_path: &str) -> bool {
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

/// DaemonAuth middleware entrypoint — use via
/// `axum::middleware::from_fn_with_state(state, daemon_auth_middleware)`.
pub async fn daemon_auth_middleware(
    State(state): State<DaemonAuthState>,
    mut req: Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    clear_untrusted_identity_headers(&mut req);

    let Some(auth_header) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
    else {
        tracing::debug!(path = ?req.uri().path(), "daemon_auth: missing authorization header");
        return Err((
            StatusCode::UNAUTHORIZED,
            r#"{"error":"missing authorization header"}"#,
        ));
    };

    // Bearer prefix required — an unprefixed header falls to the invalid
    // format branch here (unlike Auth, there is no cookie fallback).
    let Some(token) = auth_header.strip_prefix("Bearer ") else {
        tracing::debug!(path = ?req.uri().path(), "daemon_auth: invalid format");
        return Err((
            StatusCode::UNAUTHORIZED,
            r#"{"error":"invalid authorization format"}"#,
        ));
    };
    let hash = hash_token(token);

    // Daemon token: "mdt_" prefix — the primary path.
    if token.starts_with("mdt_") {
        // Cache hit short-circuits the DB lookup entirely.
        if let Some(id) = state.daemon_cache.get(&hash).await {
            if let Ok(workspace) = id.workspace_id.parse() {
                req.headers_mut().insert(DAEMON_WORKSPACE_HEADER, workspace);
            }
            if let Ok(daemon) = id.daemon_id.parse() {
                req.headers_mut().insert(DAEMON_ID_HEADER, daemon);
            }
            req.extensions_mut().insert(DaemonContext {
                workspace_id: Some(id.workspace_id),
                daemon_id: Some(id.daemon_id),
                auth_path: DAEMON_AUTH_PATH_DAEMON_TOKEN,
            });
            return Ok(next.run(req).await);
        }

        let Some(dt) = daemon_token::get_daemon_token_by_hash(&state.pool, &hash)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "daemon_auth: daemon token lookup failed");
                None
            })
        else {
            tracing::warn!(path = ?req.uri().path(), "daemon_auth: invalid daemon token");
            return Err((
                StatusCode::UNAUTHORIZED,
                r#"{"error":"invalid daemon token"}"#,
            ));
        };

        let identity = DaemonTokenIdentity {
            workspace_id: dt.workspace_id.to_string(),
            daemon_id: dt.daemon_id.clone(),
        };
        if let Ok(workspace) = identity.workspace_id.parse() {
            req.headers_mut().insert(DAEMON_WORKSPACE_HEADER, workspace);
        }
        if let Ok(daemon) = identity.daemon_id.parse() {
            req.headers_mut().insert(DAEMON_ID_HEADER, daemon);
        }
        // expires_at is NOT NULL; SQL also filters expired rows.
        let ttl = ttl_for_expiry(chrono::Utc::now(), Some(dt.expires_at));
        state.daemon_cache.set(&hash, &identity, ttl).await;

        req.extensions_mut().insert(DaemonContext {
            workspace_id: Some(identity.workspace_id),
            daemon_id: Some(identity.daemon_id),
            auth_path: DAEMON_AUTH_PATH_DAEMON_TOKEN,
        });
        return Ok(next.run(req).await);
    }

    // Cloud Node PAT: "mcn_" prefix. Cordy Cloud Fleet is authoritative; we
    // only surface the resolved owner_id as X-User-ID downstream. Same
    // fail-closed semantics as Auth: no verifier configured → 401, Fleet
    // unreachable → 503.
    if token.starts_with(cordy_auth::cloud_pat::CLOUD_PAT_PREFIX) {
        let owner_id = match crate::auth::verify_cloud_pat(
            &state.pool,
            state.cloud_pat_verifier.as_ref(),
            token,
        )
        .await
        {
            Ok(owner_id) => owner_id,
            Err(crate::auth::CloudAuthError::Invalid) => {
                tracing::warn!(path = ?req.uri().path(), "daemon_auth: cloud PAT rejected");
                return Err((StatusCode::UNAUTHORIZED, r#"{"error":"invalid token"}"#));
            }
            Err(crate::auth::CloudAuthError::Unavailable) => {
                tracing::warn!(path = ?req.uri().path(), "daemon_auth: cloud PAT verifier unavailable");
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    r#"{"error":"cloud pat verifier unavailable"}"#,
                ));
            }
        };
        if reject_disabled(&owner_id, "", DAEMON_AUTH_PATH_CLOUD_PAT) {
            return Err(err_disabled());
        }
        set_user(&mut req, &owner_id);
        req.headers_mut()
            .insert("x-actor-source", HeaderValue::from_static("cloud_pat"));
        req.extensions_mut().insert(DaemonContext {
            workspace_id: None,
            daemon_id: None,
            auth_path: DAEMON_AUTH_PATH_CLOUD_PAT,
        });
        return Ok(next.run(req).await);
    }

    // Fallback: PAT tokens ("mul_" prefix).
    if token.starts_with("mul_") {
        if let Some(user_id) = state.pat_cache.get(&hash).await {
            if reject_disabled(&user_id, "", DAEMON_AUTH_PATH_PAT) {
                return Err(err_disabled());
            }
            set_user(&mut req, &user_id);
            req.extensions_mut().insert(DaemonContext {
                workspace_id: None,
                daemon_id: None,
                auth_path: DAEMON_AUTH_PATH_PAT,
            });
            return Ok(next.run(req).await);
        }

        let Some(pat) =
            personal_access_token::get_personal_access_token_by_hash(&state.pool, &hash)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "daemon_auth: invalid PAT");
                    None
                })
        else {
            tracing::warn!(path = ?req.uri().path(), "daemon_auth: invalid PAT");
            return Err((StatusCode::UNAUTHORIZED, r#"{"error":"invalid token"}"#));
        };

        let user_id = pat.user_id.to_string();
        if reject_disabled(&user_id, "", DAEMON_AUTH_PATH_PAT) {
            return Err(err_disabled());
        }
        set_user(&mut req, &user_id);

        let ttl = ttl_for_expiry(chrono::Utc::now(), pat.expires_at);
        state.pat_cache.set(&hash, &user_id, ttl).await;

        // Cache miss = first request in this TTL window; refresh last_used_at
        // asynchronously, subsequent hits skip the write entirely.
        let pool = state.pool.clone();
        let pat_id = pat.id;
        state.side_effects.spawn(Box::pin(async move {
            if let Err(e) =
                personal_access_token::update_personal_access_token_last_used(&pool, pat_id).await
            {
                tracing::warn!(error = %e, "daemon_auth: failed to refresh PAT last_used_at");
            }
        }));

        req.extensions_mut().insert(DaemonContext {
            workspace_id: None,
            daemon_id: None,
            auth_path: DAEMON_AUTH_PATH_PAT,
        });
        return Ok(next.run(req).await);
    }

    // Fallback: JWT tokens.
    let claims = crate::auth::decode_jwt_claims(token);
    let Some(claims) = claims else {
        tracing::warn!(path = ?req.uri().path(), "daemon_auth: invalid token");
        return Err((StatusCode::UNAUTHORIZED, r#"{"error":"invalid token"}"#));
    };
    let sub = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(sub) = sub else {
        return Err((StatusCode::UNAUTHORIZED, r#"{"error":"invalid claims"}"#));
    };
    let email = claims.get("email").and_then(|v| v.as_str()).unwrap_or("");
    if reject_disabled(sub, email, DAEMON_AUTH_PATH_JWT) {
        return Err(err_disabled());
    }
    set_user(&mut req, sub);
    req.extensions_mut().insert(DaemonContext {
        workspace_id: None,
        daemon_id: None,
        auth_path: DAEMON_AUTH_PATH_JWT,
    });
    Ok(next.run(req).await)
}

/// Clears every identity header owned by authentication before inspecting the
/// presented credential. In particular, an `mdt_` token deliberately has no
/// human user identity; retaining a client-supplied `X-User-ID` would let that
/// daemon enter handlers or WebSocket indexes as an arbitrary user.
fn clear_untrusted_identity_headers(req: &mut Request) {
    req.headers_mut().remove("x-user-id");
    req.headers_mut().remove("x-user-email");
    req.headers_mut().remove("x-actor-source");
    req.headers_mut().remove(DAEMON_WORKSPACE_HEADER);
    req.headers_mut().remove(DAEMON_ID_HEADER);
}

fn err_disabled() -> (StatusCode, &'static str) {
    (StatusCode::FORBIDDEN, r#"{"error":"account disabled"}"#)
}

fn set_user(req: &mut Request, user_id: &str) {
    use axum::http::HeaderValue;
    if let Ok(v) = HeaderValue::from_str(user_id) {
        req.headers_mut().insert("x-user-id", v);
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;

    use super::*;

    #[test]
    fn daemon_auth_boundary_drops_spoofed_human_identity() {
        let victim_id = "01972f7e-7e8d-77ef-a13d-1b0ce3e9c001";
        let mut request = Request::builder()
            .header("x-user-id", victim_id)
            .header("x-user-email", "victim@example.com")
            .header("x-actor-source", "jwt")
            .header(DAEMON_WORKSPACE_HEADER, "spoofed-workspace")
            .header(DAEMON_ID_HEADER, "spoofed-daemon")
            .body(Body::empty())
            .unwrap();

        clear_untrusted_identity_headers(&mut request);

        for name in [
            "x-user-id",
            "x-user-email",
            "x-actor-source",
            DAEMON_WORKSPACE_HEADER,
            DAEMON_ID_HEADER,
        ] {
            assert!(
                request.headers().get(name).is_none(),
                "client-controlled {name} crossed the daemon auth boundary"
            );
        }
    }

    #[test]
    fn authenticated_user_can_be_restamped_after_boundary_clear() {
        let authenticated_id = "01972f7e-7e8d-77ef-a13d-1b0ce3e9c002";
        let mut request = Request::builder()
            .header("x-user-id", "01972f7e-7e8d-77ef-a13d-1b0ce3e9c001")
            .body(Body::empty())
            .unwrap();

        clear_untrusted_identity_headers(&mut request);
        set_user(&mut request, authenticated_id);

        assert_eq!(
            request
                .headers()
                .get("x-user-id")
                .and_then(|value| value.to_str().ok()),
            Some(authenticated_id)
        );
    }
}
