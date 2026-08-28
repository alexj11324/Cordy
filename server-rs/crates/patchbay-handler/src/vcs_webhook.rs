//! Public webhook ingress for token-based Git providers.

use std::collections::HashSet;
use std::sync::LazyLock;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use patchbay_db::models::{Issue, VcsConnection, VcsPullRequest};
use patchbay_vcs::{CiStatusEvent, EventKind, PullRequestEvent};
use regex::Regex;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const MAX_BODY_BYTES: usize = 10 << 20;

static IDENTIFIER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b([a-z][a-z0-9]{1,9})-(\d+)\b").expect("issue identifier regex is valid")
});
static CLOSING_IDENTIFIER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)[:\s]+([a-z][a-z0-9]{1,9})-(\d+)\b")
        .expect("closing identifier regex is valid")
});

pub fn router() -> Router<HandlerState> {
    Router::new().route("/api/webhooks/vcs/{connection_id}", post(handle))
}

async fn handle(
    State(state): State<HandlerState>,
    Path(raw_connection_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !state.vcs_integration_enabled {
        return error_response(StatusCode::NOT_FOUND, "unknown connection");
    }
    let Some(secret_box) = state.vcs_secret_box.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "vcs webhooks not configured",
        );
    };
    let connection_id = match Uuid::parse_str(&raw_connection_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid connection id"),
    };
    let body = match read_limited(body, MAX_BODY_BYTES).await {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!(%error, "vcs: read webhook body failed");
            return error_response(StatusCode::BAD_REQUEST, "read body failed");
        }
    };
    let connection =
        match patchbay_db::queries::vcs::get_vcs_connection_by_id(&state.pool, connection_id).await
        {
            Ok(Some(connection)) => connection,
            Ok(None) => return error_response(StatusCode::NOT_FOUND, "unknown connection"),
            Err(error) => {
                tracing::warn!(%error, %connection_id, "vcs: lookup connection failed");
                return error_response(StatusCode::NOT_FOUND, "unknown connection");
            }
        };
    let Some(provider) = patchbay_vcs::for_kind(&connection.provider) else {
        tracing::error!(provider = %connection.provider, "vcs: connection has unknown provider");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "unknown provider");
    };
    let secret = match decrypt_secret(secret_box, &connection.webhook_secret_encrypted) {
        Ok(secret) => secret,
        Err(error) => {
            tracing::error!(%error, %connection_id, "vcs: decrypt webhook secret failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "secret error");
        }
    };
    if !provider.verify_signature(&secret, &headers, &body) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid signature");
    }
    match provider.event_kind(&headers) {
        EventKind::PullRequest => match provider.parse_pull_request(&body) {
            Ok(event) => mirror_pull_request(&state, &connection, event).await,
            Err(error) => {
                tracing::warn!(provider = %connection.provider, %error, "vcs: bad pull_request payload")
            }
        },
        EventKind::CiStatus => match provider.parse_ci_status(&body) {
            Ok(event) => mirror_ci_status(&state, &connection, event).await,
            Err(error) => {
                tracing::warn!(provider = %connection.provider, %error, "vcs: bad status payload")
            }
        },
        EventKind::Other => {}
    }
    StatusCode::ACCEPTED.into_response()
}

