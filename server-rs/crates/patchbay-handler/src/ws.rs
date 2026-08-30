//! WebSocket pump, token authentication, and first-message auth handshake on
//! axum's WS upgrade.
//!
//! Contract notes (must stay byte-identical with Go):
//! - Query params: `workspace_id`, or `workspace_slug` resolved via DB.
//! - Cookie auth (`patchbay_auth`) when present, else first-frame auth
//!   `{"type":"auth","payload":{"token":"..."}}`.
//! - Frames: `auth_ack` / `auth_error` / `subscribe(_ack|_error)` /
//!   `unsubscribe_ack` / `ping`→`pong`.
//! - Read limit 64 KiB (pre-auth enforced), pong deadline 60 s, server ping
//!   every 54 s. gorilla's pong-handler deadline refresh becomes "any inbound
//!   frame counts as liveness" — axum auto-replies protocol pings.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{ConnectInfo, Extension, Query, State, WebSocketUpgrade};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use patchbay_auth::cookie::{AUTH_COOKIE_NAME, LEGACY_AUTH_COOKIE_NAME};
use patchbay_auth::disabled_users::{
    is_temporarily_disabled_user, is_temporarily_disabled_user_id,
};
use patchbay_auth::jwt::hash_token;
use patchbay_db::queries::{guest, member, personal_access_token, user, workspace};
use patchbay_middleware::auth::decode_jwt_claims;
use patchbay_realtime::broadcaster::{SCOPE_CHAT, SCOPE_TASK, SCOPE_USER, SCOPE_WORKSPACE};
use patchbay_realtime::hub::PatResolver as _;
use patchbay_realtime::hub::{ClientHandle, Hub, ScopeAuthorizer};
use patchbay_realtime::metrics::M;
use patchbay_service::task_service::TaskService;
use serde_json::{json, Value};

use crate::error::error_response;
use crate::state::HandlerState;

const PONG_WAIT_SECS: u64 = 60;
const PING_PERIOD_SECS: u64 = PONG_WAIT_SECS * 9 / 10;
/// Caps a single inbound message (Go: inboundReadLimit). The largest frame a
/// client legitimately sends is the token auth frame (<1 KiB); without a cap
/// one connection can grow the buffered message unbounded (Go hub.go comment).
const INBOUND_READ_LIMIT: usize = 64 * 1024;
const WRITE_WAIT_SECS: u64 = 10;

/// DB-backed ownership check for task/chat subscriptions. The hub's legacy
/// seam is synchronous, so the point lookup is bridged with `block_in_place`;
/// the async WebSocket task itself is never nested through `Handle::block_on`.
pub struct DbScopeAuthorizer {
    tasks: Arc<patchbay_service::task_service::TaskService>,
}

impl DbScopeAuthorizer {
    pub fn new(tasks: Arc<patchbay_service::task_service::TaskService>) -> Self {
        Self { tasks }
    }
}

impl ScopeAuthorizer for DbScopeAuthorizer {
    fn authorize_scope(
        &self,
        _user_id: &str,
        workspace_id: &str,
        scope_type: &str,
        scope_id: &str,
    ) -> anyhow::Result<bool> {
        let workspace_id = uuid::Uuid::parse_str(workspace_id)?;
        let scope_id = uuid::Uuid::parse_str(scope_id)?;
        let tasks = self.tasks.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                match scope_type {
                    SCOPE_TASK => {
                        let Some(task) =
                            patchbay_db::queries::agent::get_agent_task(&tasks.pool, scope_id)
                                .await?
                        else {
                            return Ok(false);
                        };
                        Ok(tasks
                            .resolve_task_workspace_id(&task)
                            .await
                            .is_some_and(|resolved| resolved == workspace_id.to_string()))
                    }
                    SCOPE_CHAT => Ok(patchbay_db::queries::chat::get_chat_session_in_workspace(
                        &tasks.pool,
                        scope_id,
                        workspace_id,
                    )
                    .await?
                    .is_some()),
                    _ => Ok(false),
                }
            })
        })
    }
}

/// GET /ws — upgrade + auth handshake, then spawn read/write pumps.
///
/// Port of `realtime.HandleWebSocket`. WebSocket handshakes do not inherit
/// ordinary CORS enforcement, so origin validation happens before upgrade.
pub async fn ws_handler(
    State(state): State<HandlerState>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    connect_info: Option<Extension<ConnectInfo<std::net::SocketAddr>>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(hub) = state.hub.clone() else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "websocket unavailable");
    };

    if !check_origin(&headers, connect_info.map(|Extension(info)| info.0.ip())) {
        return error_response(StatusCode::FORBIDDEN, "websocket origin not allowed");
    }

    let workspace_id = match resolve_workspace_id(&state, &query).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    // Cookie auth branch — when the session cookie is present the identity
    // must fully validate (including membership) BEFORE the upgrade;
    // failures answer over plain HTTP exactly like Go's http.Error.
    let mut user_id = String::new();
    if let Some(token) = cookie_token(&headers) {
        let pr = DbPatResolver::new(
            state.pool.clone(),
            state.pat_cache.clone(),
            state.tasks.clone(),
        );
        let uid = match authenticate_token(&pr, &token) {
            Ok(uid) => uid,
            Err(payload) => return auth_http_error(payload),
        };
        if !is_member(&state, &uid, &workspace_id).await {
            return error_response(StatusCode::FORBIDDEN, "not a member of this workspace");
        }
        user_id = uid;
    }

    let meta = ClientMeta {
        platform: query.get("client_platform").cloned().unwrap_or_default(),
        version: query.get("client_version").cloned().unwrap_or_default(),
        os: query.get("client_os").cloned().unwrap_or_default(),
    };

    upgrade
        .max_message_size(INBOUND_READ_LIMIT)
        .max_frame_size(INBOUND_READ_LIMIT)
        .on_upgrade(move |socket| async move {
            post_upgrade(hub, state, socket, user_id, workspace_id, meta).await;
        })
}

