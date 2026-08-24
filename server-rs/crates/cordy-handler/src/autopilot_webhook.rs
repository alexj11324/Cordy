//! Public, bearer-token autopilot webhook ingress. The raw request is
//! signature-checked before dispatch and persisted before acknowledgement so
//! provider retries are idempotent and recoverable.

use axum::body::Body;
use axum::extract::{ConnectInfo, Extension, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::Utc;
use cordy_db::queries::{autopilot, webhook_delivery};
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::net::SocketAddr;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::webhook_rate_limit::{GateDecision, SlidingWindowGate};
use crate::{error::error_response, state::HandlerState};

const MAX_BODY: usize = 256 * 1024;
type HmacSha256 = Hmac<Sha256>;

pub fn router() -> Router<HandlerState> {
    Router::new().route("/api/webhooks/autopilots/{token}", post(webhook))
}

async fn read_body(body: Body) -> Result<Vec<u8>, Response> {
    let mut stream = body.into_data_stream();
    let mut output = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|_| error_response(StatusCode::BAD_REQUEST, "failed to read request body"))?;
        if output.len().saturating_add(chunk.len()) > MAX_BODY {
            return Err(error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload too large",
            ));
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

fn header(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn normalize(body: &[u8], headers: &HeaderMap) -> Result<Value, &'static str> {
    let body = body.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(body);
    if body.is_empty() {
        return Err("empty body");
    }
    let payload: Value = serde_json::from_slice(body).map_err(|_| "invalid json")?;
    if !payload.is_object() && !payload.is_array() {
        return Err("body must be a JSON object or array");
    }
    let supplied_event = payload
        .get("event")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let event = if let Some(value) = supplied_event {
        value.to_string()
    } else {
        let github = header(headers, "x-github-event");
        let gitlab = header(headers, "x-gitlab-event");
        let explicit = header(headers, "x-event-type");
        let action = payload
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !github.is_empty() && !action.is_empty() {
            format!("github.{github}.{action}")
        } else if !github.is_empty() {
            format!("github.{github}")
        } else if !gitlab.is_empty() {
            format!("gitlab.{gitlab}")
        } else if !explicit.is_empty() {
            explicit
        } else {
            payload
                .get("type")
                .or_else(|| payload.get("action"))
                .and_then(Value::as_str)
                .unwrap_or("webhook.received")
                .to_string()
        }
    };
    let event_payload = if supplied_event.is_some() {
        payload.get("eventPayload").cloned().unwrap_or(payload)
    } else {
        payload
    };
    let content_type = header(headers, header::CONTENT_TYPE.as_str())
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    Ok(
        json!({"event": event, "eventPayload": event_payload, "request": {
            "receivedAt": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "contentType": content_type,
        }}),
    )
}

fn signature_status(secret: Option<&str>, headers: &HeaderMap, body: &[u8]) -> &'static str {
    let Some(secret) = secret.filter(|value| !value.is_empty()) else {
        return "not_required";
    };
    let signature = header(headers, "x-hub-signature-256");
    let Some(hex_value) = signature.strip_prefix("sha256=") else {
        return if signature.is_empty() {
            "missing"
        } else {
            "invalid"
        };
    };
    let Ok(expected) = hex::decode(hex_value) else {
        return "invalid";
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return "invalid";
    };
    mac.update(body);
    if mac.verify_slice(&expected).is_ok() {
        "valid"
    } else {
        "invalid"
    }
}

fn selected_headers(headers: &HeaderMap) -> Value {
    let mut output = serde_json::Map::new();
    for name in [
        "user-agent",
        "x-github-event",
        "x-github-delivery",
        "x-gitlab-event",
        "x-event-type",
        "idempotency-key",
    ] {
        let value = header(headers, name);
        if !value.is_empty() {
            output.insert(name.into(), Value::String(value));
        }
    }
    if !header(headers, "x-hub-signature-256").is_empty() {
        output.insert("x-hub-signature-256-present".into(), Value::Bool(true));
    }
    Value::Object(output)
}

fn dedupe(provider: &str, headers: &HeaderMap) -> (Option<String>, Option<&'static str>) {
    let github = header(headers, "x-github-delivery");
    if provider == "github" && !github.is_empty() {
        return (Some(github), Some("x-github-delivery"));
    }
    let generic = header(headers, "idempotency-key");
    if !generic.is_empty() {
        return (Some(generic), Some("idempotency-key"));
    }
    if !github.is_empty() {
        return (Some(github), Some("x-github-delivery"));
    }
    (None, None)
}