async fn read_limited(body: Body, limit: usize) -> Result<Vec<u8>, axum::Error> {
    // io::LimitReader truncates at the cap; preserve that exact Go contract.
    let mut stream = body.into_data_stream();
    let mut output = Vec::new();
    while output.len() < limit {
        let Some(chunk) = stream.next().await else {
            break;
        };
        let chunk = chunk?;
        let remaining = limit - output.len();
        output.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    Ok(output)
}

fn decrypt_secret(
    secret_box: &patchbay_util::secretbox::SecretBox,
    encoded: &str,
) -> anyhow::Result<String> {
    if encoded.is_empty() {
        return Ok(String::new());
    }
    let sealed = STANDARD.decode(encoded)?;
    let plaintext = secret_box.open(&sealed)?;
    Ok(String::from_utf8(plaintext)?)
}

async fn mirror_pull_request(
    state: &HandlerState,
    connection: &VcsConnection,
    event: PullRequestEvent,
) {
    if event.repo_owner.is_empty() || event.repo_name.is_empty() || event.number == 0 {
        tracing::warn!(provider = %connection.provider, "vcs: pull_request missing repo identity");
        return;
    }
    let event_updated_at = event_time(&event.updated_at);
    let pull_request = match patchbay_db::queries::vcs::upsert_vcs_pull_request(
        &state.pool,
        connection.workspace_id,
        connection.id,
        &connection.provider,
        &event.repo_owner,
        &event.repo_name,
        event.number,
        &event.title,
        &event.state,
        &event.html_url,
        Some(event_time(&event.created_at)),
        Some(event_updated_at),
        event.additions,
        event.deletions,
        event.changed_files,
        &event.head_sha,
        nonempty(&event.branch),
        nonempty(&event.author_login),
        nonempty(&event.author_avatar_url),
        optional_event_time(&event.merged_at),
        optional_event_time(&event.closed_at),
    )
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%error, "vcs: upsert pr failed");
            return;
        }
    };
    if pull_request.pr_updated_at > event_updated_at {
        return;
    }

    let prefix =
        match patchbay_db::queries::workspace::get_workspace(&state.pool, connection.workspace_id)
            .await
        {
            Ok(Some(workspace)) => workspace.issue_prefix,
            _ => String::new(),
        };
    let identifiers = extract_identifiers([
        event.title.as_str(),
        event.body.as_str(),
        event.branch.as_str(),
    ]);
    let closing: HashSet<String> =
        extract_closing_identifiers([event.title.as_str(), event.body.as_str()])
            .into_iter()
            .collect();
    let qualifying: HashSet<String> =
        extract_identifiers([event.title.as_str(), event.branch.as_str()])
            .into_iter()
            .chain(closing.iter().cloned())
            .collect();
    let preserve_close_intent =
        !event.terminal() && matches!(event.state.as_str(), "merged" | "closed");
    let mut linked_issue_ids = Vec::new();
    let mut reevaluate = Vec::new();
    for identifier in identifiers {
        let Some(issue) = lookup_issue(state, connection.workspace_id, &prefix, &identifier).await
        else {
            continue;
        };
        let close_intent = closing.contains(&identifier) && !preserve_close_intent;
        let reference_only = !qualifying.contains(&identifier);
        match patchbay_db::queries::vcs::link_issue_to_vcs_pull_request(
            &state.pool,
            issue.id,
            pull_request.id,
            close_intent,
            Some("system"),
            None,
            reference_only,
            preserve_close_intent,
        )
        .await
        {
            Ok(_) => {
                linked_issue_ids.push(issue.id.to_string());
                reevaluate.push(issue);
            }
            Err(error) => tracing::warn!(%error, "vcs: link failed"),
        }
    }
    if matches!(event.state.as_str(), "merged" | "closed") {
        for issue in reevaluate {
            maybe_complete_issue(state, issue).await;
        }
    }
    publish_pull_request(state, &pull_request, linked_issue_ids);
}

async fn mirror_ci_status(state: &HandlerState, connection: &VcsConnection, event: CiStatusEvent) {
    if event.sha.is_empty() || event.state.is_empty() {
        return;
    }
    if let Err(error) = patchbay_db::queries::vcs::upsert_vcs_commit_status(
        &state.pool,
        connection.id,
        &event.sha,
        &event.context,
        &event.state,
        Some(event_time(&event.updated_at)),
        nonempty(&event.target_url),
        nonempty(&event.description),
    )
    .await
    {
        tracing::warn!(%error, "vcs: upsert commit status failed");
        return;
    }
    let issue_ids = match patchbay_db::queries::vcs::list_issue_i_ds_for_vcspr_head(
        &state.pool,
        connection.id,
        &event.sha,
    )
    .await
    {
        Ok(ids) => ids,
        Err(error) => {
            tracing::warn!(%error, "vcs: lookup issues for status failed");
            return;
        }
    };
    for issue_id in issue_ids.into_iter().flatten() {
        state.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_PULL_REQUEST_UPDATED.into(),
            workspace_id: connection.workspace_id.to_string(),
            actor_type: "system".into(),
            payload: serde_json::json!({ "issue_id": issue_id.to_string() }),
            ..Default::default()
        });
    }
}

