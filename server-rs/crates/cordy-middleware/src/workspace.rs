//! Workspace guards — port of `server/internal/middleware/workspace.go`.
//!
//! Four Go variants collapse onto one middleware configured via
//! [`WorkspaceGuardState`]:
//! - `RequireWorkspaceMember`            → member-only, slug/id resolution
//! - `RequireWorkspaceRole(roles...)`    → + role check
//! - `RequireWorkspaceMemberFromURL(p)`  → id from URL path parameter
//! - `RequireWorkspaceRoleFromURL(p, …)` → both
//!
//! For the FromURL variants the workspace id is read from the matched route
//! pattern (`MatchedPath`) — attach these with `Router::route_layer` so the
//! pattern is available, mirroring chi's post-routing middleware semantics.
//!
//! Identity comes from the `X-User-ID` header stamped by the auth middleware;
//! the resolved workspace + member are injected as a [`WorkspaceContext`]
//! request extension for downstream handlers.

use axum::extract::{MatchedPath, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use cordy_db::models::Member;
use cordy_db::queries::{member as member_q, workspace as ws_q};
use uuid::Uuid;

/// Workspace ID + resolved member injected into request extensions
/// (`SetMemberContext` equivalent).
#[derive(Clone)]
pub struct WorkspaceContext {
    pub workspace_id: String,
    pub member: Member,
}

/// Configuration for the unified workspace guard middleware.
#[derive(Clone)]
pub struct WorkspaceGuardState {
    pub pool: sqlx::PgPool,
    /// When set, the workspace id is taken from this URL path parameter.
    pub url_param: Option<&'static str>,
    /// When non-empty, the member's role must be one of these.
    pub roles: Vec<String>,
}

impl WorkspaceGuardState {
    pub fn member_only(pool: sqlx::PgPool) -> Self {
        Self {
            pool,
            url_param: None,
            roles: Vec::new(),
        }
    }

    pub fn with_roles(pool: sqlx::PgPool, roles: Vec<String>) -> Self {
        Self {
            pool,
            url_param: None,
            roles,
        }
    }

    pub fn from_url(pool: sqlx::PgPool, param: &'static str) -> Self {
        Self {
            pool,
            url_param: Some(param),
            roles: Vec::new(),
        }
    }

    pub fn from_url_with_roles(
        pool: sqlx::PgPool,
        param: &'static str,
        roles: Vec<String>,
    ) -> Self {
        Self {
            pool,
            url_param: Some(param),
            roles,
        }
    }
}

fn header<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.headers().get(name).and_then(|v| v.to_str().ok())
}

fn query_param<'a>(req: &'a Request, key: &str) -> Option<&'a str> {
    let query = req.uri().query()?;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key && !v.is_empty()).then_some(v)
    })
    // NOTE: no percent-decoding — workspace slugs are URL-safe by construction.
}

/// Reads a URL path parameter by matching the actual URI against the
/// `MatchedPath` pattern (axum 0.8 `{param}` syntax, plus legacy `:param`).
fn url_param(req: &Request, param: &str) -> Option<String> {
    let matched = req.extensions().get::<MatchedPath>()?;
    let pattern = matched.as_str();
    let path = req.uri().path();
    let mut pat_segs = pattern.split('/');
    let mut path_segs = path.split('/');
    loop {
        match (pat_segs.next(), path_segs.next()) {
            (Some(p), Some(s)) => {
                let name = p
                    .strip_prefix('{')
                    .and_then(|r| r.strip_suffix('}'))
                    .or_else(|| p.strip_prefix(':'));
                if let Some(name) = name {
                    if name == param {
                        return Some(s.to_string());
                    }
                }
            }
            _ => return None,
        }
    }
}

/// Which workspace is this request targeting? Single source of truth shared
/// by middleware-protected routes and middleware-less handlers
/// (`ResolveWorkspaceIDFromRequest` equivalent).
///
/// Priority:
///  1. task-token binding (X-Actor-Source == "task_token") — authoritative
///  2. `X-Workspace-Slug` header → GetWorkspaceBySlug → UUID
///  3. `?workspace_slug` query → GetWorkspaceBySlug → UUID
///  4. `X-Workspace-ID` header (CLI/daemon compat)
///  5. `?workspace_id` query
///
/// Returns None when no identifier was provided OR a slug didn't resolve.
pub async fn resolve_workspace_id_from_request(
    state: &WorkspaceGuardState,
    req: &Request,
) -> Option<String> {
    // A mat_ task token is bound to exactly one workspace by the token row.
    // Any other workspace identifier on the request is the agent trying to
    // widen its blast radius — ignore it (MUL-2600).
    if header(req, "x-actor-source") == Some("task_token") {
        return header(req, "x-workspace-id").map(str::to_string);
    }
    if let Some(slug) = header(req, "x-workspace-slug") {
        if let Some(ws) = ws_q::get_workspace_by_slug(&state.pool, slug)
            .await
            .ok()
            .flatten()
        {
            return Some(ws.id.to_string());
        }
    }
    if let Some(slug) = query_param(req, "workspace_slug") {
        if let Some(ws) = ws_q::get_workspace_by_slug(&state.pool, slug)
            .await
            .ok()
            .flatten()
        {
            return Some(ws.id.to_string());
        }
    }
    if let Some(id) = header(req, "x-workspace-id") {
        return Some(id.to_string());
    }
    query_param(req, "workspace_id").map(str::to_string)
}

