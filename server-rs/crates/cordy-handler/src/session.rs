//! Public session endpoints.

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};

use crate::error::error_response;
use crate::state::HandlerState;

pub fn public_router() -> Router<HandlerState> {
    Router::new().route("/auth/logout", post(logout))
}

async fn logout() -> Response {
    let domain = cordy_auth::cookie::configured_cookie_domain();
    let secure = cordy_auth::cookie::configured_secure_cookie();
    let mut headers = HeaderMap::new();

    for value in cordy_auth::cookie::clear_auth_cookie_values(domain, secure) {
        let Ok(value) = HeaderValue::from_str(&value) else {
            tracing::error!("failed to construct auth cookie clearing header");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to clear auth cookies",
            );
        };
        headers.append(header::SET_COOKIE, value);
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
        let state = HandlerState::new(pool, cordy_auth::pat_cache::PatCache::disabled(), None);
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
        assert!(cookies[1].starts_with("cordy_csrf=;"));
        assert!(!cookies[1].contains("; HttpOnly"));

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, r#"{"message":"logged out"}"#.as_bytes());
    }
}
