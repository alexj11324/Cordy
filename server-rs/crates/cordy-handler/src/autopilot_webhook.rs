//! Public, bearer-token autopilot webhook ingress. The raw request is
//! signature-checked before dispatch and persisted before acknowledgement so
//! provider retries are idempotent and recoverable.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::Utc;
use cordy_db::queries::{autopilot, webhook_delivery};
use cordy_service::autopilot::AutopilotQuotaExceededError;
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use ipnetwork::IpNetwork;
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{error::error_response, state::HandlerState};

const MAX_BODY: usize = 256 * 1024;
type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct SlidingIpLimit {
    hits: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
    limit: usize,
    window: Duration,
}

impl SlidingIpLimit {
    fn new(limit: usize, window: Duration) -> Self {
        Self {
            hits: Arc::new(Mutex::new(HashMap::new())),
            limit,
            window,
        }
    }

    async fn evaluate(&self, ip: &str, consume: bool) -> bool {
        if self.limit == 0 || ip.is_empty() {
            return true;
        }
        let cutoff = Instant::now() - self.window;
        let mut hits = self.hits.lock().await;
        hits.retain(|_, entries| {
            entries.retain(|seen| *seen > cutoff);
            !entries.is_empty()
        });
        let count = hits.get(ip).map_or(0, Vec::len);
        if count >= self.limit {
            return false;
        }
        if consume {
            hits.entry(ip.to_string()).or_default().push(Instant::now());
        }
        true
    }
}

/// Two independent webhook safety budgets. The high absolute ceiling is
/// charged by every request; the lower budget is checked before token lookup
/// but charged only after an unknown token or bad signature is identified.
#[derive(Clone)]
pub(crate) struct WebhookRateLimits {
    absolute: SlidingIpLimit,
    bad_credentials: SlidingIpLimit,
    pub(crate) trusted_proxies: Vec<IpNetwork>,
}

impl WebhookRateLimits {
    pub(crate) fn new(trusted_proxies: Vec<IpNetwork>) -> Self {
        Self {
            absolute: SlidingIpLimit::new(600, Duration::from_secs(60)),
            bad_credentials: SlidingIpLimit::new(30, Duration::from_secs(60)),
            trusted_proxies,
        }
    }

    #[cfg(test)]
    fn test(absolute: usize, bad_credentials: usize) -> Self {
        Self {
            absolute: SlidingIpLimit::new(absolute, Duration::from_secs(60)),
            bad_credentials: SlidingIpLimit::new(bad_credentials, Duration::from_secs(60)),
            trusted_proxies: Vec::new(),
        }
    }

    fn client_ip(&self, request: &Request) -> String {
        let remote = request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0.ip());
        if remote.is_some_and(|ip| {
            self.trusted_proxies
                .iter()
                .any(|network| network.contains(ip))
        }) {
            if let Some(forwarded) = request
                .headers()
                .get("x-forwarded-for")
                .and_then(|value| value.to_str().ok())
            {
                for candidate in forwarded.rsplit(',').map(str::trim) {
                    if let Ok(ip) = candidate.parse::<IpAddr>() {
                        if !self
                            .trusted_proxies
                            .iter()
                            .any(|network| network.contains(ip))
                        {
                            return ip.to_string();
                        }
                    }
                }
            }
        }
        remote.map(|ip| ip.to_string()).unwrap_or_default()
    }

    async fn admit(&self, ip: &str) -> bool {
        self.absolute.evaluate(ip, true).await && self.bad_credentials.evaluate(ip, false).await
    }

    async fn charge_bad_credential(&self, ip: &str) {
        let _ = self.bad_credentials.evaluate(ip, true).await;
    }
}

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

/// Rejected signatures are terminal at the initial INSERT. Besides making a
/// failed follow-up update non-dispatchable, this keeps them outside the
/// partial dedupe index so a bad attempt cannot collide with or impersonate a
/// valid provider delivery.
fn rejected_signature(signature_status: &str) -> bool {
    matches!(signature_status, "missing" | "invalid")
}