fn event_allowed(filters: Option<&Value>, envelope: &Value) -> bool {
    let Some(filters) = filters.and_then(Value::as_array) else {
        return true;
    };
    if filters.is_empty() {
        return true;
    }
    let normalized = envelope
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let parts = normalized.split('.').collect::<Vec<_>>();
    let known = matches!(
        parts.first().copied(),
        Some("github" | "gitlab" | "generic")
    );
    let event = if known {
        parts.get(1).copied().unwrap_or_default()
    } else {
        parts.first().copied().unwrap_or_default()
    };
    let action = if known { parts.get(2).copied() } else { None }.or_else(|| {
        envelope
            .pointer("/eventPayload/action")
            .and_then(Value::as_str)
    });
    filters.iter().any(|filter| {
        if filter.get("event").and_then(Value::as_str) != Some(event) {
            return false;
        }
        match filter.get("actions").and_then(Value::as_array) {
            None => true,
            Some(actions) if actions.is_empty() => true,
            Some(actions) => action.is_some_and(|value| {
                actions
                    .iter()
                    .any(|allowed| allowed.as_str() == Some(value))
            }),
        }
    })
}

async fn terminal(
    state: &HandlerState,
    id: Uuid,
    status: &str,
    code: StatusCode,
    response: &Value,
    reason: &str,
) {
    let encoded = response.to_string();
    let _ = webhook_delivery::update_webhook_delivery_terminal(
        &state.pool,
        id,
        status,
        Some(reason),
        Some(reason),
        Some(code.as_u16().into()),
        Some(&encoded),
    )
    .await;
}

async fn duplicate_response(
    state: &HandlerState,
    delivery: cordy_db::models::WebhookDelivery,
) -> Response {
    let _ = webhook_delivery::bump_webhook_delivery_attempt(&state.pool, delivery.id).await;
    let run_id = match delivery.autopilot_run_id {
        Some(value) => Some(value),
        None => autopilot::get_autopilot_run_by_webhook_delivery(&state.pool, delivery.id)
            .await
            .ok()
            .flatten()
            .map(|run| run.id),
    };
    Json(json!({"status": "duplicate", "delivery_id": delivery.id, "run_id": run_id}))
        .into_response()
}

async fn webhook(
    State(state): State<HandlerState>,
    Path(token): Path<String>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if token.is_empty() {
        return error_response(StatusCode::NOT_FOUND, "webhook not found");
    }
    let cancel = CancellationToken::new();
    let ip = webhook_client_ip(&headers, peer, &state.auth_rate_limit.trusted_proxies);
    if !ip.is_empty() {
        if let Some(response) = limited_response(
            state
                .webhook_rate_limits
                .absolute_ip
                .allow(&ip, &cancel)
                .await,
            "absolute_ip",
            state.business_metrics.as_deref(),
        ) {
            return response;
        }
        if let Some(response) = limited_response(
            state
                .webhook_rate_limits
                .bad_credential_ip
                .check(&ip, &cancel)
                .await,
            "bad_credential_ip",
            state.business_metrics.as_deref(),
        ) {
            return response;
        }
    }
    let trigger = match autopilot::get_webhook_trigger_by_token(&state.pool, Some(&token)).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            consume_bad_credential(&state.webhook_rate_limits.bad_credential_ip, &ip, &cancel)
                .await;
            return error_response(StatusCode::NOT_FOUND, "webhook not found");
        }
        Err(error) => {
            tracing::error!(%error, "autopilot webhook token lookup failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let raw = match read_body(body).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let envelope = match normalize(&raw, &headers) {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };
    let (Some(trigger_id), Some(autopilot_id), Some(workspace_id)) = (
        trigger.id,
        trigger.autopilot_id,
        trigger.autopilot_workspace_id,
    ) else {
        return error_response(StatusCode::NOT_FOUND, "webhook not found");
    };
    let rule = match autopilot::get_autopilot(&state.pool, autopilot_id).await {
        Ok(Some(value)) if value.workspace_id == workspace_id => value,
        Ok(_) => return error_response(StatusCode::NOT_FOUND, "webhook not found"),
        Err(error) => {
            tracing::error!(%error, "autopilot webhook rule lookup failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let provider = if trigger.provider.trim().is_empty() {
        "generic"
    } else {
        trigger.provider.trim()
    };
    let (dedupe_key, dedupe_source) = dedupe(provider, &headers);
    if let Some(key) = dedupe_key.as_deref() {
        if let Ok(Some(existing)) = webhook_delivery::get_webhook_delivery_by_trigger_and_dedupe(
            &state.pool,
            trigger_id,
            Some(key),
        )
        .await
        {
            state.notify_webhook_delivery();
            return duplicate_response(&state, existing).await;
        }
    }
    let sig = signature_status(trigger.signing_secret.as_deref(), &headers, &raw);
    let event = envelope
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or("webhook.received");
    let content_type = header(&headers, header::CONTENT_TYPE.as_str());
    let delivery = match webhook_delivery::create_webhook_delivery(
        &state.pool,
        workspace_id,
        autopilot_id,
        trigger_id,
        provider,
        event,
        sig,
        "queued",
        &selected_headers(&headers),
        dedupe_key.as_deref(),
        dedupe_source,
        Some(&content_type),
        Some(&raw),
        Uuid::nil(),
        None,
        None,
        Uuid::now_v7(),
    )
    .await
    {
        Ok(Some(value)) => value,
        Err(error) if dedupe_key.is_some() => {
            tracing::debug!(%error, "autopilot webhook concurrent dedupe collision");
            match webhook_delivery::get_webhook_delivery_by_trigger_and_dedupe(
                &state.pool,
                trigger_id,
                dedupe_key.as_deref(),
            )
            .await
            {
                Ok(Some(existing)) => {
                    state.notify_webhook_delivery();
                    return duplicate_response(&state, existing).await;
                }
                _ => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
            }
        }
        Ok(None) | Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    };
    if matches!(sig, "missing" | "invalid") {
        let reason = if sig == "missing" {
            "missing_signature"
        } else {
            "invalid_signature"
        };
        let response = json!({"status": "rejected", "delivery_id": delivery.id, "reason": reason});
        consume_bad_credential(&state.webhook_rate_limits.bad_credential_ip, &ip, &cancel).await;
        terminal(
            &state,
            delivery.id,
            "rejected",
            StatusCode::UNAUTHORIZED,
            &response,
            reason,
        )
        .await;
        return (StatusCode::UNAUTHORIZED, Json(response)).into_response();
    }
    let ignored = if !trigger.enabled {
        Some("trigger_disabled")
    } else if rule.status == "archived" {
        Some("autopilot_archived")
    } else if rule.status != "active" {
        Some("autopilot_paused")
    } else if !event_allowed(trigger.event_filters.as_ref(), &envelope) {
        Some("event_filtered")
    } else {
        None
    };
    if let Some(reason) = ignored {
        let response = json!({"status": "ignored", "delivery_id": delivery.id, "reason": reason});
        terminal(
            &state,
            delivery.id,
            "ignored",
            StatusCode::OK,
            &response,
            reason,
        )
        .await;
        return Json(response).into_response();
    }
    let service = cordy_service::autopilot::AutopilotService::new(
        state.pool.clone(),
        state.bus.clone(),
        state.tasks.clone(),
    );
    let run = match service
        .admit_autopilot_webhook_delivery(&rule, trigger_id, &envelope, delivery.id)
        .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, delivery_id = %delivery.id, "autopilot webhook admission failed");
            state.notify_webhook_delivery();
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to admit autopilot",
            );
        }
    };
    let response = if run.status == "skipped" {
        json!({"status": "skipped", "delivery_id": delivery.id, "run_id": run.id, "reason": run.failure_reason})
    } else {
        json!({"status": "accepted", "delivery_id": delivery.id, "run_id": run.id, "autopilot_id": rule.id, "trigger_id": trigger_id})
    };
    let _ = webhook_delivery::acknowledge_webhook_delivery(
        &state.pool,
        delivery.id,
        Some(200),
        Some(&response.to_string()),
    )
    .await;
    state.notify_webhook_delivery();
    Json(response).into_response()
}

