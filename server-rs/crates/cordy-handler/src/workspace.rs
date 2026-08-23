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
use cordy_db::queries::share_link;
use serde::Serialize;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new().route("/api/share-links/{code}", get(get_share_link_info))
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
