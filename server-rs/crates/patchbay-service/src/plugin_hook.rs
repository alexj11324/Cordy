//! The hook engine.
//!
//! The one place Patchbay calls OUT to a plugin's own server.
//!
//! Everything before this ran the other way — a sandboxed surface asked the
//! host and the host acted on the signed-in user's session, so no request ever
//! left our infrastructure on a plugin's say-so. A hook does leave, which is
//! why the checks here are about the destination rather than the caller: the
//! host must be one the manifest declared through a `net:` scope, it must
//! resolve to a public address at dial time, and the body must be signed so
//! the receiver can tell our call from anyone else's.

use std::time::{Duration, SystemTime};

use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use patchbay_db::models::PluginInstallation;
use patchbay_plugincontract::{net_domains, Hook, TRANSPORT_HTTP};

use crate::plugin::{
    check_hook_rate, decode_scopes, hook_allows_trigger, hook_failure_status, host_in_net_scopes,
    is_dev_origin, parse_installation_manifest, plugin_errf, redact_hook_error, truncate_str,
    uuid_string, PluginError, PluginErrorKind, PluginService, HOOK_BREAKER_THRESHOLD,
    HOOK_BREAKER_WINDOW_SECS, HOOK_RATE_WINDOW_SECS, HOOK_SIGNATURE_VERSION,
};
use crate::plugin_token::{CallbackGrantParts, CallbackTokens, HookActor};

/// Fallback when a manifest omits timeout_ms.
pub const HOOK_DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Response bodies are read for the caller, so they are capped. A hook that
/// answers with a gigabyte should fail, not consume the host.
const HOOK_MAX_RESPONSE_BYTES: usize = 1 << 20;

/// Three attempts total, then the call is abandoned.
pub const HOOK_EVENT_ATTEMPTS: i32 = 3;
/// Backoff between event-hook retries.
pub const HOOK_EVENT_BACKOFF: Duration = Duration::from_secs(2);

/// What a completed hook call returns to whoever invoked it.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct HookResult {
    pub status: String,
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    pub output: serde_json::Value,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    pub latency_ms: i64,
    pub hook_key: String,
    pub trigger: String,
    pub attempts: i32,
}

impl HookResult {
    fn new(hook_key: &str, trigger: &str, attempts: i32) -> Self {
        Self {
            hook_key: hook_key.to_string(),
            trigger: trigger.to_string(),
            attempts,
            ..Default::default()
        }
    }
}

/// One call the engine has been asked to make.
#[derive(Debug, Clone)]
pub struct HookInvocation<'a> {
    pub installation: &'a PluginInstallation,
    pub hook: &'a Hook,
    pub trigger: &'a str,
    /// Set only for the event trigger, and names what happened.
    pub event_type: &'a str,
    pub actor: HookActor,
    /// The issue this call is about, when there is one. It narrows the callback
    /// token so a handler answering about one issue cannot use the same grant
    /// to reach across the workspace.
    pub issue_id: Option<Uuid>,
    pub input: Option<&'a serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Wire format — plugin_hook.go 102-140
// ---------------------------------------------------------------------------

/// The wire format. Stable and small on purpose: a hook handler written against
/// v1 should not have to care what else the host learns how to send later.
#[derive(Debug, Serialize)]
struct HookRequestBody {
    version: u8,
    hook_key: String,
    trigger: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    event_type: String,
    workspace_id: String,
    installation_id: String,
    /// The issue this call is about, as resolved and permission-checked by the
    /// host. Sent because the alternative is every handler reading it out of
    /// client-supplied `input` — unvalidated, and absent entirely for the event
    /// trigger, where no client was involved at all.
    #[serde(rename = "issue_id", skip_serializing_if = "String::is_empty")]
    issue_id: String,
    actor: HookRequestActor,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<serde_json::Value>,
    /// The installation's non-secret configuration, the values an administrator
    /// typed into the host-rendered form. Sent because the handler has no other
    /// way to read them — the Action API deliberately has no config endpoint —
    /// and a plugin forced to keep its own second copy would make the manifest's
    /// config block decorative.
    ///
    /// Secret-typed fields are NEVER included. They are the plugin's credentials
    /// for ITS OWN services; it already has them, and putting them on the wire
    /// would hand every value to whoever holds the endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    config: Option<serde_json::Map<String, serde_json::Value>>,
    /// Lets the handler call the Action API back for the few minutes it is
    /// valid. Narrower than the installation's own token and tied to this one
    /// call, so a handler that leaks it leaks a few minutes of the scopes it
    /// was already using, not standing access.
    #[serde(skip_serializing_if = "String::is_empty")]
    callback_token: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    callback_url: String,
}

