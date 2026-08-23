//! Workspace domain handlers — first slice of the route port (S8).
//!
//! Port of `server/internal/handler/workspace.go` (ListWorkspaces /
//! GetWorkspace) and `share_link.go` GetShareLinkInfo. Wire shapes match the
//! Go structs field-for-field: UUIDs as hyphenated strings, timestamps as
//! RFC3339, nullable columns as absent-or-null JSON.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::models::Workspace;
use cordy_db::queries::{member, share_link, workspace};
use serde::Serialize;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn public_router() -> Router<HandlerState> {
    Router::new().route("/api/share-links/{code}", get(get_share_link_info))
}

/// Authenticated workspace routes from router.go. The collection is user
/// scoped; the item route additionally requires membership in the workspace
/// named by `{id}`.
pub fn authenticated_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/workspaces", get(list_workspaces))
        .route("/api/workspaces/", get(list_workspaces))
        .route("/api/workspaces/{id}", get(get_workspace))
        .route("/api/workspaces/{id}/", get(get_workspace))
}

/// GET /api/share-links/{code} — public preview of a workspace share link.
async fn get_share_link_info(
    State(state): State<HandlerState>,
    Path(code): Path<String>,
) -> Response {
    let code = code.trim();
    if code.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "code is required");
    }
    let Some(row) = share_link::get_share_link_info_by_code(&state.pool, code)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "share link lookup failed");
            None
        })
    else {
        return error_response(StatusCode::NOT_FOUND, "share link not found or expired");
    };
    Json(ShareLinkInfoResponse {
        workspace_name: row.workspace_name,
        workspace_slug: row.workspace_slug,
        creator_name: row.creator_name,
        role: row.role,
    })
    .into_response()
}

#[derive(Serialize)]
struct ShareLinkInfoResponse {
    workspace_name: String,
    workspace_slug: String,
    creator_name: String,
    role: String,
}

/// GET /api/workspaces — list the workspaces visible to the authenticated
/// user. Authentication stamps `x-user-id`; never trust a client-provided
/// workspace id for this user-scoped collection.
async fn list_workspaces(
    State(state): State<HandlerState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(user_id) = header_uuid(&headers, "x-user-id") else {
        return error_response(StatusCode::UNAUTHORIZED, "user not authenticated");
    };

    match workspace::list_workspaces(&state.pool, user_id).await {
        Ok(rows) => Json(
            rows.into_iter()
                .map(WorkspaceResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to list workspaces");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list workspaces",
            )
        }
    }
}

/// GET /api/workspaces/{id} — resolve membership before returning the row.
/// Returning 404 for non-members preserves the Go guard's non-enumeration
/// contract.
async fn get_workspace(
    State(state): State<HandlerState>,
    Path(raw_id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Ok(id) = Uuid::parse_str(raw_id.trim()) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid workspace_id");
    };
    if headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        == Some("task_token")
        && header_uuid(&headers, "x-workspace-id") != Some(id)
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "task token is bound to a different workspace",
        );
    }
    let Some(user_id) = header_uuid(&headers, "x-user-id") else {
        return error_response(StatusCode::UNAUTHORIZED, "user not authenticated");
    };
    let is_member = member::get_member_by_user_and_workspace(&state.pool, user_id, id)
        .await
        .ok()
        .flatten()
        .is_some();
    if !is_member {
        return error_response(StatusCode::NOT_FOUND, "workspace not found");
    }

    match workspace::get_workspace(&state.pool, id).await {
        Ok(Some(row)) => Json(WorkspaceResponse::from(row)).into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "workspace not found"),
        Err(error) => {
            tracing::warn!(%error, workspace_id = %id, "failed to get workspace");
            error_response(StatusCode::NOT_FOUND, "workspace not found")
        }
    }
}

fn header_uuid(headers: &axum::http::HeaderMap, name: &str) -> Option<Uuid> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
}

#[derive(Debug, Serialize)]
struct WorkspaceResponse {
    id: String,
    name: String,
    slug: String,
    description: Option<String>,
    context: Option<String>,
    settings: serde_json::Value,
    repos: serde_json::Value,
    issue_prefix: String,
    avatar_url: Option<String>,
    created_at: String,
    updated_at: String,
}

impl From<Workspace> for WorkspaceResponse {
    fn from(workspace: Workspace) -> Self {
        Self {
            id: workspace.id.to_string(),
            name: workspace.name,
            slug: workspace.slug,
            description: workspace.description,
            context: workspace.context,
            settings: workspace.settings,
            repos: workspace.repos,
            issue_prefix: workspace.issue_prefix,
            avatar_url: workspace.avatar_url,
            created_at: crate::timefmt::rfc3339(workspace.created_at),
            updated_at: crate::timefmt::rfc3339(workspace.updated_at),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use tower::ServiceExt;

    fn test_router() -> Router {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let state = HandlerState::new(
            pool.clone(),
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        );
        authenticated_router().with_state(state)
    }

    #[tokio::test]
    async fn malformed_workspace_id_uses_json_error_contract() {
        let response = test_router()
            .oneshot(
                Request::get("/api/workspaces/not-a-uuid")
                    .header("x-user-id", Uuid::nil().to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(bytes, r#"{"error":"invalid workspace_id"}"#.as_bytes());
    }

    #[tokio::test]
    async fn task_token_cannot_cross_its_bound_workspace() {
        let requested = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let bound = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        let response = test_router()
            .oneshot(
                Request::get(format!("/api/workspaces/{requested}"))
                    .header("x-user-id", Uuid::nil().to_string())
                    .header("x-actor-source", "task_token")
                    .header("x-workspace-id", bound.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn workspace_response_matches_go_wire_shape() {
        let id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f11").unwrap();
        let timestamp = Utc.with_ymd_and_hms(2026, 8, 23, 2, 30, 0).unwrap();
        let response = WorkspaceResponse::from(Workspace {
            attribution_fail_closed: false,
            avatar_url: None,
            context: Some("context".into()),
            created_at: timestamp,
            description: None,
            id,
            issue_counter: 7,
            issue_prefix: "CORD".into(),
            name: "Cordy".into(),
            repos: json!([]),
            settings: json!({}),
            slug: "cordy".into(),
            updated_at: timestamp,
        });

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "id": id.to_string(),
                "name": "Cordy",
                "slug": "cordy",
                "description": null,
                "context": "context",
                "settings": {},
                "repos": [],
                "issue_prefix": "CORD",
                "avatar_url": null,
                "created_at": "2026-08-23T02:30:00Z",
                "updated_at": "2026-08-23T02:30:00Z"
            })
        );
    }

    #[test]
    fn workspace_response_strips_fractional_timestamp_seconds() {
        let timestamp = chrono::DateTime::parse_from_rfc3339("2026-08-23T02:30:00.987Z")
            .unwrap()
            .with_timezone(&Utc);
        let response = WorkspaceResponse::from(Workspace {
            attribution_fail_closed: false,
            avatar_url: None,
            context: None,
            created_at: timestamp,
            description: None,
            id: Uuid::nil(),
            issue_counter: 0,
            issue_prefix: "CORD".into(),
            name: "Cordy".into(),
            repos: json!([]),
            settings: json!({}),
            slug: "cordy".into(),
            updated_at: timestamp,
        });

        assert_eq!(response.created_at, "2026-08-23T02:30:00Z");
        assert_eq!(response.updated_at, "2026-08-23T02:30:00Z");
    }
}