fn check_origin(headers: &HeaderMap, remote_ip: Option<std::net::IpAddr>) -> bool {
    let trusted = std::env::var("PATCHBAY_TRUSTED_PROXIES")
        .ok()
        .map(|raw| patchbay_middleware::ratelimit::parse_trusted_proxies(&raw))
        .unwrap_or_default();
    let proxy_trusted =
        remote_ip.is_some_and(|ip| trusted.iter().any(|network| network.contains(ip)));
    check_origin_with_policy(headers, proxy_trusted, &websocket_allowed_origins())
}

fn websocket_allowed_origins() -> Vec<String> {
    std::env::var("ALLOWED_ORIGINS")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|origins| !origins.is_empty())
        .unwrap_or_else(crate::allowed_origins)
}

fn check_origin_with_policy(
    headers: &HeaderMap,
    proxy_trusted: bool,
    allowed_origins: &[String],
) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let Some(origin_host) = origin_host(origin) else {
        return false;
    };

    if headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|host| origin_host.eq_ignore_ascii_case(host))
    {
        return true;
    }

    if proxy_trusted
        && first_forwarded_host(headers).is_some_and(|host| origin_host.eq_ignore_ascii_case(host))
    {
        return true;
    }

    allowed_origins.iter().any(|allowed| allowed == origin)
}

fn origin_host(origin: &str) -> Option<String> {
    origin
        .parse::<axum::http::Uri>()
        .ok()?
        .authority()
        .map(|authority| authority.as_str().to_string())
}

fn first_forwarded_host(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-forwarded-host")?
        .to_str()
        .ok()?
        .split(',')
        .next()
        .map(str::trim)
        .filter(|host| !host.is_empty())
}

#[derive(Default, Clone)]
struct ClientMeta {
    platform: String,
    version: String,
    os: String,
}

async fn post_upgrade(
    hub: Arc<Hub>,
    state: HandlerState,
    mut socket: WebSocket,
    mut user_id: String,
    workspace_id: String,
    meta: ClientMeta,
) {
    // First-message auth (non-cookie clients): read ONE bounded frame before
    // anything else, mirroring Go's pre-auth SetReadLimit placement.
    if user_id.is_empty() {
        let token = match first_message_auth(&mut socket).await {
            Ok(t) => t,
            Err(None) => return, // socket already torn down
            Err(Some(payload)) => {
                write_auth_error_and_close(&mut socket, payload).await;
                return;
            }
        };
        let pr = DbPatResolver::new(
            state.pool.clone(),
            state.pat_cache.clone(),
            state.tasks.clone(),
        );
        let uid = match authenticate_token(&pr, &token) {
            Ok(uid) => uid,
            Err(payload) => {
                write_auth_error_and_close(&mut socket, payload).await;
                return;
            }
        };
        if !is_member(&state, &uid, &workspace_id).await {
            write_auth_error_and_close(
                &mut socket,
                r#"{"error":"not a member of this workspace"}"#,
            )
            .await;
            return;
        }
        user_id = uid.clone();
        if !send_direct(&mut socket, r#"{"type":"auth_ack"}"#).await {
            return;
        }
    }

    tracing::info!(
        user_id = %user_id,
        workspace_id = %workspace_id,
        client_platform = %meta.platform,
        client_version = %meta.version,
        client_os = %meta.os,
        "websocket connected"
    );

    let (client_id, mut rx, client_handle) = hub.register_with_handle(&user_id, &workspace_id);
    let Some(client) = client_handle else {
        hub.unregister(client_id);
        return;
    };

    // writePump owns the sink half; the reader keeps the stream half.
    let (mut sink, mut stream) = socket.split();
    let writer_user = user_id.clone();
    let writer_ws = workspace_id.clone();
    let writer = tokio::spawn(async move {
        write_pump(&mut sink, &mut rx, writer_user, writer_ws).await;
    });

    read_pump(&hub, &client, &mut stream).await;

    // Reader exited: stop the writer, then unregister (drops the hub's
    // ClientHandle → sender dropped → any late writer sees None).
    writer.abort();
    let _ = writer.await;
    hub.unregister(client_id);
}

// ---- pumps ---------------------------------------------------------------

/// Parses client frames and dispatches
/// subscribe/unsubscribe/ping. Any transport close breaks the loop.
async fn read_pump<S>(hub: &Arc<Hub>, client: &Arc<ClientHandle>, stream: &mut S)
where
    S: StreamExt<Item = Result<Message, axum::Error>> + Unpin,
{
    let liveness = tokio::time::sleep(Duration::from_secs(PONG_WAIT_SECS));
    tokio::pin!(liveness);
    loop {
        let msg = tokio::select! {
            () = &mut liveness => {
                tracing::warn!(
                    user_id = %client.user_id,
                    workspace_id = %client.workspace_id,
                    "ws: pong deadline exceeded"
                );
                break;
            }
            msg = stream.next() => msg,
        };
        let Some(msg) = msg else {
            break;
        };
        match msg {
            Ok(Message::Text(text)) => {
                liveness
                    .as_mut()
                    .reset(tokio::time::Instant::now() + Duration::from_secs(PONG_WAIT_SECS));
                if text.len() > INBOUND_READ_LIMIT {
                    M.inbound_too_large_total
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!(
                        limit_bytes = INBOUND_READ_LIMIT,
                        user_id = %client.user_id,
                        workspace_id = %client.workspace_id,
                        "ws: inbound frame exceeded read limit"
                    );
                    break;
                }
                handle_frame(hub, client, &text);
            }
            Ok(Message::Close(_)) | Err(_) => break,
            // Binary / Ping / Pong count as liveness; axum answers protocol
            // pings and the application intentionally ignores their payloads.
            Ok(_) => liveness
                .as_mut()
                .reset(tokio::time::Instant::now() + Duration::from_secs(PONG_WAIT_SECS)),
        }
    }
}

