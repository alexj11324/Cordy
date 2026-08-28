//! Per-user issue view-bar preferences.

use axum::body::Bytes;
use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use patchbay_db::models::IssueViewPreference;
use patchbay_db::queries::{issue_view_preference, project};
use patchbay_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const BODY_LIMIT: usize = 128 * 1024;

pub fn router() -> Router<HandlerState> {
    Router::new().route(
        "/api/issue-view-preferences",
        get(get_preference).put(put_preference),
    )
}

#[derive(Debug, Default, Deserialize)]
struct GetParams {
    #[serde(default)]
    scope_type: String,
    scope_id: Option<String>,
}

#[derive(Debug, Default)]
enum PrefsInput {
    #[default]
    Missing,
    Present(Value),
}

fn deserialize_present<'de, D>(deserializer: D) -> Result<PrefsInput, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(PrefsInput::Present)
}

fn deserialize_null_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Option::unwrap_or_default)
}

#[derive(Debug, Default, Deserialize)]
struct PutRequest {
    #[serde(default, deserialize_with = "deserialize_null_string")]
    scope_type: String,
    scope_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present")]
    prefs: PrefsInput,
}

#[derive(Debug, Serialize)]
struct PreferenceResponse {
    scope_type: String,
    scope_id: String,
    prefs: Value,
    updated_at: String,
}

impl From<IssueViewPreference> for PreferenceResponse {
    fn from(value: IssueViewPreference) -> Self {
        Self {
            scope_type: value.scope_type,
            scope_id: value.scope_id.to_string(),
            prefs: value.prefs,
            updated_at: crate::timefmt::rfc3339(value.updated_at),
        }
    }
}

async fn resolve_scope(
    state: &HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    scope_type: &str,
    raw_scope_id: Option<&str>,
) -> Result<Uuid, Response> {
    match scope_type {
        "workspace" => Ok(workspace_id),
        "my" => Ok(user_id),
        "project" => {
            let Some(raw_scope_id) = raw_scope_id.filter(|value| !value.is_empty()) else {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "scope_id is required for project scope",
                ));
            };
            let scope_id = Uuid::parse_str(raw_scope_id)
                .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid scope_id"))?;
            match project::get_project_in_workspace(&state.pool, scope_id, workspace_id).await {
                Ok(Some(_)) => Ok(scope_id),
                Ok(None) | Err(_) => {
                    Err(error_response(StatusCode::NOT_FOUND, "project not found"))
                }
            }
        }
        _ => Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid scope_type",
        )),
    }
}

fn request_ids(context: &WorkspaceContext) -> Result<(Uuid, Uuid), Response> {
    let workspace_id = Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace id"))?;
    Ok((workspace_id, context.member.user_id))
}

async fn get_preference(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<GetParams>,
) -> Response {
    let (workspace_id, user_id) = match request_ids(&context) {
        Ok(ids) => ids,
        Err(response) => return response,
    };
    let scope_id = match resolve_scope(
        &state,
        workspace_id,
        user_id,
        &params.scope_type,
        params.scope_id.as_deref(),
    )
    .await
    {
        Ok(scope_id) => scope_id,
        Err(response) => return response,
    };

    match issue_view_preference::get_issue_view_preference(
        &state.pool,
        workspace_id,
        user_id,
        &params.scope_type,
        scope_id,
    )
    .await
    {
        Ok(Some(preference)) => Json(PreferenceResponse::from(preference)).into_response(),
        Ok(None) => Json(PreferenceResponse {
            scope_type: params.scope_type,
            scope_id: scope_id.to_string(),
            prefs: json!({}),
            updated_at: String::new(),
        })
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to load issue view preference");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load preference",
            )
        }
    }
}

async fn put_preference(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    body: Bytes,
) -> Response {
    let (workspace_id, user_id) = match request_ids(&context) {
        Ok(ids) => ids,
        Err(response) => return response,
    };
    if body.len() > BODY_LIMIT {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    }
    let request: PutRequest = match decode_json_body(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let scope_id = match resolve_scope(
        &state,
        workspace_id,
        user_id,
        &request.scope_type,
        request.scope_id.as_deref(),
    )
    .await
    {
        Ok(scope_id) => scope_id,
        Err(response) => return response,
    };
    let prefs = match request.prefs {
        PrefsInput::Missing => json!({}),
        PrefsInput::Present(value) if value.is_object() => value,
        PrefsInput::Present(_) => {
            return error_response(StatusCode::BAD_REQUEST, "prefs must be a JSON object")
        }
    };

    match issue_view_preference::upsert_issue_view_preference(
        &state.pool,
        workspace_id,
        user_id,
        &request.scope_type,
        scope_id,
        &prefs,
    )
    .await
    {
        Ok(Some(preference)) => Json(PreferenceResponse::from(preference)).into_response(),
        Ok(None) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to save preference",
        ),
        Err(error) => {
            tracing::warn!(%error, "failed to save issue view preference");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to save preference",
            )
        }
    }
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
    fn put_decoder_preserves_missing_and_explicit_null_prefs() {
        let missing: PutRequest = decode_json_body(br#"{"scope_type":"workspace"}"#).unwrap();
        assert!(matches!(missing.prefs, PrefsInput::Missing));

        let explicit_null: PutRequest =
            decode_json_body(br#"{"scope_type":"workspace","prefs":null}"#).unwrap();
        assert!(matches!(
            explicit_null.prefs,
            PrefsInput::Present(Value::Null)
        ));

        let null_scope: PutRequest = decode_json_body(br#"{"scope_type":null}"#).unwrap();
        assert!(null_scope.scope_type.is_empty());
    }

    #[test]
    fn go_decoder_contract_accepts_first_value_and_unknown_fields() {
        let request: PutRequest =
            decode_json_body(br#"{"scope_type":"workspace","prefs":{},"future":true} trailing"#)
                .unwrap();
        assert_eq!(request.scope_type, "workspace");
        assert!(matches!(request.prefs, PrefsInput::Present(value) if value.is_object()));
    }

    #[test]
    fn missing_row_response_matches_go_shape() {
        let response = PreferenceResponse {
            scope_type: "workspace".into(),
            scope_id: "018f946a-1234-7890-abcd-1234567890ab".into(),
            prefs: json!({}),
            updated_at: String::new(),
        };
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "scope_type": "workspace",
                "scope_id": "018f946a-1234-7890-abcd-1234567890ab",
                "prefs": {},
                "updated_at": ""
            })
        );
    }
}