#[derive(Debug, Serialize)]
struct HookRequestActor {
    #[serde(rename = "type")]
    actor_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    id: String,
}

// ---------------------------------------------------------------------------
// Engine — plugin_hook.go 178-323
// ---------------------------------------------------------------------------

async fn recent_invocation_count(
    pool: &sqlx::PgPool,
    installation_id: Uuid,
    hook_key: &str,
) -> i64 {
    let since = (Utc::now() - chrono::Duration::seconds(HOOK_RATE_WINDOW_SECS)).to_string();
    match since.parse::<chrono::DateTime<Utc>>() {
        Ok(created_at) => {
            // A telemetry read that fails must not take the feature down with
            // it — map any error to a count of 0 before the pure check.
            patchbay_db::queries::plugin::count_recent_plugin_invocations(
                pool,
                installation_id,
                hook_key,
                Some(created_at),
            )
            .await
            .ok()
            .flatten()
            .unwrap_or(0)
        }
        Err(_) => 0,
    }
}

/// Reports whether event delivery for this hook is currently suspended after
/// repeated failures. Public entry mirroring Go's `HookBreakerOpen`.
pub async fn hook_breaker_open(pool: &sqlx::PgPool, installation_id: Uuid, hook_key: &str) -> bool {
    let since = (Utc::now() - chrono::Duration::seconds(HOOK_BREAKER_WINDOW_SECS)).to_string();
    let failures = match since.parse::<chrono::DateTime<Utc>>() {
        Ok(created_at) => {
            match patchbay_db::queries::plugin::count_recent_plugin_failures(
                pool,
                installation_id,
                hook_key,
                Some(created_at),
            )
            .await
            {
                Ok(count) => count.unwrap_or(0),
                Err(_) => return false,
            }
        }
        Err(_) => return false,
    };
    failures >= HOOK_BREAKER_THRESHOLD
}

/// Performs one call and records it.
///
/// Callers pick the blocking behaviour, not this function: ui/manual await the
/// result because a person is watching, and the event dispatcher runs it on its
/// own task because the host request that produced the event must not wait for
/// a third party.
pub async fn invoke_hook(
    service: &PluginService,
    callbacks: Option<&CallbackTokens>,
    callback_base_url: &str,
    invocation: HookInvocation<'_>,
    attempt: i32,
) -> (HookResult, Result<(), PluginError>) {
    let hook = invocation.hook;
    let mut result = HookResult::new(&hook.key, invocation.trigger, attempt);
    let installation = invocation.installation;

    let refusal = async {
        if !installation.enabled {
            return Some(plugin_errf(
                PluginErrorKind::Forbidden,
                "this Plugin is disabled",
            ));
        }
        if !hook_allows_trigger(hook, invocation.trigger) {
            return Some(plugin_errf(
                PluginErrorKind::Forbidden,
                format!(
                    "hook {:?} does not declare the {} trigger",
                    hook.key, invocation.trigger
                ),
            ));
        }
        if hook.transport.transport_type != TRANSPORT_HTTP {
            return Some(plugin_errf(
                PluginErrorKind::Incompatible,
                format!(
                    "hook transport {:?} is not supported yet",
                    hook.transport.transport_type
                ),
            ));
        }
        None
    }
    .await;
    if let Some(err) = refusal {
        return (result, Err(err));
    }

    let count = recent_invocation_count(&service.pool, installation.id, &hook.key).await;
    if let Err(err) = check_hook_rate(count, &hook.key) {
        return (result, Err(err));
    }

    let started = SystemTime::now();
    let outcome = call_hook_endpoint(service, callbacks, callback_base_url, &invocation).await;
    let latency_ms = started
        .elapsed()
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;

    result.latency_ms = latency_ms;
    let (message, call_failed) = match outcome {
        Err(call_err) => {
            result.status = hook_failure_status(&call_err).to_string();
            let message = redact_hook_error(&call_err);
            result.error = message.clone();
            (message, Some(call_err))
        }
        Ok(output) => {
            result.status = "ok".to_string();
            result.output = output;
            (String::new(), None)
        }
    };

    record_invocation(
        service,
        &invocation,
        &result.status,
        attempt,
        latency_ms,
        &message,
    )
    .await;
    if let Some(err) = call_failed {
        return (result, Err(err));
    }
    (result, Ok(()))
}

