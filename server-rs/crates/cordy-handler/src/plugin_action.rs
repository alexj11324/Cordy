//! Plugin Action API — port of `server/internal/handler/plugin_action.go`.
//!
//! A surface authenticates with the signed-in user's session. A plugin server
//! authenticates with an install or callback bearer token. In both cases the
//! installation, granted scope, workspace membership, and optional callback
//! issue boundary are checked before a resource is touched.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::SecondsFormat;
use cordy_db::models::{Comment, Issue, Member};
use cordy_db::queries::{comment, issue as issue_q, member, user, workspace};
use cordy_plugincontract::{
    SCOPE_COMMENTS_READ, SCOPE_COMMENTS_WRITE, SCOPE_ISSUES_READ, SCOPE_ISSUES_WRITE,
    SCOPE_STORAGE_USER, SCOPE_STORAGE_WORKSPACE, TRIGGER_MANUAL, TRIGGER_UI,
};
use cordy_service::plugin::{find_hook, PluginError, PluginErrorKind};
use cordy_service::plugin_action::{
    authorize_plugin_action, build_plugin_context, has_scope, PluginActionCaller,
};
use cordy_service::plugin_hook::{invoke_hook, HookInvocation};
use cordy_service::plugin_storage::{
    delete_storage_value, get_storage_value, list_storage_keys, resolve_storage_scope,
    set_storage_value, PLUGIN_STORAGE_USER,
};
use cordy_service::plugin_token::{authenticate_install_token, HookActor, CALLBACK_TOKEN_PREFIX};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::issue::{issue_prefix, IssueResponse};
use crate::state::HandlerState;

const INSTALLATION_HEADER: &str = "x-cordy-plugin-installation";
const MAX_COMMENT_BYTES: usize = 64 * 1024;
const MAX_COMMENTS_PER_READ: i32 = 200;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/v1/plugin/context", get(get_context))
        .route(
            "/api/v1/plugin/hooks/{key}",
            axum::routing::post(invoke_plugin_hook),
        )
        .route(
            "/api/v1/plugin/issues/{id}",
            get(get_issue).patch(patch_issue),
        )
        .route(
            "/api/v1/plugin/issues/{id}/comments",
            get(list_comments).post(create_comment),
        )
        .route("/api/v1/plugin/storage/{scope}", get(list_storage))
        .route(
            "/api/v1/plugin/storage/{scope}/{key}",
            get(get_storage).put(put_storage).delete(delete_storage),
        )
}

#[derive(Deserialize)]
struct InvokeHookRequest {
    trigger: String,
    issue_id: Option<String>,
    input: Option<Value>,
}

async fn invoke_plugin_hook(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Bytes,
) -> Response {
    let (caller, actor) = match caller(&state, &headers, "").await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(user_id) = actor.user_id() else {
        return error_response(
            StatusCode::FORBIDDEN,
            "this endpoint requires a user; the presented token acts as the Plugin itself",
        );
    };
    let request: InvokeHookRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.trigger != TRIGGER_UI && request.trigger != TRIGGER_MANUAL {
        return error_response(StatusCode::BAD_REQUEST, "trigger must be ui or manual");
    }
    let manifest = serde_json::to_vec(&caller.installation.manifest).unwrap_or_default();
    let hook = match find_hook(&manifest, &key) {
        Ok(hook) => hook,
        Err(error) => return plugin_error(&error, "failed to load the hook"),
    };
    let issue_id = if let Some(raw) = request
        .issue_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        match plugin_issue(&state, &caller, raw).await {
            Ok(issue) => Some(issue.id),
            Err(response) => return response,
        }
    } else {
        None
    };
    let invocation = HookInvocation {
        installation: &caller.installation,
        hook: &hook,
        trigger: &request.trigger,
        event_type: "",
        actor: HookActor {
            actor_type: "member".to_string(),
            id: user_id,
        },
        issue_id,
        input: request.input.as_ref(),
    };
    let (result, outcome) = invoke_hook(
        &state.plugins,
        state.callbacks.as_deref(),
        &state.callback_base_url,
        invocation,
        1,
    )
    .await;
    match outcome {
        Ok(()) => Json(result).into_response(),
        Err(error) => plugin_error(&error, "the hook call failed"),
    }
}