enum ResolveOutcome {
    Found(String),
    NoIdentifier,
    NotFound,
}

/// Slug-first resolver used by the guard middleware
/// (`resolveWorkspaceUUID` equivalent). Note: query precedes header here —
/// the inverse of `resolve_workspace_id_from_request`, faithfully preserved.
async fn resolve_workspace_uuid(state: &WorkspaceGuardState, req: &Request) -> ResolveOutcome {
    // Task-token-authenticated requests must operate on the token's bound
    // workspace; nothing on the wire can override it (MUL-2600).
    if header(req, "x-actor-source") == Some("task_token") {
        return match header(req, "x-workspace-id") {
            Some(id) if !id.is_empty() => ResolveOutcome::Found(id.to_string()),
            _ => ResolveOutcome::NotFound,
        };
    }
    // Slug path (preferred — frontend sends this after the URL refactor).
    if let Some(slug) = query_param(req, "workspace_slug") {
        if let Some(ws) = ws_q::get_workspace_by_slug(&state.pool, slug)
            .await
            .ok()
            .flatten()
        {
            return ResolveOutcome::Found(ws.id.to_string());
        }
        return ResolveOutcome::NotFound;
    }
    if let Some(slug) = header(req, "x-workspace-slug") {
        if let Some(ws) = ws_q::get_workspace_by_slug(&state.pool, slug)
            .await
            .ok()
            .flatten()
        {
            return ResolveOutcome::Found(ws.id.to_string());
        }
        return ResolveOutcome::NotFound;
    }
    // UUID fallback (CLI, daemon, legacy clients).
    if let Some(id) = query_param(req, "workspace_id") {
        return ResolveOutcome::Found(id.to_string());
    }
    if let Some(id) = header(req, "x-workspace-id") {
        return ResolveOutcome::Found(id.to_string());
    }
    ResolveOutcome::NoIdentifier
}

/// Unified workspace guard — all four Go variants. Use via
/// `axum::middleware::from_fn_with_state(guard_state, require_workspace)`.
pub async fn require_workspace(
    State(state): State<WorkspaceGuardState>,
    mut req: Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    let resolution = if let Some(param) = state.url_param {
        url_param(&req, param)
            .map(ResolveOutcome::Found)
            .unwrap_or(ResolveOutcome::NoIdentifier)
    } else {
        resolve_workspace_uuid(&state, &req).await
    };

    let workspace_id = match resolution {
        ResolveOutcome::Found(id) => id,
        ResolveOutcome::NotFound => {
            return Err((StatusCode::NOT_FOUND, r#"{"error":"workspace not found"}"#));
        }
        ResolveOutcome::NoIdentifier => {
            return Err((
                StatusCode::BAD_REQUEST,
                r#"{"error":"workspace_id or workspace_slug is required"}"#,
            ));
        }
    };

    // Final task-token binding catch-all: even when the workspace came from a
    // URL parameter, the agent must not operate outside its token-bound
    // workspace (MUL-2600).
    if header(&req, "x-actor-source") == Some("task_token") {
        let bound = header(&req, "x-workspace-id").unwrap_or("");
        if bound.is_empty() || workspace_id != bound {
            return Err((
                StatusCode::FORBIDDEN,
                r#"{"error":"task token is bound to a different workspace"}"#,
            ));
        }
    }

    let Some(user_id) = header(&req, "x-user-id") else {
        return Err((
            StatusCode::UNAUTHORIZED,
            r#"{"error":"user not authenticated"}"#,
        ));
    };
    let Ok(user_uuid) = Uuid::parse_str(user_id) else {
        return Err((
            StatusCode::UNAUTHORIZED,
            r#"{"error":"user not authenticated"}"#,
        ));
    };
    let Ok(ws_uuid) = Uuid::parse_str(&workspace_id) else {
        return Err((
            StatusCode::BAD_REQUEST,
            r#"{"error":"invalid workspace_id"}"#,
        ));
    };

    let Some(member) = member_q::get_member_by_user_and_workspace(&state.pool, user_uuid, ws_uuid)
        .await
        .ok()
        .flatten()
    else {
        return Err((StatusCode::NOT_FOUND, r#"{"error":"workspace not found"}"#));
    };

    if !state.roles.is_empty() && !state.roles.contains(&member.role) {
        return Err((
            StatusCode::FORBIDDEN,
            r#"{"error":"insufficient permissions"}"#,
        ));
    }

    req.extensions_mut().insert(WorkspaceContext {
        workspace_id,
        member,
    });
    Ok(next.run(req).await)
}
