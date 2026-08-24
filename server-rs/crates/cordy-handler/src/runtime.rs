//! Runtime collection and editable metadata handlers.

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::{Json, Router};
use cordy_db::models::{AgentRuntime, Member};
use cordy_db::queries::runtime;
use cordy_middleware::workspace::WorkspaceContext;
use cordy_protocol::EVENT_DAEMON_REGISTER;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const MAX_CUSTOM_NAME_CHARS: usize = 100;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/runtimes", get(list))
        .route("/api/runtimes/", get(list))
        .route("/api/runtimes/{runtime_id}", patch(update))
        .route("/api/runtimes/{runtime_id}/", patch(update))
}

#[derive(Debug, Serialize)]
struct RuntimeResponse {
    id: String,
    workspace_id: String,
    daemon_id: Option<String>,
    name: String,
    custom_name: Option<String>,
    runtime_mode: String,
    provider: String,
    launch_header: &'static str,
    status: String,
    device_info: String,
    metadata: Value,
    owner_id: Option<String>,
    visibility: String,
    profile_id: Option<String>,
    last_seen_at: Option<String>,
    created_at: String,
    updated_at: String,
}

impl From<AgentRuntime> for RuntimeResponse {
    fn from(runtime: AgentRuntime) -> Self {
        Self {
            id: runtime.id.to_string(),
            workspace_id: runtime.workspace_id.to_string(),
            daemon_id: runtime.daemon_id,
            name: runtime.name,
            custom_name: runtime.custom_name,
            runtime_mode: runtime.runtime_mode,
            launch_header: crate::daemon::launch_header(&runtime.provider),
            provider: runtime.provider,
            status: runtime.status,
            device_info: runtime.device_info,
            metadata: if runtime.metadata.is_null() {
                json!({})
            } else {
                runtime.metadata
            },
            owner_id: runtime.owner_id.map(|id| id.to_string()),
            visibility: runtime.visibility,
            profile_id: runtime.profile_id.map(|id| id.to_string()),
            last_seen_at: runtime.last_seen_at.map(crate::timefmt::rfc3339),
            created_at: crate::timefmt::rfc3339(runtime.created_at),
            updated_at: crate::timefmt::rfc3339(runtime.updated_at),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ListParams {
    owner: Option<String>,
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<ListParams>,
) -> Response {
    let workspace_id = match Uuid::parse_str(&context.workspace_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace_id"),
    };
    let result = if params.owner.as_deref() == Some("me") {
        runtime::list_agent_runtimes_by_owner(&state.pool, workspace_id, context.member.user_id)
            .await
    } else {
        runtime::list_agent_runtimes(&state.pool, workspace_id).await
    };
    match result {
        Ok(runtimes) => Json(
            runtimes
                .into_iter()
                .map(RuntimeResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::error!(%error, "failed to list runtimes");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list runtimes")
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct UpdateRequest {
    visibility: Option<String>,
    custom_name: Option<String>,
    #[serde(deserialize_with = "null_default")]
    apply_to_machine: bool,
}

fn decode_update(body: &[u8]) -> Result<UpdateRequest, ()> {
    let mut decoder = serde_json::Deserializer::from_slice(body);
    match Value::deserialize(&mut decoder).map_err(|_| ())? {
        Value::Null => Ok(UpdateRequest::default()),
        value => serde_json::from_value(value).map_err(|_| ()),
    }
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn can_edit(member: &Member, runtime: &AgentRuntime) -> bool {
    matches!(member.role.as_str(), "owner" | "admin") || runtime.owner_id == Some(member.user_id)
}

fn can_set_visibility(member: &Member, runtime: &AgentRuntime) -> bool {
    runtime.owner_id == Some(member.user_id)
}

async fn update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    body: Bytes,
) -> Response {
    let runtime_id = match Uuid::parse_str(&raw_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"),
    };
    let mut found = match runtime::get_agent_runtime(&state.pool, runtime_id).await {
        Ok(Some(found)) => found,
        Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "runtime not found"),
    };
    if found.workspace_id != context.member.workspace_id {
        return error_response(StatusCode::NOT_FOUND, "runtime not found");
    }
    if !can_edit(&context.member, &found) {
        return error_response(StatusCode::FORBIDDEN, "you can only edit your own runtimes");
    }

    let request = match decode_update(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid JSON body"),
    };

    let mut visibility_change = None;
    if let Some(visibility) = request.visibility.as_deref() {
        if !matches!(visibility, "private" | "public") {
            return error_response(
                StatusCode::BAD_REQUEST,
                "visibility must be 'private' or 'public'",
            );
        }
        if visibility != found.visibility {
            if !can_set_visibility(&context.member, &found) {
                return error_response(
                    StatusCode::FORBIDDEN,
                    "only the runtime owner can change its visibility",
                );
            }
            visibility_change = Some(visibility);
        }
    }

    let custom_name = request.custom_name.as_deref().map(str::trim);
    if custom_name.is_some_and(|name| name.chars().count() > MAX_CUSTOM_NAME_CHARS) {
        return error_response(StatusCode::BAD_REQUEST, "custom name is too long");
    }

    let mut changed = false;
    if let Some(visibility) = visibility_change {
        found = match runtime::update_agent_runtime_visibility(&state.pool, visibility, found.id)
            .await
        {
            Ok(Some(updated)) => updated,
            Ok(None) | Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to update runtime",
                )
            }
        };
        changed = true;
    }

    if let Some(name) = custom_name {
        let stored = (!name.is_empty()).then_some(name);
        if request.apply_to_machine && found.daemon_id.is_some() {
            let owner_filter = (!matches!(context.member.role.as_str(), "owner" | "admin"))
                .then_some(context.member.user_id);
            let rows = match runtime::update_agent_runtime_custom_name_by_daemon(
                &state.pool,
                stored,
                found.workspace_id,
                found.daemon_id.as_deref(),
                owner_filter,
            )
            .await
            {
                Ok(rows) => rows,
                Err(error) => {
                    tracing::error!(%error, runtime_id = %found.id, "failed to rename runtime machine");
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to update runtime",
                    );
                }
            };
            if let Some(updated) = rows.into_iter().find(|row| row.id == found.id) {
                found = updated;
            }
        } else {
            found = match runtime::update_agent_runtime_custom_name(&state.pool, stored, found.id)
                .await
            {
                Ok(Some(updated)) => updated,
                Ok(None) | Err(_) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to update runtime",
                    )
                }
            };
        }
        changed = true;
    }

    if changed {
        state.bus.publish(&cordy_events::Event {
            event_type: EVENT_DAEMON_REGISTER.into(),
            workspace_id: found.workspace_id.to_string(),
            actor_type: "member".into(),
            actor_id: context.member.user_id.to_string(),
            payload: json!({ "action": "update" }),
            ..Default::default()
        });
    }

    Json(RuntimeResponse::from(found)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_decoder_matches_go_first_value_and_unknown_field_behavior() {
        let request = decode_update(
            br#"{"custom_name":"  Prod Box  ","timezone":"ignored"} {"custom_name":"later"}"#,
        )
        .unwrap();
        assert_eq!(request.custom_name.as_deref(), Some("  Prod Box  "));
    }

    #[test]
    fn update_decoder_accepts_go_null_zero_values() {
        let top_level = decode_update(b"null").unwrap();
        assert!(top_level.visibility.is_none());
        assert!(top_level.custom_name.is_none());
        assert!(!top_level.apply_to_machine);

        let null_bool = decode_update(br#"{"apply_to_machine":null}"#).unwrap();
        assert!(!null_bool.apply_to_machine);
    }

    #[test]
    fn custom_name_limit_counts_unicode_codepoints() {
        assert_eq!("机".repeat(MAX_CUSTOM_NAME_CHARS).chars().count(), 100);
        assert_eq!("机".repeat(MAX_CUSTOM_NAME_CHARS + 1).chars().count(), 101);
    }
}