#[derive(Clone)]
struct PluginActor {
    member: Option<Member>,
}

impl PluginActor {
    fn actor_type(&self) -> &'static str {
        if self.member.is_some() {
            "member"
        } else {
            "plugin"
        }
    }

    fn user_id(&self) -> Option<Uuid> {
        self.member.as_ref().map(|member| member.user_id)
    }
}

fn plugins_enabled(state: &HandlerState) -> bool {
    state
        .feature_flags
        .as_deref()
        .is_some_and(cordy_service::feature_flags::plugins_v1_enabled)
}

fn plugin_error(error: &PluginError, fallback: &str) -> Response {
    let status = match error.kind {
        PluginErrorKind::Invalid => StatusCode::BAD_REQUEST,
        PluginErrorKind::NotFound => StatusCode::NOT_FOUND,
        PluginErrorKind::Conflict => StatusCode::CONFLICT,
        PluginErrorKind::Forbidden => StatusCode::FORBIDDEN,
        PluginErrorKind::Incompatible => StatusCode::UNPROCESSABLE_ENTITY,
        PluginErrorKind::Quota => StatusCode::INSUFFICIENT_STORAGE,
        _ => StatusCode::BAD_GATEWAY,
    };
    error_response(
        status,
        if error.message.is_empty() {
            fallback
        } else {
            &error.message
        },
    )
}

async fn caller(
    state: &HandlerState,
    headers: &HeaderMap,
    scope: &str,
) -> Result<(PluginActionCaller, PluginActor), Response> {
    if !plugins_enabled(state) {
        return Err(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Plugin management is not enabled",
        ));
    }

    let token = cordy_middleware::plugin_auth::bearer_token(headers);
    if cordy_middleware::plugin_auth::is_plugin_bearer_token(&token) {
        let (installation_id, member_user_id, issue_scope) =
            if token.starts_with(CALLBACK_TOKEN_PREFIX) {
                let Some(callbacks) = state.callbacks.as_ref() else {
                    return Err(error_response(
                        StatusCode::FORBIDDEN,
                        "callback tokens are not enabled",
                    ));
                };
                let grant = callbacks
                    .resolve(&token)
                    .map_err(|error| plugin_error(&error, "failed to authorize the Plugin call"))?;
                let member_user_id = (grant.actor.actor_type == "member").then_some(grant.actor.id);
                (grant.installation_id, member_user_id, grant.issue_id)
            } else {
                let installation = authenticate_install_token(&state.pool, &token)
                    .await
                    .map_err(|error| plugin_error(&error, "failed to authorize the Plugin call"))?;
                (installation.id, None, None)
            };

        let mut authorized = authorize_plugin_action(
            &state.pool,
            &installation_id.to_string(),
            member_user_id.unwrap_or_default(),
            scope,
        )
        .await
        .map_err(|error| plugin_error(&error, "failed to authorize the Plugin call"))?;
        authorized.issue_scope = issue_scope;
        let member = if let Some(user_id) = member_user_id {
            member::get_member_by_user_and_workspace(&state.pool, user_id, authorized.workspace_id)
                .await
                .ok()
                .flatten()
                .ok_or_else(|| {
                    error_response(
                        StatusCode::FORBIDDEN,
                        "the user this callback acts for is no longer a member",
                    )
                })?
                .into()
        } else {
            None
        };
        return Ok((authorized, PluginActor { member }));
    }

    let user_id = header_uuid(headers, "x-user-id", "user_id")?;
    let installation_id = headers
        .get(INSTALLATION_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let authorized = authorize_plugin_action(&state.pool, installation_id, user_id, scope)
        .await
        .map_err(|error| plugin_error(&error, "failed to authorize the Plugin call"))?;
    let member =
        member::get_member_by_user_and_workspace(&state.pool, user_id, authorized.workspace_id)
            .await
            .ok()
            .flatten()
            .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "workspace not found"))?;
    Ok((
        authorized,
        PluginActor {
            member: Some(member),
        },
    ))
}

fn header_uuid(headers: &HeaderMap, name: &str, field: &str) -> Result<Uuid, Response> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, &format!("invalid {field}")))
}

