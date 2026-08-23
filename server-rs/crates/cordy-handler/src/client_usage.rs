//! Authenticated daily client-usage reporting.

use std::collections::HashMap;
use std::sync::LazyLock;

use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{middleware, Router};
use cordy_db::queries::{client_usage, member, workspace};
use cordy_middleware::client::ClientMetadata;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const CLIENT_USAGE_BODY_LIMIT: usize = 16 * 1024;

static PROVIDER_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9][a-z0-9_-]{0,63}$").expect("provider-name regex is valid")
});
static CLIENT_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\x20-\x7e]{1,64}$").expect("client-version regex is valid"));

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/client-usage", post(upsert_client_usage))
        .route_layer(middleware::from_fn(require_human_actor))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientUsageRequest {
    #[serde(default)]
    install_id: String,
    runtime: Option<ClientUsageRuntimeProbe>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientUsageRuntimeProbe {
    #[serde(default)]
    probe_result: String,
    runtime_count: Option<i32>,
    provider_summary: Option<HashMap<String, i64>>,
    online_count: Option<i32>,
    offline_count: Option<i32>,
}

#[derive(Debug, Default, PartialEq)]
struct ValidatedRuntimeProbe {
    result: Option<String>,
    runtime_count: Option<i32>,
    provider_summary: Option<Value>,
    online_count: Option<i32>,
    offline_count: Option<i32>,
}

#[derive(Debug)]
enum WorkspaceLocator {
    TaskBound(String),
    Candidates {
        header_slug: Option<String>,
        query_slug: Option<String>,
        id: Option<String>,
    },
}

async fn require_human_actor(request: Request, next: Next) -> Response {
    if is_machine_actor_source(
        request
            .headers()
            .get("x-actor-source")
            .and_then(|value| value.to_str().ok()),
    ) {
        return error_response(
            StatusCode::FORBIDDEN,
            "this endpoint is only available to human actors",
        );
    }
    next.run(request).await
}

fn is_machine_actor_source(source: Option<&str>) -> bool {
    matches!(source, Some("task_token" | "cloud_pat"))
}

async fn upsert_client_usage(State(state): State<HandlerState>, mut request: Request) -> Response {
    let user_id = match request
        .headers()
        .get("x-user-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
    {
        Some(value) => match Uuid::parse_str(value) {
            Ok(value) => value,
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid user id"),
        },
        None => return error_response(StatusCode::UNAUTHORIZED, "user not authenticated"),
    };
    let metadata = request
        .extensions()
        .get::<ClientMetadata>()
        .cloned()
        .unwrap_or_default();
    let workspace_locator = workspace_locator(&request);
    let body = std::mem::replace(request.body_mut(), Body::empty());
    let body = match to_bytes(body, CLIENT_USAGE_BODY_LIMIT).await {
        Ok(body) => body,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let input = match decode_request(&body) {
        Ok(input) => input,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };
    let install_id = match Uuid::parse_str(input.install_id.trim()) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid install_id"),
    };
    let (client_type, client_version, client_os) = match validate_metadata(&metadata) {
        Ok(metadata) => metadata,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };
    let has_runtime_probe = input.runtime.is_some();
    let runtime = match input.runtime {
        Some(_probe) if client_type != "desktop" => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "runtime data is only accepted from desktop",
            )
        }
        Some(probe) => match validate_runtime(probe) {
            Ok(runtime) => runtime,
            Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
        },
        None => ValidatedRuntimeProbe::default(),
    };

    drop(request);
    let workspace_id = resolve_workspace_id(&state.pool, workspace_locator).await;
    let workspace_id = match workspace_id {
        Some(value) => match Uuid::parse_str(&value) {
            Ok(value) => Some(value),
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace id"),
        },
        None => None,
    };

    if let Some(workspace_id) = workspace_id {
        let mut transaction = match state.pool.begin().await {
            Ok(transaction) => transaction,
            Err(error) => {
                tracing::error!(%error, "failed to begin client usage transaction");
                return record_failed();
            }
        };
        match workspace::lock_workspace_for_chat_session_create(&mut *transaction, workspace_id)
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => return workspace_not_found(),
            Err(error) => {
                tracing::error!(%error, "failed to lock client usage workspace");
                return record_failed();
            }
        }
        match member::get_member_by_user_and_workspace(&mut *transaction, user_id, workspace_id)
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => return workspace_not_found(),
            Err(error) => {
                tracing::error!(%error, "failed to validate client usage workspace");
                return record_failed();
            }
        }
        if let Err(error) = write_usage(
            &mut *transaction,
            user_id,
            &client_type,
            install_id,
            Some(workspace_id),
            &client_version,
            &client_os,
            has_runtime_probe,
            &runtime,
        )
        .await
        {
            tracing::error!(%error, %client_type, "failed to upsert client usage");
            return record_failed();
        }
        if let Err(error) = transaction.commit().await {
            tracing::error!(%error, %client_type, "failed to commit client usage");
            return record_failed();
        }
    } else if let Err(error) = write_usage(
        &state.pool,
        user_id,
        &client_type,
        install_id,
        None,
        &client_version,
        &client_os,
        has_runtime_probe,
        &runtime,
    )
    .await
    {
        tracing::error!(%error, %client_type, "failed to upsert client usage");
        return record_failed();
    }

    StatusCode::NO_CONTENT.into_response()
}