async fn consume_bad_credential(gate: &SlidingWindowGate, ip: &str, cancel: &CancellationToken) {
    if !ip.is_empty() {
        let _ = gate.allow(ip, cancel).await;
    }
}

fn limited_response(
    decision: GateDecision,
    gate: &'static str,
    metrics: Option<&cordy_metrics::BusinessMetrics>,
) -> Option<Response> {
    let GateDecision::Limited { retry_after } = decision else {
        return None;
    };
    if let Some(metrics) = metrics {
        metrics.record_webhook_rate_limited(gate);
    }
    tracing::warn!(gate, "autopilot webhook rate limited");
    let seconds = retry_after.as_millis().div_ceil(1_000).max(1);
    let mut response = error_response(StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded");
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    Some(response)
}

fn webhook_client_ip(
    headers: &HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    trusted_proxies: &[ipnetwork::IpNetwork],
) -> String {
    let remote = peer.map(|Extension(ConnectInfo(peer))| peer.ip());
    if remote.is_some_and(|ip| trusted_proxies.iter().any(|network| network.contains(ip))) {
        if let Some(forwarded) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return forwarded.to_string();
        }
        if let Some(real) = headers
            .get("x-real-ip")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return real.to_string();
        }
    }
    remote.map(|ip| ip.to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_body_bound_and_header_value_is_never_selected() {
        let mut headers = HeaderMap::new();
        let mut mac = HmacSha256::new_from_slice(b"secret").unwrap();
        mac.update(b"{}");
        headers.insert(
            "x-hub-signature-256",
            format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
                .parse()
                .unwrap(),
        );
        assert_eq!(signature_status(Some("secret"), &headers, b"{}"), "valid");
        assert_eq!(
            signature_status(Some("secret"), &headers, b"{ }"),
            "invalid"
        );
        let selected = selected_headers(&headers).to_string();
        assert!(selected.contains("present"));
        assert!(!selected.contains("sha256="));
    }

    #[test]
    fn normalization_rejects_scalars_and_keeps_provider_action() {
        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "pull_request".parse().unwrap());
        let value = normalize(br#"{"action":"opened","number":1}"#, &headers).unwrap();
        assert_eq!(value["event"], "github.pull_request.opened");
        assert!(normalize(b"true", &HeaderMap::new()).is_err());
    }
}