async fn plugin_issue(
    state: &HandlerState,
    caller: &PluginActionCaller,
    raw: &str,
) -> Result<Issue, Response> {
    if raw.is_empty() {
        return Err(error_response(StatusCode::NOT_FOUND, "issue not found"));
    }
    let result = if let Ok(id) = Uuid::parse_str(raw) {
        issue_q::get_issue_in_workspace(&state.pool, id, caller.workspace_id).await
    } else {
        let Some((prefix, number)) = raw.rsplit_once('-') else {
            return Err(error_response(StatusCode::NOT_FOUND, "issue not found"));
        };
        let expected = issue_prefix(state, caller.workspace_id).await;
        let Ok(number) = number.parse::<i32>() else {
            return Err(error_response(StatusCode::NOT_FOUND, "issue not found"));
        };
        if !prefix.eq_ignore_ascii_case(&expected) {
            return Err(error_response(StatusCode::NOT_FOUND, "issue not found"));
        }
        issue_q::get_issue_by_number(&state.pool, caller.workspace_id, number).await
    };
    let issue = result
        .ok()
        .flatten()
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "issue not found"))?;
    if caller.issue_scope.is_some_and(|scope| scope != issue.id) {
        return Err(error_response(StatusCode::NOT_FOUND, "issue not found"));
    }
    Ok(issue)
}

async fn issue_json(state: &HandlerState, issue: &Issue) -> Value {
    serde_json::to_value(IssueResponse::from_issue(
        issue,
        &issue_prefix(state, issue.workspace_id).await,
    ))
    .unwrap_or_else(|_| json!({}))
}

#[derive(Deserialize, Default)]
struct ContextQuery {
    issue_id: Option<String>,
}

async fn get_context(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Query(query): Query<ContextQuery>,
) -> Response {
    let (caller, actor) = match caller(&state, &headers, "").await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(workspace) = workspace::get_workspace(&state.pool, caller.workspace_id)
        .await
        .ok()
        .flatten()
    else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to load the workspace",
        );
    };
    let loaded_user = if let Some(user_id) = actor.user_id() {
        match user::get_user(&state.pool, user_id).await {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "failed to load plugin context user");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load the user",
                );
            }
        }
    } else {
        None
    };
    let loaded_issue = if let Some(issue_id) = query.issue_id.as_deref().filter(|id| !id.is_empty())
    {
        match plugin_issue(&state, &caller, issue_id).await {
            Ok(issue) => Some(issue),
            Err(response) => return response,
        }
    } else {
        None
    };
    Json(build_plugin_context(
        &caller,
        &workspace,
        loaded_user.as_ref(),
        loaded_issue.as_ref(),
    ))
    .into_response()
}

async fn get_issue(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let (caller, _) = match caller(&state, &headers, SCOPE_ISSUES_READ).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match plugin_issue(&state, &caller, &id).await {
        Ok(issue) => Json(issue_json(&state, &issue).await).into_response(),
        Err(response) => response,
    }
}

#[derive(Deserialize)]
struct PatchIssueRequest {
    title: Option<String>,
    description: Option<String>,
}

async fn patch_issue(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let (caller, _) = match caller(&state, &headers, SCOPE_ISSUES_WRITE).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let issue = match plugin_issue(&state, &caller, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let mut request: PatchIssueRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.title.is_none() && request.description.is_none() {
        return error_response(StatusCode::BAD_REQUEST, "title or description is required");
    }
    request.title = request.title.map(|value| value.replace('\0', ""));
    request.description = request.description.map(|value| value.replace('\0', ""));
    if request.title.as_ref().is_some_and(String::is_empty) {
        return error_response(StatusCode::BAD_REQUEST, "title must not be empty");
    }
    let updated = sqlx::query(
        r#"UPDATE issue SET
title=COALESCE($2,title),
description=COALESCE($3,description),
revision=revision + (ROW(title,description) IS DISTINCT FROM ROW(COALESCE($2,title),COALESCE($3,description)))::integer,
updated_at=CASE WHEN ROW(title,description) IS DISTINCT FROM ROW(COALESCE($2,title),COALESCE($3,description)) THEN now() ELSE updated_at END,
last_activity_at=CASE WHEN ROW(title,description) IS DISTINCT FROM ROW(COALESCE($2,title),COALESCE($3,description)) THEN GREATEST(COALESCE(last_activity_at,updated_at),now()) ELSE last_activity_at END
WHERE id=$1 AND workspace_id=$4"#,
    )
    .bind(issue.id)
    .bind(request.title.as_deref())
    .bind(request.description.as_deref())
    .bind(caller.workspace_id)
    .execute(&state.pool)
    .await;
    if let Err(error) = updated {
        tracing::warn!(%error, "failed to update plugin issue");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update the issue",
        );
    }
    match issue_q::get_issue_in_workspace(&state.pool, issue.id, caller.workspace_id).await {
        Ok(Some(issue)) => Json(issue_json(&state, &issue).await).into_response(),
        _ => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update the issue",
        ),
    }
}

