//! Public session endpoints.

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{extract::State, Json, Router};

use crate::error::error_response;
use crate::state::HandlerState;

pub fn public_router() -> Router<HandlerState> {
    Router::new().route("/auth/logout", post(logout))
}

async fn logout(State(state): State<HandlerState>) -> Response {
    let (domain, secure) = state.auth_settings.cookie_attributes();
    let mut headers = HeaderMap::new();

    for value in cordy_auth::cookie::clear_auth_cookie_values(domain.as_deref(), secure) {
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
        let mut config = cordy_config::Config::default();
        config.auth.cookie_domain = Some(" .example.com ".into());
        config.urls.frontend_origin = Some(" https://app.example.com ".into());
        let state = HandlerState::new(pool, cordy_auth::pat_cache::PatCache::disabled(), None)
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
        assert_eq!(cookies.len(), 2);
        assert!(cookies[0].starts_with("cordy_auth=;"));
        assert!(cookies[0].contains("; HttpOnly"));
        assert!(cookies[0].contains("; Domain=.example.com"));
        assert!(cookies[0].contains("; Secure"));
        assert!(cookies[1].starts_with("cordy_csrf=;"));
        assert!(!cookies[1].contains("; HttpOnly"));
        assert!(cookies[1].contains("; Domain=.example.com"));
        assert!(cookies[1].contains("; Secure"));

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, r#"{"message":"logged out"}"#.as_bytes());
    }
}