#[allow(clippy::too_many_arguments)]
async fn write_usage<'e, E>(
    executor: E,
    user_id: Uuid,
    client_type: &str,
    install_id: Uuid,
    workspace_id: Option<Uuid>,
    client_version: &str,
    client_os: &str,
    has_runtime_probe: bool,
    runtime: &ValidatedRuntimeProbe,
) -> anyhow::Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres> + Send,
{
    client_usage::upsert_client_usage_daily(
        executor,
        user_id,
        client_type,
        install_id,
        workspace_id,
        client_version,
        client_os,
        has_runtime_probe,
        runtime.result.as_deref(),
        runtime.runtime_count,
        runtime.provider_summary.as_ref(),
        runtime.online_count,
        runtime.offline_count,
    )
    .await?;
    Ok(())
}

fn decode_request(body: &[u8]) -> Result<ClientUsageRequest, &'static str> {
    serde_json::from_slice(body).map_err(|_| "invalid request body")
}

fn workspace_locator(request: &Request) -> Option<WorkspaceLocator> {
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    if header("x-actor-source").as_deref() == Some("task_token") {
        return header("x-workspace-id").map(WorkspaceLocator::TaskBound);
    }
    let query = |key: &str| {
        request.uri().query().and_then(|query| {
            query.split('&').find_map(|pair| {
                let (name, value) = pair.split_once('=')?;
                (name == key && !value.is_empty()).then(|| value.to_string())
            })
        })
    };
    let header_slug = header("x-workspace-slug");
    let query_slug = query("workspace_slug");
    let id = header("x-workspace-id").or_else(|| query("workspace_id"));
    if header_slug.is_none() && query_slug.is_none() && id.is_none() {
        None
    } else {
        Some(WorkspaceLocator::Candidates {
            header_slug,
            query_slug,
            id,
        })
    }
}

async fn resolve_workspace_id(
    pool: &sqlx::PgPool,
    locator: Option<WorkspaceLocator>,
) -> Option<String> {
    match locator {
        Some(WorkspaceLocator::TaskBound(id)) => Some(id),
        Some(WorkspaceLocator::Candidates {
            header_slug,
            query_slug,
            id,
        }) => {
            for slug in [header_slug, query_slug].into_iter().flatten() {
                if let Some(found) = workspace::get_workspace_by_slug(pool, &slug)
                    .await
                    .ok()
                    .flatten()
                {
                    return Some(found.id.to_string());
                }
            }
            id
        }
        None => None,
    }
}