/// The network half: validate the destination, sign, send.
async fn call_hook_endpoint(
    service: &PluginService,
    callbacks: Option<&CallbackTokens>,
    callback_base_url: &str,
    invocation: &HookInvocation<'_>,
) -> Result<serde_json::Value, PluginError> {
    let installation = invocation.installation;
    let granted =
        decode_scopes(&crate::plugin::json_bytes(&installation.granted_scopes)).unwrap_or_default();

    // The destination must be inside the consented `net:` set. Passing the
    // granted domains as the allowlist means the same string the admin approved
    // on the consent screen is what bounds the request — there is no second
    // list to fall out of sync.
    let domains = net_domains(&granted);
    if domains.is_empty() {
        return Err(plugin_errf(
            PluginErrorKind::Forbidden,
            "this Plugin was granted no net: scope, so it cannot call out",
        ));
    }

    // Two ways to reach an endpoint, and only the network guard differs between
    // them. The consent check does not: a destination outside the granted
    // `net:` set is refused on both paths, because that is what the admin
    // approved and no deployment setting may widen it.
    let transport_url = invocation.hook.transport.url.as_str();
    let endpoint = if is_dev_origin(&service.dev_origins, transport_url) {
        // The operator named this exact origin in PATCHBAY_PLUGIN_DEV_ORIGINS —
        // the same opt-in that lets a manifest be served from a local dev
        // server, for the same reason: an author building a hook has nowhere
        // public to point it yet.
        let parsed = url::Url::parse(transport_url).map_err(|e| {
            PluginError::with_source(
                PluginErrorKind::Invalid,
                "hook endpoint is not a valid URL",
                e,
            )
        })?;
        let hostname = parsed.host_str().unwrap_or_default();
        if !host_in_net_scopes(hostname, &domains) {
            return Err(plugin_errf(
                PluginErrorKind::Forbidden,
                "hook endpoint host is not covered by a net: scope",
            ));
        }
        parsed
    } else {
        patchbay_remotemcp::validate_public_https_endpoint(transport_url, &domains, None)
            .await
            .map_err(|e| {
                PluginError::with_source(
                    PluginErrorKind::Forbidden,
                    "hook endpoint is not allowed",
                    e,
                )
            })?
    };

    let mut body = build_hook_body(callbacks, callback_base_url, invocation)?;
    // The grant lives exactly as long as the call it was issued for. Without
    // this it would stay usable for the rest of its TTL after the handler has
    // already answered.
    let issued_callback_token = body.callback_token.clone();
    let encoded = serde_json::to_vec(&body).map_err(|e| {
        PluginError::with_source(PluginErrorKind::Invalid, "encode hook request", e)
    })?;
    body.input = None; // encoded above; keep the borrow checker simple

    let timeout = if invocation.hook.timeout_ms > 0 {
        Duration::from_millis(invocation.hook.timeout_ms as u64)
    } else {
        HOOK_DEFAULT_TIMEOUT
    };

    let timestamp = Utc::now().timestamp().to_string();
    let signature = sign_hook_payload_with_deployment_key(
        &service.deployment_key,
        installation.id,
        &timestamp,
        &encoded,
    )?;

    let response = send_hook_request(
        service,
        &endpoint,
        encoded,
        &timestamp,
        &signature,
        timeout,
        installation.id,
    )
    .await;

    // Revoke after the attempt regardless of outcome.
    if let Some(callbacks) = callbacks {
        if !issued_callback_token.is_empty() {
            callbacks.revoke(&issued_callback_token);
        }
    }
    let response = response?;
    let payload_bytes = response;

    if payload_bytes.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_slice::<serde_json::Value>(&payload_bytes).map_err(|_| {
        plugin_errf(
            PluginErrorKind::Invalid,
            "hook endpoint returned a non-JSON body",
        )
    })
}

