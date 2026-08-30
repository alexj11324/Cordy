//! Public session endpoints.

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{extract::State, Json, Router};
use patchbay_auth::jwt::hash_token;
use patchbay_db::queries::guest as guest_queries;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn public_router() -> Router<HandlerState> {
    Router::new().route("/auth/logout", post(logout))
}

async fn logout(State(state): State<HandlerState>, headers: HeaderMap) -> Response {
    // Desktop guest sessions use a bearer token rather than the cookie
    // session used by the web app. Revoke that server-side before clearing
    // client state so copied guest credentials stop authenticating at logout.
    if let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| value.starts_with("pbg_"))
    {
        if let Err(error) =
            guest_queries::revoke_active_by_token_hash(&state.pool, &hash_token(token)).await
        {
            tracing::warn!(%error, "guest auth: failed to revoke session on logout");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "guest session revocation unavailable",
            );
        }
    }

    let (domain, secure) = state.auth_settings.cookie_attributes();
    let mut headers = HeaderMap::new();

    for value in patchbay_auth::cookie::clear_auth_cookie_values(domain.as_deref(), secure)
        .into_iter()
        .chain(patchbay_auth::cookie::clear_legacy_auth_cookie_values(
            domain.as_deref(),
            secure,
        ))
    {
        let Ok(value) = HeaderValue::from_str(&value) else {
            tracing::error!("failed to construct auth cookie clearing header");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to clear auth cookies",
            );
        };
        headers.append(header::SET_COOKIE, value);
    }
    if let Some(signer) = state.attachment_download.cloudfront_signer.as_ref() {
        for value in signer.clear_cookie_headers() {
            let Ok(value) = HeaderValue::from_str(&value) else {
                tracing::error!("failed to construct CloudFront cookie clearing header");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to clear auth cookies",
                );
            };
            headers.append(header::SET_COOKIE, value);
        }
    }

    (headers, Json(serde_json::json!({"message": "logged out"}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn logout_clears_both_cookies_and_returns_go_body() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let mut config = patchbay_config::Config::default();
        config.auth.cookie_domain = Some(" .example.com ".into());
        config.urls.frontend_origin = Some(" https://app.example.com ".into());
        let state = HandlerState::new(pool, patchbay_auth::pat_cache::PatCache::disabled(), None)
            .with_auth_settings(crate::auth::AuthSettings::from_config(&config));
        let response = public_router()
            .with_state(state)
            .oneshot(Request::post("/auth/logout").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let cookies = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(cookies.len(), 4);
        assert!(cookies[0].starts_with("patchbay_auth=;"));
        assert!(cookies[0].contains("; HttpOnly"));
        assert!(cookies[0].contains("; Domain=.example.com"));
        assert!(cookies[0].contains("; Secure"));
        assert!(cookies[1].starts_with("patchbay_csrf=;"));
        assert!(!cookies[1].contains("; HttpOnly"));
        assert!(cookies[1].contains("; Domain=.example.com"));
        assert!(cookies[1].contains("; Secure"));
        assert!(cookies[2].starts_with("cordy_auth=;")); // legacy-brand-compat
        assert!(cookies[3].starts_with("cordy_csrf=;")); // legacy-brand-compat

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, r#"{"message":"logged out"}"#.as_bytes());
    }
}