fn validate_metadata(metadata: &ClientMetadata) -> Result<(String, String, String), &'static str> {
    let client_type = metadata.platform.trim().to_ascii_lowercase();
    if client_type != "web" && client_type != "desktop" {
        return Err("client platform must be web or desktop");
    }
    let mut version = metadata.version.trim().to_string();
    if version.is_empty() {
        version = "unknown".to_string();
    }
    if !CLIENT_VERSION.is_match(&version) {
        return Err("invalid client version");
    }
    let client_os = normalize_os(&metadata.os);
    Ok((client_type, version, client_os))
}

fn normalize_os(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "macos" | "windows" | "linux" | "ios" | "android" | "chromeos" => normalized,
        _ => "unknown".to_string(),
    }
}

fn validate_runtime(probe: ClientUsageRuntimeProbe) -> Result<ValidatedRuntimeProbe, &'static str> {
    let result = probe.probe_result.trim().to_ascii_lowercase();
    if result != "success" && result != "error" {
        return Err("runtime probe_result must be success or error");
    }
    if result == "error" {
        if probe.runtime_count.is_some()
            || probe.provider_summary.is_some()
            || probe.online_count.is_some()
            || probe.offline_count.is_some()
        {
            return Err("failed runtime probes must not include counts");
        }
        return Ok(ValidatedRuntimeProbe {
            result: Some(result),
            ..ValidatedRuntimeProbe::default()
        });
    }

    let (Some(runtime_count), Some(provider_summary), Some(online_count), Some(offline_count)) = (
        probe.runtime_count,
        probe.provider_summary,
        probe.online_count,
        probe.offline_count,
    ) else {
        return Err("successful runtime probes require all counts");
    };
    if !(0..=1000).contains(&runtime_count)
        || online_count < 0
        || offline_count < 0
        || online_count.checked_add(offline_count) != Some(runtime_count)
    {
        return Err("invalid runtime counts");
    }
    if provider_summary.len() > 32 {
        return Err("too many runtime providers");
    }
    let mut provider_total = 0_i64;
    for (provider, count) in &provider_summary {
        if !PROVIDER_NAME.is_match(provider) || !(0..=1000).contains(count) {
            return Err("invalid runtime provider summary");
        }
        provider_total += count;
    }
    if provider_total != i64::from(runtime_count) {
        return Err("runtime provider counts do not match runtime_count");
    }
    Ok(ValidatedRuntimeProbe {
        result: Some(result),
        runtime_count: Some(runtime_count),
        provider_summary: Some(
            serde_json::to_value(provider_summary)
                .map_err(|_| "invalid runtime provider summary")?,
        ),
        online_count: Some(online_count),
        offline_count: Some(offline_count),
    })
}

fn workspace_not_found() -> Response {
    error_response(StatusCode::FORBIDDEN, "workspace not found")
}