#[derive(Serialize)]
struct CommentResponse {
    id: String,
    author_type: String,
    author_id: String,
    content: String,
    #[serde(rename = "type")]
    type_: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<String>,
    created_at: String,
}

impl From<&Comment> for CommentResponse {
    fn from(comment: &Comment) -> Self {
        Self {
            id: comment.id.to_string(),
            author_type: comment.author_type.clone(),
            author_id: comment.author_id.to_string(),
            content: comment.content.clone(),
            type_: comment.type_.clone(),
            parent_id: comment.parent_id.map(|id| id.to_string()),
            created_at: comment
                .created_at
                .to_rfc3339_opts(SecondsFormat::Secs, true),
        }
    }
}

async fn list_comments(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let (caller, _) = match caller(&state, &headers, SCOPE_COMMENTS_READ).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let issue = match plugin_issue(&state, &caller, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    match comment::list_comments_for_issue(
        &state.pool,
        issue.id,
        caller.workspace_id,
        MAX_COMMENTS_PER_READ,
    )
    .await
    {
        Ok(comments) => Json(json!({
            "comments": comments.iter().map(CommentResponse::from).collect::<Vec<_>>()
        }))
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to list plugin comments");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list comments")
        }
    }
}

#[derive(Deserialize)]
struct CreateCommentRequest {
    content: String,
    parent_id: Option<String>,
}

async fn create_comment(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let (caller, actor) = match caller(&state, &headers, SCOPE_COMMENTS_WRITE).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let issue = match plugin_issue(&state, &caller, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let request: CreateCommentRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let content = request.content.replace('\0', "");
    if content.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "content is required");
    }
    if content.len() > MAX_COMMENT_BYTES {
        return error_response(StatusCode::BAD_REQUEST, "content is too long");
    }
    let mut parent = None;
    if let Some(raw) = request
        .parent_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        let Ok(parent_id) = Uuid::parse_str(raw) else {
            return error_response(StatusCode::BAD_REQUEST, "invalid parent_id");
        };
        parent = comment::get_comment_in_workspace(&state.pool, parent_id, caller.workspace_id)
            .await
            .ok()
            .flatten();
        if parent
            .as_ref()
            .is_none_or(|value| value.issue_id != issue.id)
        {
            return error_response(StatusCode::BAD_REQUEST, "invalid parent comment");
        }
    }
    let author_type = actor.actor_type();
    let author_id = actor.user_id().unwrap_or(caller.installation.id);
    let created = comment::create_comment(
        &state.pool,
        issue.id,
        caller.workspace_id,
        author_type,
        author_id,
        &content,
        "comment",
        parent.as_ref().map(|value| value.id),
        None,
        None,
        Some(caller.installation.id),
        cordy_db::dbid::new_v7(),
    )
    .await;
    let created = match created {
        Ok(Some(created)) => created,
        _ => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create the comment",
            )
        }
    };
    let payload = json!({
        "id": created.id.map(|id| id.to_string()).unwrap_or_default(),
        "author_type": created.author_type,
        "author_id": created.author_id.map(|id| id.to_string()).unwrap_or_default(),
        "content": created.content,
        "type": created.type_,
        "parent_id": created.parent_id.map(|id| id.to_string()),
        "created_at": created.created_at.map(|time| time.to_rfc3339_opts(SecondsFormat::Secs, true)).unwrap_or_default(),
    });
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_COMMENT_CREATED.to_string(),
        workspace_id: caller.workspace_id.to_string(),
        actor_type: author_type.to_string(),
        actor_id: author_id.to_string(),
        payload: json!({
            "comment": payload.clone(),
            "issue_title": issue.title,
            "issue_assignee_type": issue.assignee_type,
            "issue_assignee_id": issue.assignee_id.map(|id| id.to_string()),
            "issue_status": issue.status,
            "issue_revision": created.issue_revision,
        }),
        ..Default::default()
    });
    state
        .tasks
        .auto_unresolve_thread_on_reply(
            parent.as_ref(),
            &caller.workspace_id.to_string(),
            author_type,
            &author_id.to_string(),
        )
        .await;
    (StatusCode::CREATED, Json(payload)).into_response()
}

