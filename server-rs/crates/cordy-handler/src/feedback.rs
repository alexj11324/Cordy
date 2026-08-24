//! Authenticated product feedback endpoint.

use std::sync::LazyLock;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use cordy_db::queries::feedback;
use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const FEEDBACK_MAX_MESSAGE_LEN: usize = 10_000;
const FEEDBACK_HOURLY_RATE_LIMIT: i64 = 10;
const FEEDBACK_BODY_LIMIT: usize = 64 * 1024;
const DESKTOP_ROUTE_ERROR_KIND: &str = "desktop_route_error";

static FEEDBACK_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\([^)]+\)").expect("feedback image regex is valid"));

pub fn router() -> Router<HandlerState> {
    Router::new().route("/api/feedback", post(create_feedback))
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct FeedbackErrorContext {
    #[serde(default)]
    name: String,
    #[serde(default)]
    message: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    stack: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct FeedbackContext {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    trigger: String,
    #[serde(default)]
    error: FeedbackErrorContext,
}

#[derive(Debug, Deserialize)]
struct CreateFeedbackRequest {
    #[serde(default)]
    message: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    kind: String,
    workspace_id: Option<String>,
    context: Option<FeedbackContext>,
}

#[derive(Debug, Serialize)]
struct FeedbackResponse {
    id: String,
    created_at: String,
}

async fn create_feedback(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(user_id) = header_uuid(&headers, "x-user-id") else {
        return error_response(StatusCode::UNAUTHORIZED, "user not authenticated");
    };
    if body.len() > FEEDBACK_BODY_LIMIT {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    }
    let request: CreateFeedbackRequest = match decode_json_body(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let message = request.message.trim();
    if message.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "message is required");
    }
    if message.len() > FEEDBACK_MAX_MESSAGE_LEN {
        return error_response(StatusCode::BAD_REQUEST, "message too long");
    }
    if !valid_feedback_context(request.context.as_ref()) {
        return error_response(StatusCode::BAD_REQUEST, "invalid feedback context");
    }

    let recent = match feedback::count_recent_feedback_by_user(&state.pool, user_id).await {
        Ok(count) => count.unwrap_or_default(),
        Err(error) => {
            tracing::warn!(%error, %user_id, "count recent feedback failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to check rate limit",
            );
        }
    };
    if recent >= FEEDBACK_HOURLY_RATE_LIMIT {
        return error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "too many feedback submissions, please try again later",
        );
    }

    let platform = header_value(&headers, "x-client-platform");
    let version = header_value(&headers, "x-client-version");
    let client_os = header_value(&headers, "x-client-os");
    let user_agent = header_value(&headers, "user-agent");
    let mut metadata = serde_json::json!({
        "url": request.url,
        "platform": platform,
        "version": version,
        "os": client_os,
        "user_agent": user_agent,
    });
    if let Some(context) = request.context.as_ref() {
        metadata["context"] = serde_json::to_value(context).unwrap_or(serde_json::Value::Null);
    }

    let workspace_id = match request.workspace_id.as_deref() {
        Some(raw) if !raw.is_empty() => match Uuid::parse_str(raw.trim()) {
            Ok(workspace_id) => Some(workspace_id),
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace_id"),
        },
        _ => None,
    };
    let created =
        match feedback::create_feedback(&state.pool, user_id, message, &metadata, workspace_id)
            .await
        {
            Ok(Some(feedback)) => feedback,
            Ok(None) | Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to submit feedback",
                )
            }
        };

    if let Some(metrics) = state.business_metrics.as_deref() {
        let kind = match request.kind.trim() {
            "" => "general",
            kind => kind,
        };
        metrics.inc_for_event(&cordy_analytics::feedback_submitted(
            &user_id.to_string(),
            &created
                .workspace_id
                .map(|workspace_id| workspace_id.to_string())
                .unwrap_or_default(),
            kind,
            message.len() as i64,
            FEEDBACK_IMAGE.is_match(message),
            platform,
            version,
        ));
    }

    (
        StatusCode::CREATED,
        Json(FeedbackResponse {
            id: created.id.to_string(),
            created_at: crate::timefmt::rfc3339(created.created_at),
        }),
    )
        .into_response()
}

fn valid_feedback_context(context: Option<&FeedbackContext>) -> bool {
    context.is_none_or(|context| {
        context.kind == DESKTOP_ROUTE_ERROR_KIND
            && !context.trigger.trim().is_empty()
            && !context.error.name.trim().is_empty()
            && !context.error.message.trim().is_empty()
    })
}

fn header_uuid(headers: &HeaderMap, name: &str) -> Option<Uuid> {
    Uuid::parse_str(header_value(headers, name)).ok()
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
}

fn decode_json_body<T>(body: &[u8]) -> Result<T, serde_json::Error>
where
    T: serde::de::DeserializeOwned,
{
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    T::deserialize(&mut deserializer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_context_validation_matches_desktop_error_contract() {
        assert!(valid_feedback_context(None));
        assert!(valid_feedback_context(Some(&FeedbackContext {
            kind: DESKTOP_ROUTE_ERROR_KIND.into(),
            trigger: "route-change".into(),
            error: FeedbackErrorContext {
                name: "Error".into(),
                message: "failed".into(),
                stack: String::new(),
            },
        })));
        assert!(!valid_feedback_context(Some(&FeedbackContext {
            kind: "other".into(),
            trigger: "route-change".into(),
            error: FeedbackErrorContext {
                name: "Error".into(),
                message: "failed".into(),
                stack: String::new(),
            },
        })));
    }

    #[test]
    fn markdown_image_detection_is_coarse_like_go() {
        assert!(FEEDBACK_IMAGE.is_match("see ![screen](https://example.test/a.png)"));
        assert!(!FEEDBACK_IMAGE.is_match("plain feedback"));
    }

    #[test]
    fn omitted_fields_decode_to_go_zero_values() {
        let request: CreateFeedbackRequest = decode_json_body(br#"{"context":{}}"#).unwrap();
        assert!(request.message.is_empty());
        assert!(!valid_feedback_context(request.context.as_ref()));
    }
}