fn record_failed() -> Response {
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "failed to record client usage",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    async fn stamp_task_actor(mut request: Request, next: Next) -> Response {
        request
            .headers_mut()
            .insert("x-actor-source", HeaderValue::from_static("task_token"));
        next.run(request).await
    }

    fn probe(body: &str) -> ClientUsageRuntimeProbe {
        serde_json::from_str(body).unwrap()
    }

    #[test]
    fn request_decoder_rejects_unknown_fields_and_trailing_values() {
        assert!(decode_request(br#"{"install_id":"x","extra":true}"#).is_err());
        assert!(decode_request(br#"{"install_id":"x"} {"install_id":"y"}"#).is_err());
        assert!(decode_request(
            br#"{"install_id":"x","runtime":{"probe_result":"error","extra":true}}"#
        )
        .is_err());
    }

    #[test]
    fn runtime_validation_matches_go_contract() {
        assert_eq!(
            validate_runtime(probe(r#"{"probe_result":"wat"}"#)),
            Err("runtime probe_result must be success or error")
        );
        assert_eq!(
            validate_runtime(probe(r#"{"probe_result":"error","runtime_count":0}"#)),
            Err("failed runtime probes must not include counts")
        );
        assert_eq!(
            validate_runtime(probe(r#"{"probe_result":"success"}"#)),
            Err("successful runtime probes require all counts")
        );
        assert_eq!(
            validate_runtime(probe(
                r#"{"probe_result":"success","runtime_count":2,"provider_summary":{"codex":2},"online_count":1,"offline_count":1}"#
            )),
            Ok(ValidatedRuntimeProbe {
                result: Some("success".to_string()),
                runtime_count: Some(2),
                provider_summary: Some(serde_json::json!({"codex": 2})),
                online_count: Some(1),
                offline_count: Some(1),
            })
        );
    }

    #[test]
    fn runtime_validation_rejects_inconsistent_counts_and_providers() {
        for (body, expected) in [
            (
                r#"{"probe_result":"success","runtime_count":2,"provider_summary":{"codex":2},"online_count":2,"offline_count":1}"#,
                "invalid runtime counts",
            ),
            (
                r#"{"probe_result":"success","runtime_count":2,"provider_summary":{"Codex":2},"online_count":1,"offline_count":1}"#,
                "invalid runtime provider summary",
            ),
            (
                r#"{"probe_result":"success","runtime_count":2,"provider_summary":{"codex":1},"online_count":1,"offline_count":1}"#,
                "runtime provider counts do not match runtime_count",
            ),
        ] {
            assert_eq!(validate_runtime(probe(body)), Err(expected));
        }
    }

    #[test]
    fn metadata_and_actor_source_are_normalized_strictly() {
        assert_eq!(normalize_os(" MacOS "), "macos");
        assert_eq!(normalize_os("Darwin 24.4"), "unknown");
        assert!(is_machine_actor_source(Some("task_token")));
        assert!(is_machine_actor_source(Some("cloud_pat")));
        assert!(!is_machine_actor_source(Some("future_source")));

        let metadata = ClientMetadata {
            platform: " Desktop ".to_string(),
            version: " ".to_string(),
            os: " Linux ".to_string(),
        };
        assert_eq!(
            validate_metadata(&metadata),
            Ok((
                "desktop".to_string(),
                "unknown".to_string(),
                "linux".to_string()
            ))
        );
    }

    #[test]
    fn workspace_locator_preserves_slug_and_uuid_fallback_priority() {
        let request =
            HttpRequest::get("/api/client-usage?workspace_slug=query-slug&workspace_id=query-id")
                .header("x-workspace-slug", "header-slug")
                .header("x-workspace-id", "header-id")
                .body(Body::empty())
                .unwrap();
        assert!(matches!(
            workspace_locator(&request),
            Some(WorkspaceLocator::Candidates {
                header_slug: Some(header_slug),
                query_slug: Some(query_slug),
                id: Some(id),
            }) if header_slug == "header-slug"
                && query_slug == "query-slug"
                && id == "header-id"
        ));

        let task_request =
            HttpRequest::get("/api/client-usage?workspace_slug=ignored&workspace_id=ignored")
                .header("x-actor-source", "task_token")
                .header("x-workspace-id", "bound-id")
                .header("x-workspace-slug", "ignored")
                .body(Body::empty())
                .unwrap();
        assert!(matches!(
            workspace_locator(&task_request),
            Some(WorkspaceLocator::TaskBound(id)) if id == "bound-id"
        ));
    }

    #[tokio::test]
    async fn route_guard_blocks_known_machine_sources_only() {
        let state = HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        );
        for source in ["task_token", "cloud_pat"] {
            let response = router()
                .with_state(state.clone())
                .oneshot(
                    HttpRequest::post("/api/client-usage")
                        .header("x-actor-source", source)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }

        let response = router()
            .with_state(state)
            .oneshot(
                HttpRequest::post("/api/client-usage")
                    .header("x-actor-source", "future_source")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_outer_layer_stamps_actor_before_route_guard() {
        let state = HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        );
        let response = router()
            .route_layer(middleware::from_fn(stamp_task_actor))
            .with_state(state)
            .oneshot(
                HttpRequest::post("/api/client-usage")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