pub(crate) async fn maybe_complete_issue(state: &HandlerState, issue: Issue) {
    let category =
        patchbay_service::issue_status::effective(&state.pool, issue.workspace_id, &issue.status)
            .await;
    if matches!(category.as_str(), "done" | "cancelled") {
        return;
    }
    let aggregate =
        match patchbay_db::queries::vcs::get_issue_combined_pull_request_close_aggregate(
            &state.pool,
            issue.id,
        )
        .await
        {
            Ok(Some(aggregate)) => aggregate,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, issue_id = %issue.id, "vcs: count linked pr states failed");
                return;
            }
        };
    if aggregate.open_count == 0 && aggregate.merged_with_close_intent_count > 0 {
        crate::issue::advance_issue_to_done_from_pr(state, &issue, "github_pr_merged").await;
    }
}

fn publish_pull_request(
    state: &HandlerState,
    pull_request: &VcsPullRequest,
    linked_issue_ids: Vec<String>,
) {
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::EVENT_PULL_REQUEST_UPDATED.into(),
        workspace_id: pull_request.workspace_id.to_string(),
        actor_type: "system".into(),
        payload: serde_json::json!({
            "pull_request": crate::issue_pull_request::vcs_model_response(pull_request.clone()),
            "linked_issue_ids": linked_issue_ids,
        }),
        ..Default::default()
    });
}

pub(crate) async fn lookup_issue(
    state: &HandlerState,
    workspace_id: Uuid,
    prefix: &str,
    identifier: &str,
) -> Option<Issue> {
    let captures = IDENTIFIER.captures(identifier)?;
    if !captures[1].eq_ignore_ascii_case(prefix) {
        return None;
    }
    let number = captures[2].parse::<i32>().ok()?;
    patchbay_db::queries::issue::get_issue_by_number(&state.pool, workspace_id, number)
        .await
        .ok()
        .flatten()
}

pub(crate) fn extract_identifiers<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    extract_with(&IDENTIFIER, texts)
}

pub(crate) fn extract_closing_identifiers<'a>(
    texts: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    extract_with(&CLOSING_IDENTIFIER, texts)
}

fn extract_with<'a>(regex: &Regex, texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for text in texts {
        for captures in regex.captures_iter(text) {
            let identifier = format!("{}-{}", captures[1].to_ascii_uppercase(), &captures[2]);
            if seen.insert(identifier.clone()) {
                output.push(identifier);
            }
        }
    }
    output
}

fn event_time(raw: &str) -> DateTime<Utc> {
    optional_event_time(raw).unwrap_or_else(Utc::now)
}

fn optional_event_time(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw.trim())
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

fn nonempty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt;

    #[test]
    fn identifiers_match_go_link_and_close_grammar() {
        assert_eq!(
            extract_identifiers(["CORD-2 then cord-1 and CORD-2"]),
            vec!["CORD-2", "CORD-1"]
        );
        assert_eq!(
            extract_closing_identifiers(["Related CORD-1. Fixes: cord-2; resolve CORD-3"]),
            vec!["CORD-2", "CORD-3"]
        );
    }

    #[tokio::test]
    async fn limited_reader_matches_go_prefix_contract() {
        let body = read_limited(Body::from("abcdef"), 4).await.expect("body");
        assert_eq!(body, b"abcd");
    }

    #[tokio::test]
    async fn public_route_is_mounted_before_authentication() {
        let response = crate::build_router(None, None)
            .oneshot(
                Request::post(format!("/api/webhooks/vcs/{}", Uuid::new_v4()))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn empty_encrypted_secret_reaches_signature_rejection() {
        let secret_box = patchbay_util::secretbox::SecretBox::new(&[7; 32]).expect("secret box");
        let secret = decrypt_secret(&secret_box, "").expect("Go treats empty ciphertext as empty");
        let provider = patchbay_vcs::for_kind("forgejo").expect("forgejo provider");

        assert!(secret.is_empty());
        assert!(!provider.verify_signature(&secret, &HeaderMap::new(), b"{}"));
    }
}