fn storage_scope(
    caller: &PluginActionCaller,
    actor: &PluginActor,
    scope_type: &str,
) -> Result<Uuid, Response> {
    let required = if scope_type == PLUGIN_STORAGE_USER {
        SCOPE_STORAGE_USER
    } else {
        SCOPE_STORAGE_WORKSPACE
    };
    if !has_scope(&caller.scopes, required) {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            &format!("this Plugin was not granted the {required} scope"),
        ));
    }
    let user_id = if scope_type == PLUGIN_STORAGE_USER {
        actor.user_id().ok_or_else(|| {
            error_response(
                StatusCode::FORBIDDEN,
                "this endpoint requires a user; the presented token acts as the Plugin itself",
            )
        })?
    } else {
        Uuid::nil()
    };
    resolve_storage_scope(scope_type, caller.workspace_id, user_id)
        .map_err(|error| plugin_error(&error, "invalid storage scope"))
}

async fn list_storage(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Path(scope): Path<String>,
) -> Response {
    let (caller, actor) = match caller(&state, &headers, "").await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let scope_id = match storage_scope(&caller, &actor, &scope) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match list_storage_keys(&state.pool, caller.installation.id, &scope, scope_id).await {
        Ok(keys) => Json(json!({ "keys": keys })).into_response(),
        Err(error) => plugin_error(&error, "failed to list storage"),
    }
}

async fn get_storage(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Path((scope, key)): Path<(String, String)>,
) -> Response {
    let (caller, actor) = match caller(&state, &headers, "").await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let scope_id = match storage_scope(&caller, &actor, &scope) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match get_storage_value(&state.pool, caller.installation.id, &scope, scope_id, &key).await {
        Ok(value) => Json(json!({ "value": value })).into_response(),
        Err(error) => plugin_error(&error, "failed to read storage"),
    }
}

#[derive(Deserialize)]
struct PutStorageRequest {
    value: String,
}

async fn put_storage(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Path((scope, key)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let (caller, actor) = match caller(&state, &headers, "").await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let scope_id = match storage_scope(&caller, &actor, &scope) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let request: PutStorageRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    match set_storage_value(
        &state.pool,
        caller.installation.id,
        &scope,
        scope_id,
        &key,
        &request.value,
    )
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => plugin_error(&error, "failed to write storage"),
    }
}

async fn delete_storage(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Path((scope, key)): Path<(String, String)>,
) -> Response {
    let (caller, actor) = match caller(&state, &headers, "").await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let scope_id = match storage_scope(&caller, &actor, &scope) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match delete_storage_value(&state.pool, caller.installation.id, &scope, scope_id, &key).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => plugin_error(&error, "failed to delete storage"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_error_statuses_match_go_contract() {
        for (kind, status) in [
            (PluginErrorKind::Invalid, StatusCode::BAD_REQUEST),
            (PluginErrorKind::NotFound, StatusCode::NOT_FOUND),
            (PluginErrorKind::Conflict, StatusCode::CONFLICT),
            (PluginErrorKind::Forbidden, StatusCode::FORBIDDEN),
            (
                PluginErrorKind::Incompatible,
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (PluginErrorKind::Quota, StatusCode::INSUFFICIENT_STORAGE),
        ] {
            let error = PluginError::new(kind, "mapped");
            assert_eq!(plugin_error(&error, "fallback").status(), status);
        }
    }

    #[test]
    fn plugin_actor_never_invents_a_member() {
        let actor = PluginActor { member: None };
        assert_eq!(actor.actor_type(), "plugin");
        assert_eq!(actor.user_id(), None);
    }
}