fn handle_frame(hub: &Arc<Hub>, client: &Arc<ClientHandle>, raw: &str) {
    let parsed: Result<Value, _> = serde_json::from_str(raw);
    let Ok(frame) = parsed else {
        tracing::debug!(user_id = %client.user_id, "ws inbound: invalid json");
        return;
    };
    let ftype = frame["type"].as_str().unwrap_or_default();
    match ftype {
        "subscribe" | "unsubscribe" => {
            let scope = frame["payload"]["scope"].as_str().unwrap_or("");
            let id = frame["payload"]["id"].as_str().unwrap_or("");
            if scope.is_empty() || id.is_empty() {
                try_send(
                    client,
                    json!({
                        "type": format!("{ftype}_error"),
                        "payload": {"scope": scope, "id": id, "error": "invalid payload"}
                    }),
                );
                return;
            }
            if ftype == "subscribe" {
                handle_subscribe(hub, client, scope, id);
            } else {
                hub.unsubscribe(client, scope, id);
                try_send(
                    client,
                    json!({
                        "type": "unsubscribe_ack",
                        "payload": {"scope": scope, "id": id}
                    }),
                );
            }
        }
        "ping" => try_send(client, json!({"type": "pong"})),
        other => {
            // Unknown frame — ignore silently for forward compat.
            tracing::debug!(frame = %other, user_id = %client.user_id, "ws inbound: unknown frame");
        }
    }
}

/// Handles subscriptions with an implicit-scope identity guard and
/// authorizer wiring + ack/error payloads.
fn handle_subscribe(hub: &Arc<Hub>, client: &Arc<ClientHandle>, scope: &str, id: &str) {
    match scope {
        SCOPE_WORKSPACE | SCOPE_USER => {
            let matches_identity = (scope == SCOPE_WORKSPACE && id == client.workspace_id)
                || (scope == SCOPE_USER && id == client.user_id);
            if !matches_identity {
                M.subscribe_denied_total(scope)
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                try_send(
                    client,
                    json!({
                        "type": "subscribe_error",
                        "payload": {"scope": scope, "id": id, "error": "forbidden"}
                    }),
                );
                return;
            }
            // Already auto-subscribed at connect time; reply ack idempotently.
            hub.subscribe(client, scope, id);
        }
        SCOPE_TASK | SCOPE_CHAT => {
            if let Err(reason) = hub.authorize_subscription(client, scope, id) {
                M.subscribe_denied_total(scope)
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                try_send(
                    client,
                    json!({
                        "type": "subscribe_error",
                        "payload": {"scope": scope, "id": id, "error": reason}
                    }),
                );
                return;
            }
            hub.subscribe(client, scope, id);
        }
        _ => {
            M.subscribe_denied_total(scope)
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            try_send(
                client,
                json!({
                    "type": "subscribe_error",
                    "payload": {"scope": scope, "id": id, "error": "unknown_scope"}
                }),
            );
            return;
        }
    }
    try_send(
        client,
        json!({
            "type": "subscribe_ack",
            "payload": {"scope": scope, "id": id}
        }),
    );
}

/// Best-effort enqueue (Go `sendJSON`): drops when the queue is full; the
/// slow client is evicted by the next broadcast cycle.
fn try_send(client: &Arc<ClientHandle>, v: Value) {
    let _ = client.sender.try_send(v.to_string().into_bytes());
}

async fn write_pump<S>(
    sink: &mut S,
    rx: &mut tokio::sync::mpsc::Receiver<Vec<u8>>,
    user_id: String,
    workspace_id: String,
) where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Display,
{
    let mut ping = tokio::time::interval(Duration::from_secs(PING_PERIOD_SECS));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.reset();
    loop {
        tokio::select! {
            item = rx.recv() => {
                match item {
                    Some(data) => {
                        let write = sink.send(Message::Text(
                            String::from_utf8_lossy(&data).into_owned().into(),
                        ));
                        match tokio::time::timeout(
                            Duration::from_secs(WRITE_WAIT_SECS), write,
                        ).await {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => {
                                tracing::warn!(error = %e, user_id = %user_id,
                                    workspace_id = %workspace_id, "websocket write error");
                                return;
                            }
                            Err(_) => {
                                tracing::warn!(user_id = %user_id,
                                    workspace_id = %workspace_id, "websocket write timeout");
                                return;
                            }
                        }
                    }
                    // Hub dropped our sender (unregister/evict) → Go writes an
                    // empty close message and returns.
                    None => {
                        let _ = sink.send(Message::Close(None)).await;
                        return;
                    }
                }
            }
            _ = ping.tick() => {
                if tokio::time::timeout(
                    Duration::from_secs(WRITE_WAIT_SECS),
                    sink.send(Message::Ping(Default::default())),
                ).await.map_or(true, |result| result.is_err()) {
                    return;
                }
            }
        }
    }
}

// ---- auth helpers ----------------------------------------------------------

/// Bridges the sync [`PatResolver`] trait onto async DB code for one token.
fn resolve_pat(pr: &DbPatResolver, token: &str) -> Option<String> {
    pr.resolve_token(token)
}