fn webhook_dedupe_conflict(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .is_some_and(|database| {
            database.code().as_deref() == Some("23505")
                && database.constraint() == Some("idx_webhook_delivery_dedupe")
        })
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
    error: Option<&str>,
    reason_code: Option<&str>,
) -> anyhow::Result<()> {
    let encoded = response.to_string();
    let updated = webhook_delivery::update_webhook_delivery_terminal(
        &state.pool,
        id,
        status,
        error,
        reason_code,
        Some(code.as_u16().into()),
        Some(&encoded),
    )
    .await?;
    anyhow::ensure!(
        updated.is_some(),
        "webhook delivery disappeared during terminal update"
    );
    Ok(())
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
    request: Request,
) -> Response {
    let client_ip = state.webhook_rate_limits.client_ip(&request);
    if !state.webhook_rate_limits.admit(&client_ip).await {
        return error_response(StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded");
    }
    if token.is_empty() {
        state
            .webhook_rate_limits
            .charge_bad_credential(&client_ip)
            .await;
        return error_response(StatusCode::NOT_FOUND, "webhook not found");
    }
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let trigger = match autopilot::get_webhook_trigger_by_token(&state.pool, Some(&token)).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            state
                .webhook_rate_limits
                .charge_bad_credential(&client_ip)
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
    let sig = signature_status(trigger.signing_secret.as_deref(), &headers, &raw);
    let signature_rejected = rejected_signature(sig);
    if signature_rejected {
        state
            .webhook_rate_limits
            .charge_bad_credential(&client_ip)
            .await;
    }
    let (dedupe_key, dedupe_source) = dedupe(provider, &headers);
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
        if signature_rejected {
            "rejected"
        } else {
            "queued"
        },
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
        Err(error) if dedupe_key.is_some() && webhook_dedupe_conflict(&error) => {
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
                Ok(None) => {
                    tracing::error!(trigger_id = %trigger_id, "webhook dedupe conflict row disappeared");
                    return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
                }
                Err(error) => {
                    tracing::error!(%error, trigger_id = %trigger_id, "webhook dedupe conflict lookup failed");
                    return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
                }
            }
        }
        Ok(None) => {
            tracing::error!("webhook delivery insert returned no row");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
        Err(error) => {
            tracing::error!(%error, trigger_id = %trigger_id, "webhook delivery insert failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    if signature_rejected {
        let reason = if sig == "missing" {
            "missing_signature"
        } else {
            "invalid_signature"
        };
        let response = json!({"status": "rejected", "delivery_id": delivery.id, "reason": reason});
        if let Err(error) = terminal(
            &state,
            delivery.id,
            "rejected",
            StatusCode::UNAUTHORIZED,
            &response,
            Some(reason),
            Some(reason),
        )
        .await
        {
            tracing::error!(%error, delivery_id = %delivery.id, "failed to reject webhook delivery");
            state.notify_webhook_delivery();
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
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
        if let Err(error) = terminal(
            &state,
            delivery.id,
            "ignored",
            StatusCode::OK,
            &response,
            Some(reason),
            Some(reason),
        )
        .await
        {
            tracing::error!(%error, delivery_id = %delivery.id, "failed to ignore webhook delivery");
            state.notify_webhook_delivery();
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
        return Json(response).into_response();
    }
    let run = match state
        .autopilots
        .admit_autopilot_webhook_delivery(&rule, trigger_id, &envelope, delivery.id)
        .await
    {
        Ok(value) => value,
        Err(error) => {
            if let Some(quota) = error.downcast_ref::<AutopilotQuotaExceededError>() {
                let quota_error = quota.to_string();
                let response = json!({
                    "status": "ignored",
                    "delivery_id": delivery.id,
                    "reason_code": "quota_exceeded",
                });
                if let Err(update_error) = terminal(
                    &state,
                    delivery.id,
                    "ignored",
                    StatusCode::OK,
                    &response,
                    Some(&quota_error),
                    Some("quota_exceeded"),
                )
                .await
                {
                    tracing::error!(
                        %update_error,
                        delivery_id = %delivery.id,
                        "failed to persist quota-exceeded webhook delivery"
                    );
                    state.notify_webhook_delivery();
                    return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
                }
                return Json(response).into_response();
            }
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

    #[test]
    fn rejected_signatures_are_terminal_before_persistence() {
        let mut headers = HeaderMap::new();
        headers.insert("x-github-delivery", "delivery-1".parse().unwrap());

        assert!(!rejected_signature("valid"));
        assert!(!rejected_signature("not_required"));
        assert!(rejected_signature("invalid"));
        assert!(rejected_signature("missing"));
        assert_eq!(dedupe("github", &headers).0.as_deref(), Some("delivery-1"));
    }

    #[tokio::test]
    async fn webhook_ip_limits_separate_absolute_traffic_from_bad_credentials() {
        let absolute = WebhookRateLimits::test(1, 10);
        assert!(absolute.admit("203.0.113.1").await);
        assert!(!absolute.admit("203.0.113.1").await);

        let debt = WebhookRateLimits::test(10, 1);
        assert!(debt.admit("203.0.113.2").await);
        assert!(debt.admit("203.0.113.2").await);
        debt.charge_bad_credential("203.0.113.2").await;
        assert!(!debt.admit("203.0.113.2").await);
    }
}