/// Sends the POST through the right client and applies the response cap.
async fn send_hook_request(
    service: &PluginService,
    endpoint: &url::Url,
    encoded: Vec<u8>,
    timestamp: &str,
    signature: &str,
    timeout: Duration,
    installation_id: Uuid,
) -> Result<Vec<u8>, PluginError> {
    if is_dev_origin(&service.dev_origins, endpoint.as_str()) {
        // Dev path: plain reqwest with no redirects. A 302 would replay the
        // SIGNED body and the callback token to wherever it pointed —
        // redirects stay off. (The dev CA from PATCHBAY_PLUGIN_DEV_CA applies to
        // the manifest fetch path; hook calls to dev origins go through this
        // client with system roots.)
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| {
                PluginError::with_source(PluginErrorKind::Invalid, "build hook request", e)
            })?;
        let response = client
            .post(endpoint.as_str())
            .timeout(timeout)
            .header("Content-Type", "application/json")
            .header("X-Patchbay-Timestamp", timestamp)
            .header("X-Cordy-Timestamp", timestamp) // legacy-brand-compat
            .header(
                "X-Patchbay-Signature",
                format!("{HOOK_SIGNATURE_VERSION}={signature}"),
            )
            .header(
                "X-Cordy-Signature", // legacy-brand-compat
                format!("{HOOK_SIGNATURE_VERSION}={signature}"),
            )
            .header("X-Patchbay-Plugin-Installation", uuid_string(installation_id))
            .header(
                "X-Cordy-Plugin-Installation", // legacy-brand-compat
                uuid_string(installation_id),
            )
            .header("User-Agent", "Patchbay-Hooks/1")
            .body(encoded)
            .send()
            .await
            .map_err(|e| {
                PluginError::with_source(
                    PluginErrorKind::Unavailable,
                    "hook endpoint did not answer",
                    e,
                )
            })?;
        finish_hook_response(response).await
    } else {
        // Secure path: pinned connector re-resolves at dial and refuses a
        // non-public address, so a hostname that passed validation and then
        // flipped to a private IP cannot be used to reach inside the network.
        // (hyper follows no redirects by construction — matching Go's
        // CheckRedirect refusal.)
        let client = patchbay_remotemcp::new_secure_http_client(endpoint);
        let request = http::Request::builder()
            .method(http::Method::POST)
            .uri(endpoint.as_str())
            .header("Content-Type", "application/json")
            .header("X-Patchbay-Timestamp", timestamp)
            .header("X-Cordy-Timestamp", timestamp) // legacy-brand-compat
            .header(
                "X-Patchbay-Signature",
                format!("{HOOK_SIGNATURE_VERSION}={signature}"),
            )
            .header(
                "X-Cordy-Signature", // legacy-brand-compat
                format!("{HOOK_SIGNATURE_VERSION}={signature}"),
            )
            .header("X-Patchbay-Plugin-Installation", uuid_string(installation_id))
            .header(
                "X-Cordy-Plugin-Installation", // legacy-brand-compat
                uuid_string(installation_id),
            )
            .header("User-Agent", "Patchbay-Hooks/1")
            .body(patchbay_remotemcp::RequestBody::from(encoded))
            .map_err(|e| {
                PluginError::with_source(PluginErrorKind::Invalid, "build hook request", e)
            })?;
        let response = tokio::time::timeout(timeout, client.send(request))
            .await
            .map_err(|_| {
                PluginError::with_source(
                    PluginErrorKind::Unavailable,
                    "hook endpoint did not answer in time",
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "hook timeout",
                    )),
                )
            })?
            .map_err(|e| match e {
                patchbay_remotemcp::Error::CallTimeout => PluginError::with_source(
                    PluginErrorKind::Unavailable,
                    "hook endpoint did not answer in time",
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "call timeout",
                    )),
                ),
                other => PluginError::with_source(
                    PluginErrorKind::Unavailable,
                    "hook endpoint did not answer",
                    Box::new(other),
                ),
            })?;
        let (parts, body) = response.into_parts();
        let status = parts.status;
        if !(200..300).contains(&status.as_u16()) {
            return Err(plugin_errf(
                PluginErrorKind::Unavailable,
                format!("hook endpoint returned {}", status.as_u16()),
            ));
        }
        if body.len() > HOOK_MAX_RESPONSE_BYTES {
            return Err(plugin_errf(
                PluginErrorKind::Invalid,
                format!("hook response exceeds {HOOK_MAX_RESPONSE_BYTES} bytes"),
            ));
        }
        Ok(body)
    }
}