/// Validates a JWT or PAT string and returns the user ID — port of Go
/// `authenticateToken`. Error payloads are the exact JSON strings Go writes
/// back before closing.
fn authenticate_token(pr: &DbPatResolver, token: &str) -> Result<String, &'static str> {
    if token.starts_with("pbg_") || token.starts_with("pby_") {
        let Some(user_id) = resolve_pat(pr, token) else {
            return Err(r#"{"error":"invalid token"}"#);
        };
        if is_temporarily_disabled_user_id(&user_id) {
            return Err(r#"{"error":"account disabled"}"#);
        }
        return Ok(user_id);
    }

    let Some(claims) = decode_jwt_claims(token) else {
        return Err(r#"{"error":"invalid token"}"#);
    };
    let Some(sub) = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Err(r#"{"error":"invalid claims"}"#);
    };
    let email = claims.get("email").and_then(|v| v.as_str()).unwrap_or("");
    if is_temporarily_disabled_user(sub, email) {
        return Err(r#"{"error":"account disabled"}"#);
    }
    Ok(sub.to_string())
}

/// DB-backed PAT resolver sharing the middleware PAT cache — Rust analogue of
/// router.go's `patResolver`, so a revoke through any path invalidates all.
/// Nil cache degrades to direct DB lookups in Go; [`PatCache::disabled`] is
/// the equivalent no-op here.
pub struct DbPatResolver {
    pool: sqlx::PgPool,
    pat_cache: patchbay_auth::pat_cache::PatCache,
    side_effects: Arc<TaskService>,
}

impl DbPatResolver {
    pub fn new(
        pool: sqlx::PgPool,
        pat_cache: patchbay_auth::pat_cache::PatCache,
        side_effects: Arc<TaskService>,
    ) -> Self {
        Self {
            pool,
            pat_cache,
            side_effects,
        }
    }
}

impl patchbay_realtime::hub::PatResolver for DbPatResolver {
    fn resolve_token(&self, token: &str) -> Option<String> {
        // Sync trait (called once per connection, not per frame) bridged onto
        // the current runtime; bounded cost per Go's original design note.
        let pool = self.pool.clone();
        let cache = self.pat_cache.clone();
        let side_effects = self.side_effects.clone();
        let hash = hash_token(token);
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                // Guest bearer tokens are live session credentials, not PATs;
                // do not cache this result because logout revokes the session
                // row and the next connection must observe that immediately.
                if token.starts_with("pbg_") {
                    let session = guest::find_active_by_token_hash(&pool, &hash)
                        .await
                        .ok()??;
                    let guest_user = user::get_user(&pool, session.user_id).await.ok()??;
                    if !guest_user.is_guest {
                        return None;
                    }
                    return Some(guest_user.id.to_string());
                }

                if let Some(user_id) = cache.get(&hash).await {
                    return Some(user_id);
                }
                let pat = personal_access_token::get_personal_access_token_by_hash(&pool, &hash)
                    .await
                    .ok()??;
                let user_id = pat.user_id.to_string();
                let ttl =
                    patchbay_auth::pat_cache::ttl_for_expiry(chrono::Utc::now(), pat.expires_at);
                cache.set(&hash, &user_id, ttl).await;
                // Cache miss = first WS auth in this TTL window; refresh
                // last_used_at without blocking the handshake (Go does `go …`).
                let p2 = pool.clone();
                side_effects.spawn_side_effect(async move {
                    let _ =
                        personal_access_token::update_personal_access_token_last_used(&p2, pat.id)
                            .await;
                });
                Some(user_id)
            })
        })
    }
}

async fn is_member(state: &HandlerState, user_id: &str, workspace_id: &str) -> bool {
    let (Ok(uid), Ok(wid)) = (
        uuid::Uuid::parse_str(user_id),
        uuid::Uuid::parse_str(workspace_id),
    ) else {
        return false;
    };
    member::get_member_by_user_and_workspace(&state.pool, uid, wid)
        .await
        .map(|row| row.is_some())
        .unwrap_or(false)
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in cookies.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            if (k == AUTH_COOKIE_NAME || k == LEGACY_AUTH_COOKIE_NAME) && !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Plain-HTTP rejection before upgrade. Go picks 403 specifically for the
/// account-disabled body, 401 otherwise.
fn auth_http_error(payload: &'static str) -> Response {
    let status = if payload.contains("account disabled") {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::UNAUTHORIZED
    };
    (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        payload,
    )
        .into_response()
}

// ---- handshake helpers -----------------------------------------------------

/// Resolves workspace_id from query params — port of Go's
/// workspace_id → workspace_slug fallback (`SlugResolver`).
async fn resolve_workspace_id(
    state: &HandlerState,
    query: &HashMap<String, String>,
) -> Result<String, Response> {
    if let Some(id) = non_empty(query.get("workspace_id")) {
        return Ok(id.to_string());
    }
    if let Some(slug) = non_empty(query.get("workspace_slug")) {
        let Some(ws_row) = workspace::get_workspace_by_slug(&state.pool, slug)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "ws: slug lookup failed");
                None
            })
        else {
            return Err(error_response(StatusCode::NOT_FOUND, "workspace not found"));
        };
        return Ok(ws_row.id.to_string());
    }
    Err(error_response(
        StatusCode::BAD_REQUEST,
        "workspace_id or workspace_slug required",
    ))
}

fn non_empty(v: Option<&String>) -> Option<&str> {
    v.map(String::as_str).filter(|s| !s.is_empty())
}

