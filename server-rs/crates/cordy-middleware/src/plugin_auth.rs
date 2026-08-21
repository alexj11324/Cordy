//! Plugin auth routing — port of `server/internal/middleware/plugin_auth.go`.
//!
//! Lets the Action API be reached two ways without weakening either:
//!
//! The original way is the only one a SURFACE has: no credential at all. The
//! iframe posts a message to the host page, the host re-issues the call on the
//! signed-in user's session, and this middleware sends it through the ordinary
//! Auth chain like any other request.
//!
//! Hooks add a second way. A plugin's own server has no session and never
//! will, so when it presents a plugin bearer token this middleware steps
//! aside and lets the handler resolve the token itself — which it must,
//! because only the handler knows which installation and which scopes that
//! token stands for.
//!
//! Stepping aside is not the same as skipping authentication: every Action API
//! handler starts by resolving a caller, and a request that arrives with
//! neither a session nor a valid token fails there.

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

use crate::auth::{auth_middleware, AuthState};

/// Pulls the raw credential out of an Authorization header.
/// Case-insensitive `Bearer ` prefix; empty when absent or malformed.
pub fn bearer_token(headers: &HeaderMap) -> String {
    let header_value = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim();
    if header_value.is_empty() {
        return String::new();
    }
    const PREFIX: &str = "Bearer ";
    if header_value.len() <= PREFIX.len()
        || !header_value[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
    {
        return String::new();
    }
    header_value[PREFIX.len()..].trim().to_string()
}

/// Reports whether a credential is one of ours to resolve.
///
/// Prefix-matched rather than validated: this only decides which code path
/// gets to look at the token, and an invalid token routed here is refused by
/// the handler a moment later. Deciding by prefix keeps a plugin token from
/// being tried against the PAT cache and a PAT from being tried against
/// installations.
pub fn is_plugin_bearer_token(token: &str) -> bool {
    token.starts_with("mpi_") || token.starts_with("mpc_")
}

/// PluginAuth wrapper — use via
/// `axum::middleware::from_fn_with_state(state, plugin_auth)`. Requests
/// bearing a plugin token go straight to the handler; everything else runs
/// through the ordinary Auth chain.
pub async fn plugin_auth(
    State(state): State<AuthState>,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    if is_plugin_bearer_token(&bearer_token(req.headers())) {
        return Ok(next.run(req).await);
    }
    auth_middleware(State(state), req, next).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn hm(value: &str) -> HeaderMap {
        let mut m = HeaderMap::new();
        m.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        m
    }

    #[test]
    fn bearer_token_extracts_case_insensitively() {
        assert_eq!(bearer_token(&hm("Bearer mpi_x")), "mpi_x");
        assert_eq!(bearer_token(&hm("bearer mpi_x")), "mpi_x");
        assert_eq!(bearer_token(&hm("BeArEr  mpi_x  ")), "mpi_x");
    }

    #[test]
    fn bearer_token_rejects_malformed() {
        assert_eq!(bearer_token(&HeaderMap::new()), "");
        assert_eq!(bearer_token(&hm("")), "");
        assert_eq!(bearer_token(&hm("Basic abc")), "");
        assert_eq!(bearer_token(&hm("Bearer")), "");
        assert_eq!(bearer_token(&hm("Bearer ")), "");
    }

    #[test]
    fn plugin_prefixes_recognized() {
        assert!(is_plugin_bearer_token("mpi_abc"));
        assert!(is_plugin_bearer_token("mpc_abc"));
        assert!(!is_plugin_bearer_token("mul_abc"));
        assert!(!is_plugin_bearer_token("mdt_abc"));
        assert!(!is_plugin_bearer_token(""));
    }
}