async fn finish_hook_response(response: reqwest::Response) -> Result<Vec<u8>, PluginError> {
    let status = response.status();
    let bytes = response.bytes().await.map_err(|e| {
        PluginError::with_source(PluginErrorKind::Unavailable, "read hook response", e)
    })?;
    if !(200..300).contains(&status.as_u16()) {
        return Err(plugin_errf(
            PluginErrorKind::Unavailable,
            format!("hook endpoint returned {}", status.as_u16()),
        ));
    }
    if bytes.len() > HOOK_MAX_RESPONSE_BYTES {
        return Err(plugin_errf(
            PluginErrorKind::Invalid,
            format!("hook response exceeds {HOOK_MAX_RESPONSE_BYTES} bytes"),
        ));
    }
    Ok(bytes.to_vec())
}

/// Reads the installation's stored configuration and drops anything the
/// manifest declared as a secret.
///
/// Filtered against the MANIFEST rather than against the stored shape: secrets
/// live in their own table and should never appear in the config column at all,
/// so this is the check that would catch it if one ever did.
pub(crate) fn non_secret_config(
    installation: &PluginInstallation,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    if installation.config.is_null() {
        return None;
    }
    let values = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(
        &crate::plugin::json_bytes(&installation.config),
    )
    .ok()?;
    let manifest =
        parse_installation_manifest(&crate::plugin::json_bytes(&installation.manifest)).ok()?;
    // Without a manifest there is no way to tell which keys are secret, so
    // send none of them.
    let filtered: serde_json::Map<String, serde_json::Value> = values
        .into_iter()
        .filter(|(key, _)| {
            manifest
                .config
                .field(key)
                .map(|field| field.field_type != CONFIG_SECRET_VALUE)
                .unwrap_or(false)
        })
        .collect();
    if filtered.is_empty() {
        None
    } else {
        Some(filtered)
    }
}

use patchbay_plugincontract::CONFIG_SECRET as CONFIG_SECRET_VALUE;

fn build_hook_body(
    callbacks: Option<&CallbackTokens>,
    callback_base_url: &str,
    invocation: &HookInvocation<'_>,
) -> Result<HookRequestBody, PluginError> {
    let installation = invocation.installation;
    let mut body = HookRequestBody {
        version: 1,
        hook_key: invocation.hook.key.clone(),
        trigger: invocation.trigger.to_string(),
        event_type: invocation.event_type.to_string(),
        workspace_id: uuid_string(installation.workspace_id),
        installation_id: uuid_string(installation.id),
        issue_id: invocation.issue_id.map(uuid_string).unwrap_or_default(),
        actor: HookRequestActor {
            actor_type: invocation.actor.actor_type.clone(),
            id: if invocation.actor.id == Uuid::nil() {
                String::new()
            } else {
                uuid_string(invocation.actor.id)
            },
        },
        input: None,
        config: non_secret_config(installation),
        callback_token: String::new(),
        callback_url: String::new(),
    };
    if let Some(input) = invocation.input {
        body.input = Some(input.clone());
    }
    if let Some(callbacks) = callbacks {
        let token = callbacks.issue(CallbackGrantParts {
            installation_id: installation.id,
            workspace_id: installation.workspace_id,
            hook_key: &invocation.hook.key,
            trigger: invocation.trigger,
            actor: invocation.actor.clone(),
            issue_id: invocation.issue_id,
        })?;
        body.callback_token = token;
        body.callback_url = callback_base_url.to_string();
    }
    Ok(body)
}

/// Signs one outbound hook payload with this service's deployment key. Port of
/// `(*PluginService).SignHookPayload`.
pub fn sign_hook_payload_with_deployment_key(
    deployment_key: &[u8],
    installation_id: Uuid,
    timestamp: &str,
    body: &[u8],
) -> Result<String, PluginError> {
    crate::plugin::sign_hook_payload(deployment_key, installation_id, timestamp, body)
}

/// Records one attempt. Best effort: telemetry must never fail the call it is
/// describing.
async fn record_invocation(
    service: &PluginService,
    invocation: &HookInvocation<'_>,
    status: &str,
    attempt: i32,
    latency_ms: i64,
    message: &str,
) {
    let event_type = if invocation.event_type.is_empty() {
        None
    } else {
        Some(invocation.event_type)
    };
    let error = if message.is_empty() {
        None
    } else {
        Some(truncate_str(message, 500))
    };
    let latency_i32 = latency_ms.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    let _ = patchbay_db::queries::plugin::create_plugin_invocation(
        &service.pool,
        invocation.installation.id,
        invocation.installation.workspace_id,
        &invocation.hook.key,
        invocation.trigger,
        status,
        attempt,
        latency_i32,
        event_type,
        error,
    )
    .await;
}