/// Reads ONE inbound frame with a 10 s bound and extracts the token — port of
/// Go `firstMessageAuth`. `Err(None)` means the socket already tore down
/// (caller returns silently); `Err(Some(payload))` is an auth_error body.
async fn first_message_auth(socket: &mut WebSocket) -> Result<String, Option<&'static str>> {
    match tokio::time::timeout(Duration::from_secs(WRITE_WAIT_SECS), socket.recv()).await {
        Err(_) => Err(Some(r#"{"error":"auth timeout or read error"}"#)),
        Ok(None) => Err(None),
        Ok(Some(Err(_))) => Err(Some(r#"{"error":"auth timeout or read error"}"#)),
        Ok(Some(Ok(Message::Close(_)))) => Err(None),
        Ok(Some(Ok(Message::Text(text)))) => {
            if text.len() > INBOUND_READ_LIMIT {
                M.inbound_too_large_total
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tracing::warn!(
                    limit_bytes = INBOUND_READ_LIMIT,
                    "ws: pre-auth frame exceeded read limit"
                );
                // gorilla already replied CloseMessageTooBig (1009); writing
                // an auth_error past that would be data after the close frame.
                return Err(None);
            }
            let parsed: Result<Value, _> = serde_json::from_str(&text);
            let token: Option<String> = parsed.ok().and_then(|v| {
                let t = v["payload"]["token"].as_str().unwrap_or("").to_string();
                (v["type"] == "auth" && !t.is_empty()).then_some(t)
            });
            token.ok_or(Some(r#"{"error":"expected auth message as first frame"}"#))
        }
        Ok(Some(Ok(_))) => Err(Some(r#"{"error":"expected auth message as first frame"}"#)),
    }
}

/// Sends a text frame directly over the socket during the handshake phase
/// (before the pumps take ownership) — Go `writeWSAuthFrame`.
async fn send_direct(socket: &mut WebSocket, payload: &str) -> bool {
    socket
        .send(Message::Text(payload.to_owned().into()))
        .await
        .is_ok()
}

async fn write_auth_error_and_close(socket: &mut WebSocket, payload: &'static str) {
    send_direct(socket, payload).await;
    let _ = socket.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use patchbay_auth::pat_cache::PatCache;
    use patchbay_realtime::broadcaster::{SCOPE_TASK, SCOPE_WORKSPACE};
    use sqlx::postgres::PgPoolOptions;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    use tokio_tungstenite::tungstenite::Message as ClientMessage;
    use uuid::Uuid;

    fn headers(host: &str, origin: &str, forwarded_host: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_str(host).unwrap());
        headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        if let Some(forwarded_host) = forwarded_host {
            headers.insert(
                "x-forwarded-host",
                HeaderValue::from_str(forwarded_host).unwrap(),
            );
        }
        headers
    }

    fn origins() -> Vec<String> {
        vec![
            "http://localhost:3000".to_string(),
            "https://patchbay.aspectlylabs.com".to_string(),
        ]
    }

    #[test]
    fn websocket_origin_policy_matches_go_contract() {
        let allowed = origins();
        let empty = HeaderMap::new();
        assert!(check_origin_with_policy(&empty, false, &allowed));
        assert!(check_origin_with_policy(
            &headers("API.AspectlyLabs.Com", "https://api.aspectlylabs.com", None),
            false,
            &allowed,
        ));
        assert!(check_origin_with_policy(
            &headers("localhost:8080", "http://localhost:3000", None),
            false,
            &allowed,
        ));
        assert!(!check_origin_with_policy(
            &headers("api.aspectlylabs.com", "https://evil.example", None),
            false,
            &allowed,
        ));
    }

    #[test]
    fn forwarded_host_requires_a_trusted_proxy() {
        let allowed = origins();
        let headers = headers(
            "internal.proxy",
            "https://public.example",
            Some("public.example, proxy.internal"),
        );

        assert!(!check_origin_with_policy(&headers, false, &allowed));
        assert!(check_origin_with_policy(&headers, true, &allowed));
    }

    async fn client_json<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> Value
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        loop {
            match tokio::time::timeout(Duration::from_secs(2), socket.next())
                .await
                .expect("websocket response timeout")
                .expect("websocket closed")
                .expect("websocket frame")
            {
                ClientMessage::Text(text) => {
                    return serde_json::from_str(&text).expect("JSON websocket frame");
                }
                ClientMessage::Ping(payload) => socket
                    .send(ClientMessage::Pong(payload))
                    .await
                    .expect("answer protocol ping"),
                ClientMessage::Close(frame) => panic!("unexpected close: {frame:?}"),
                _ => {}
            }
        }
    }

    async fn wait_for_disconnect(hub: &Hub) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if hub.snapshot()["connections"] == 0 {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("hub disconnect cleanup");
    }

    async fn wait_for_client_close<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match socket.next().await {
                    Some(Ok(ClientMessage::Close(_))) | Some(Err(_)) | None => return,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await
        .expect("websocket close deadline");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn production_websocket_session_auth_scope_wire_and_cleanup() {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required for production websocket contract");
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&url)
            .await
            .expect("connect contract PostgreSQL");
        let suffix = Uuid::now_v7().simple().to_string();
        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO \"user\" (name, email) VALUES ('ws member', $1) RETURNING id",
        )
        .bind(format!("ws-member-{suffix}@example.test"))
        .fetch_one(&pool)
        .await
        .expect("create member user");
        let outsider_id: Uuid = sqlx::query_scalar(
            "INSERT INTO \"user\" (name, email) VALUES ('ws outsider', $1) RETURNING id",
        )
        .bind(format!("ws-outsider-{suffix}@example.test"))
        .fetch_one(&pool)
        .await
        .expect("create outsider user");
        let workspace_slug = format!("ws-contract-{suffix}");
        let workspace_id: Uuid = sqlx::query_scalar(
            "INSERT INTO workspace (name, slug) VALUES ('ws contract', $1) RETURNING id",
        )
        .bind(&workspace_slug)
        .fetch_one(&pool)
        .await
        .expect("create workspace");
        sqlx::query("INSERT INTO member (workspace_id, user_id, role) VALUES ($1, $2, 'member')")
            .bind(workspace_id)
            .bind(user_id)
            .execute(&pool)
            .await
            .expect("create membership");

        let member_token = format!("pby_ws_member_{suffix}");
        let outsider_token = format!("pby_ws_outsider_{suffix}");
        let member_jwt = patchbay_auth::jwt::issue_user_jwt(
            &user_id.to_string(),
            &format!("ws-member-{suffix}@example.test"),
            "ws member",
        )
        .expect("issue member JWT");
        for (token, owner) in [(&member_token, user_id), (&outsider_token, outsider_id)] {
            sqlx::query(
                "INSERT INTO personal_access_token (user_id, name, token_hash, token_prefix) VALUES ($1, 'ws contract', $2, $3)",
            )
            .bind(owner)
            .bind(hash_token(token))
            .bind(&token[..token.len().min(12)])
            .execute(&pool)
            .await
            .expect("create PAT");
        }

        let runtime_id: Uuid = sqlx::query_scalar(
            "INSERT INTO agent_runtime (workspace_id, name, runtime_mode, provider, status) VALUES ($1, 'ws runtime', 'local', 'test', 'online') RETURNING id",
        )
        .bind(workspace_id)
        .fetch_one(&pool)
        .await
        .expect("create runtime");
        let agent_id: Uuid = sqlx::query_scalar(
            "INSERT INTO agent (workspace_id, name, runtime_mode, runtime_id) VALUES ($1, 'ws agent', 'local', $2) RETURNING id",
        )
        .bind(workspace_id)
        .bind(runtime_id)
        .fetch_one(&pool)
        .await
        .expect("create agent");
        let chat_session_id: Uuid = sqlx::query_scalar(
            "INSERT INTO chat_session (workspace_id, agent_id, creator_id, title) VALUES ($1, $2, $3, 'ws chat') RETURNING id",
        )
        .bind(workspace_id)
        .bind(agent_id)
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .expect("create chat session");
        let issue_id: Uuid = sqlx::query_scalar(
            "INSERT INTO issue (workspace_id, title, status, priority, creator_type, creator_id, number, position) VALUES ($1, 'ws issue', 'todo', 'none', 'member', $2, 1, -1) RETURNING id",
        )
        .bind(workspace_id)
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .expect("create issue");
        let task_id: Uuid = sqlx::query_scalar(
            "INSERT INTO agent_task_queue (agent_id, issue_id, runtime_id, status, priority) VALUES ($1, $2, $3, 'queued', 0) RETURNING id",
        )
        .bind(agent_id)
        .bind(issue_id)
        .bind(runtime_id)
        .fetch_one(&pool)
        .await
        .expect("create task");

        let hub = Arc::new(Hub::new());
        let state = HandlerState::new(pool.clone(), PatCache::disabled(), Some(hub.clone()));
        let app = crate::build_router_from_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback websocket server");
        let address = listener.local_addr().expect("loopback address");
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve websocket contract");
        });
        let ws_url = format!("ws://{address}/ws?workspace_id={workspace_id}");

        let mut bad_origin = ws_url
            .clone()
            .into_client_request()
            .expect("origin request");
        bad_origin.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("https://foreign.example"),
        );
        let error = tokio_tungstenite::connect_async(bad_origin)
            .await
            .expect_err("cross-origin websocket must fail before upgrade");
        match error {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                assert_eq!(response.status(), StatusCode::FORBIDDEN)
            }
            other => panic!("unexpected cross-origin failure: {other}"),
        }

        let (mut malformed, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect("malformed-auth websocket upgrade");
        malformed
            .send(ClientMessage::Text(
                json!({"type":"ping"}).to_string().into(),
            ))
            .await
            .expect("send non-auth first frame");
        assert_eq!(
            client_json(&mut malformed).await,
            json!({"error":"expected auth message as first frame"})
        );
        wait_for_client_close(&mut malformed).await;
        wait_for_disconnect(&hub).await;

        let mut outsider_cookie = ws_url
            .clone()
            .into_client_request()
            .expect("outsider cookie request");
        outsider_cookie.headers_mut().insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{AUTH_COOKIE_NAME}={outsider_token}"))
                .expect("outsider cookie header"),
        );
        match tokio_tungstenite::connect_async(outsider_cookie)
            .await
            .expect_err("cookie outsider must be rejected before upgrade")
        {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                assert_eq!(response.status(), StatusCode::FORBIDDEN)
            }
            other => panic!("unexpected cookie outsider failure: {other}"),
        }

        let (mut outsider, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect("outsider websocket upgrade");
        outsider
            .send(ClientMessage::Text(
                json!({"type":"auth","payload":{"token":outsider_token}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("send outsider auth");
        assert_eq!(
            client_json(&mut outsider).await,
            json!({"error":"not a member of this workspace"})
        );
        wait_for_disconnect(&hub).await;

        let (mut jwt_socket, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect("JWT websocket upgrade");
        jwt_socket
            .send(ClientMessage::Text(
                json!({"type":"auth","payload":{"token":member_jwt}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("send JWT auth");
        assert_eq!(
            client_json(&mut jwt_socket).await,
            json!({"type":"auth_ack"})
        );
        jwt_socket.close(None).await.expect("close JWT websocket");
        wait_for_disconnect(&hub).await;

        let (mut socket, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect("member websocket upgrade");
        socket
            .send(ClientMessage::Text(
                json!({"type":"auth","payload":{"token":member_token}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("send member auth");
        assert_eq!(client_json(&mut socket).await, json!({"type":"auth_ack"}));
        assert_eq!(hub.snapshot()["connections"], 1);
        assert!(hub.has_local_subscribers(SCOPE_WORKSPACE, &workspace_id.to_string()));
        assert!(hub.has_local_subscribers(SCOPE_USER, &user_id.to_string()));

        // A connection in another workspace must not receive broadcasts from
        // this workspace, even when it authenticates as the same user.
        let workspace_two_slug = format!("ws-contract-two-{suffix}");
        let workspace_two: Uuid = sqlx::query_scalar(
            "INSERT INTO workspace (name, slug) VALUES ('ws contract two', $1) RETURNING id",
        )
        .bind(&workspace_two_slug)
        .fetch_one(&pool)
        .await
        .expect("create second workspace");
        sqlx::query("INSERT INTO member (workspace_id, user_id, role) VALUES ($1, $2, 'member')")
            .bind(workspace_two)
            .bind(user_id)
            .execute(&pool)
            .await
            .expect("create second workspace membership");
        let foreign_runtime_id: Uuid = sqlx::query_scalar(
            "INSERT INTO agent_runtime (workspace_id, name, runtime_mode, provider, status) VALUES ($1, 'foreign ws runtime', 'local', 'test', 'online') RETURNING id",
        )
        .bind(workspace_two)
        .fetch_one(&pool)
        .await
        .expect("create foreign workspace runtime");
        let foreign_agent_id: Uuid = sqlx::query_scalar(
            "INSERT INTO agent (workspace_id, name, runtime_mode, runtime_id) VALUES ($1, 'foreign ws agent', 'local', $2) RETURNING id",
        )
        .bind(workspace_two)
        .bind(foreign_runtime_id)
        .fetch_one(&pool)
        .await
        .expect("create foreign workspace agent");
        let foreign_issue_id: Uuid = sqlx::query_scalar(
            "INSERT INTO issue (workspace_id, title, status, priority, creator_type, creator_id, number, position) VALUES ($1, 'foreign ws issue', 'todo', 'none', 'member', $2, 1, -1) RETURNING id",
        )
        .bind(workspace_two)
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .expect("create foreign workspace issue");
        let foreign_task_id: Uuid = sqlx::query_scalar(
            "INSERT INTO agent_task_queue (agent_id, issue_id, runtime_id, status, priority) VALUES ($1, $2, $3, 'queued', 0) RETURNING id",
        )
        .bind(foreign_agent_id)
        .bind(foreign_issue_id)
        .bind(foreign_runtime_id)
        .fetch_one(&pool)
        .await
        .expect("create foreign workspace task");
        let ws_two_url = format!("ws://{address}/ws?workspace_id={workspace_two}");
        let (mut socket_two, _) = tokio_tungstenite::connect_async(&ws_two_url)
            .await
            .expect("second workspace websocket upgrade");
        socket_two
            .send(ClientMessage::Text(
                json!({"type":"auth","payload":{"token":member_token}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("authenticate second workspace socket");
        assert_eq!(client_json(&mut socket_two).await["type"], "auth_ack");
        socket_two
            .send(ClientMessage::Text(
                json!({"type":"subscribe","payload":{"scope":"workspace","id":workspace_two}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("subscribe second workspace");
        assert_eq!(client_json(&mut socket_two).await["type"], "subscribe_ack");
        hub.broadcast_to_scope_dedup(
            SCOPE_WORKSPACE,
            &workspace_id.to_string(),
            br#"{"type":"foreign-workspace:event"}"#,
            "",
        );
        assert_eq!(
            client_json(&mut socket).await,
            json!({"type":"foreign-workspace:event"})
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), socket_two.next())
                .await
                .is_err()
        );
        socket_two
            .close(None)
            .await
            .expect("close second workspace socket");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if hub.snapshot()["connections"] == 1 {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("second workspace disconnect cleanup");

        socket
            .send(ClientMessage::Text(
                json!({"type":"subscribe","payload":{"scope":"workspace","id":workspace_id}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("subscribe own workspace");
        assert_eq!(client_json(&mut socket).await["type"], "subscribe_ack");
        socket
            .send(ClientMessage::Text(
                json!({"type":"subscribe","payload":{"scope":"workspace","id":Uuid::now_v7()}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("subscribe foreign workspace");
        let workspace_error = client_json(&mut socket).await;
        assert_eq!(workspace_error["type"], "subscribe_error");
        assert_eq!(workspace_error["payload"]["error"], "forbidden");

        socket
            .send(ClientMessage::Text(
                json!({"type":"subscribe","payload":{"scope":"task","id":task_id}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("subscribe owned task");
        assert_eq!(client_json(&mut socket).await["type"], "subscribe_ack");
        assert!(hub.has_local_subscribers(SCOPE_TASK, &task_id.to_string()));
        socket
            .send(ClientMessage::Text(
                json!({"type":"unsubscribe","payload":{"scope":"task","id":task_id}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("unsubscribe owned task");
        assert_eq!(client_json(&mut socket).await["type"], "unsubscribe_ack");
        assert!(!hub.has_local_subscribers(SCOPE_TASK, &task_id.to_string()));
        socket
            .send(ClientMessage::Text(
                json!({"type":"subscribe","payload":{"scope":"task","id":foreign_task_id}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("subscribe foreign task");
        let task_error = client_json(&mut socket).await;
        assert_eq!(task_error["type"], "subscribe_error");
        assert_eq!(task_error["payload"]["error"], "forbidden");

        socket
            .send(ClientMessage::Text(
                json!({"type":"subscribe","payload":{"scope":"chat","id":chat_session_id}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("subscribe owned chat");
        assert_eq!(client_json(&mut socket).await["type"], "subscribe_ack");
        socket
            .send(ClientMessage::Text(
                json!({"type":"subscribe","payload":{"scope":"chat","id":Uuid::now_v7()}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("subscribe foreign chat");
        let chat_error = client_json(&mut socket).await;
        assert_eq!(chat_error["type"], "subscribe_error");
        assert_eq!(chat_error["payload"]["error"], "forbidden");

        socket
            .send(ClientMessage::Text(
                json!({"type":"subscribe","payload":{"scope":"user","id":outsider_id}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("subscribe foreign user");
        let user_error = client_json(&mut socket).await;
        assert_eq!(user_error["type"], "subscribe_error");
        assert_eq!(user_error["payload"]["error"], "forbidden");

        socket
            .send(ClientMessage::Text(
                json!({"type":"ping"}).to_string().into(),
            ))
            .await
            .expect("application ping");
        assert_eq!(client_json(&mut socket).await, json!({"type":"pong"}));
        hub.broadcast_to_scope_dedup(
            SCOPE_WORKSPACE,
            &workspace_id.to_string(),
            br#"{"type":"contract:event","payload":{"ok":true}}"#,
            "",
        );
        assert_eq!(client_json(&mut socket).await["type"], "contract:event");

        socket
            .send(ClientMessage::Text(
                json!({"type":"subscribe","payload":{"scope":"task","id":task_id}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("subscribe task before disconnect cleanup");
        assert_eq!(client_json(&mut socket).await["type"], "subscribe_ack");
        socket.close(None).await.expect("close member websocket");
        wait_for_disconnect(&hub).await;
        assert!(!hub.has_local_subscribers(SCOPE_TASK, &task_id.to_string()));

        let (mut oversized, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect("oversized-frame websocket upgrade");
        oversized
            .send(ClientMessage::Text(
                json!({"type":"auth","payload":{"token":member_token}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("oversized-frame auth");
        assert_eq!(
            client_json(&mut oversized).await,
            json!({"type":"auth_ack"})
        );
        let oversized_payload = "x".repeat(INBOUND_READ_LIMIT + 1);
        let _ = oversized
            .send(ClientMessage::Text(oversized_payload.into()))
            .await;
        wait_for_client_close(&mut oversized).await;
        wait_for_disconnect(&hub).await;

        let cookie_url = format!("ws://{address}/ws?workspace_slug={workspace_slug}");
        let mut cookie_request = cookie_url.into_client_request().expect("cookie request");
        cookie_request.headers_mut().insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{AUTH_COOKIE_NAME}={member_token}"))
                .expect("cookie header"),
        );
        let (mut cookie_socket, _) = tokio_tungstenite::connect_async(cookie_request)
            .await
            .expect("cookie-auth websocket");
        cookie_socket
            .send(ClientMessage::Text(
                json!({"type":"ping"}).to_string().into(),
            ))
            .await
            .expect("cookie session ping");
        assert_eq!(
            client_json(&mut cookie_socket).await,
            json!({"type":"pong"})
        );
        cookie_socket
            .close(None)
            .await
            .expect("close cookie socket");
        wait_for_disconnect(&hub).await;

        let jwt_token = patchbay_auth::jwt::issue_user_jwt(
            &user_id.to_string(),
            &format!("ws-member-{suffix}@example.test"),
            "ws member",
        )
        .expect("issue websocket JWT");
        let (mut jwt_socket, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect("JWT websocket upgrade");
        jwt_socket
            .send(ClientMessage::Text(
                json!({"type":"auth","payload":{"token":jwt_token}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("send JWT auth");
        assert_eq!(
            client_json(&mut jwt_socket).await,
            json!({"type":"auth_ack"})
        );
        jwt_socket.close(None).await.expect("close JWT socket");
        wait_for_disconnect(&hub).await;

        let _ = shutdown_tx.send(());
        server.await.expect("websocket server task");
        sqlx::query("DELETE FROM agent_task_queue WHERE id = ANY($1)")
            .bind(vec![task_id, foreign_task_id])
            .execute(&pool)
            .await
            .expect("delete tasks");
        sqlx::query("DELETE FROM issue WHERE id = ANY($1)")
            .bind(vec![issue_id, foreign_issue_id])
            .execute(&pool)
            .await
            .expect("delete issues");
        sqlx::query("DELETE FROM agent WHERE id = ANY($1)")
            .bind(vec![agent_id, foreign_agent_id])
            .execute(&pool)
            .await
            .expect("delete agents");
        sqlx::query("DELETE FROM personal_access_token WHERE user_id = ANY($1)")
            .bind(vec![user_id, outsider_id])
            .execute(&pool)
            .await
            .expect("delete PATs");
        sqlx::query("DELETE FROM member WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("delete membership");
        sqlx::query("DELETE FROM workspace WHERE id = $1")
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("delete workspace");
        sqlx::query("DELETE FROM workspace WHERE id = $1")
            .bind(workspace_two)
            .execute(&pool)
            .await
            .expect("delete second workspace");
        sqlx::query("DELETE FROM \"user\" WHERE id = ANY($1)")
            .bind(vec![user_id, outsider_id])
            .execute(&pool)
            .await
            .expect("delete users");
    }
}
