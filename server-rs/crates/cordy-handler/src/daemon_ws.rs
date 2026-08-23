//! Daemon WebSocket pump — port of `Handler.DaemonWebSocket`
//! (server/internal/handler/daemon_ws.go) plus the axum socket lane for the
//! daemon hub (`internal/daemonws` readPump/writePump).
//!
//! Contract notes (must stay identical with Go):
//! - Query params: deduped `runtime_id` / `runtime_ids` (comma-split), 400 when
//!   neither runtime ids nor a user identity are present.
//! - Per-runtime workspace authorization before upgrade; a runtime bound to a
//!   different daemon than the token's is a 404.
//! - Frames: `daemon:heartbeat` → `daemon:heartbeat_ack`,
//!   `daemon:rpc_request` → `daemon:rpc_response`. Unknown types ignored.
//! - Read limit 64 KiB, pong deadline 60 s (any inbound frame counts as
//!   liveness — axum auto-replies protocol pings), server ping every 54 s.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Extension, Query as AxumQuery, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use cordy_daemon::hub::{ClientIdentity, DaemonHub};
use cordy_middleware::daemon_auth::DaemonContext;
use futures_util::{SinkExt, StreamExt};

use crate::error::error_response;
use crate::state::HandlerState;

const PONG_WAIT_SECS: u64 = 60;
const PING_PERIOD_SECS: u64 = PONG_WAIT_SECS * 9 / 10;

/// GET /api/daemon/ws — pre-upgrade identity resolution + upgrade + pumps.
///
/// Port of `DaemonWebSocket`: identity validation runs before the upgrade (400
/// when no runtime ids and no user), runtime/workspace authorization is
/// enforced per runtime, and the resolved identity scopes the connection. A
/// missing hub reports 503 so older daemons fall back to HTTP polling.
pub async fn daemon_ws_handler(
    State(state): State<HandlerState>,
    AxumQuery(query): AxumQuery<std::collections::HashMap<String, String>>,
    daemon_ext: Option<Extension<DaemonContext>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(hub) = state.daemon_hub.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "daemon websocket unavailable",
        );
    };

    // Collect deduped runtime ids from both spellings (Go parseRuntimeIDs).
    let mut runtime_ids: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for raw_key in ["runtime_id", "runtime_ids"] {
        if let Some(raw) = query.get(raw_key) {
            for part in raw.split(',') {
                let id = part.trim();
                if !id.is_empty() && seen.insert(id.to_string()) {
                    runtime_ids.push(id.to_string());
                }
            }
        }
    }
    let user_id = request_user_id(&headers);
    if runtime_ids.is_empty() && user_id.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "runtime_ids or user identity required",
        );
    }

    let daemon_ctx = daemon_ext.map(|Extension(ctx)| ctx);

    let access = Access::new(&state, &headers);
    let mut workspace_ids: Vec<String> = Vec::new();
    let mut seen_ws: HashSet<String> = HashSet::new();
    for rid in &runtime_ids {
        let (rt, ws_id) =
            match require_daemon_runtime_access(&access, daemon_ctx.clone(), rid).await {
                Ok(v) => v,
                Err(res) => return res,
            };
        let token_daemon_id = daemon_id_of(daemon_ctx.clone());
        if !token_daemon_id.is_empty() && rt.daemon_id.as_deref().unwrap_or("") != token_daemon_id {
            return error_response(StatusCode::NOT_FOUND, "runtime not found");
        }
        if seen_ws.insert(ws_id.clone()) {
            workspace_ids.push(ws_id);
        }
    }

    let primary_workspace = workspace_ids.first().cloned().unwrap_or_default();
    let identity = ClientIdentity {
        daemon_id: daemon_id_of(daemon_ctx.clone()),
        user_id,
        workspace_id: primary_workspace,
        workspace_ids,
        runtime_ids,
        client_version: headers
            .get("x-client-version")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string(),
        capabilities: headers
            .get("x-client-capabilities")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string(),
    };

    upgrade
        .protocols(["daemon"])
        .write_buffer_size(0)
        .max_message_size(cordy_daemon::hub::MAX_FRAME_READ_BYTES)
        .max_frame_size(cordy_daemon::hub::MAX_FRAME_READ_BYTES)
        .on_upgrade(move |socket| async move {
            serve_daemon_socket(hub, identity, socket).await;
        })
}

// Re-use the shared helpers from the daemon module.
use crate::daemon::{daemon_id_of, request_user_id, require_daemon_runtime_access, Access};

/// Port of Go `Hub.HandleWebSocket` + client readPump/writePump on axum's split
/// sink/stream.
async fn serve_daemon_socket(hub: Arc<DaemonHub>, identity: ClientIdentity, socket: WebSocket) {
    if let Err(err) = DaemonHub::validate_identity(&identity) {
        // validate_identity already ran pre-upgrade; this is defense in depth.
        tracing::debug!(error = %err, "daemon websocket identity rejected");
        return;
    }

    let (client, mut rx): (Arc<cordy_daemon::hub::DaemonClient>, _) =
        hub.register(identity.clone());

    let (mut sink, mut stream) = socket.split();

    // writePump owns the outbound queue; ping frames keep the pong deadline
    // honest (axum auto-replies inbound pings).
    let writer_client = client.clone();
    let writer = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(PING_PERIOD_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                frame = rx.recv() => {
                    match frame {
                        Some(frame) => {
                            if tokio::time::timeout(
                                cordy_daemon::hub::WRITE_WAIT,
                                sink.send(Message::Binary(Bytes::from(frame))),
                            ).await.map_or(true, |result| result.is_err()) {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = ticker.tick() => {
                    if tokio::time::timeout(
                        cordy_daemon::hub::WRITE_WAIT,
                        sink.send(Message::Ping(Bytes::new())),
                    ).await.map_or(true, |result| result.is_err()) {
                        break;
                    }
                }
            }
        }
        let _ = writer_client;
    });

    // readPump: parse frames and dispatch through the hub's shared bookkeeping.
    loop {
        let msg = match tokio::time::timeout(cordy_daemon::hub::PONG_WAIT, stream.next()).await {
            Ok(Some(msg)) => msg,
            Ok(None) | Err(_) => break,
        };
        let raw = match msg {
            Ok(Message::Text(text)) => Some(text.as_bytes().to_vec()),
            Ok(Message::Binary(bytes)) => Some(bytes.to_vec()),
            Ok(Message::Ping(_) | Message::Pong(_)) => None,
            Ok(Message::Close(_)) => break,
            Err(e) => {
                tracing::debug!(error = %e, daemon_id = %identity.daemon_id, "daemon websocket read error");
                break;
            }
        };
        let Some(raw) = raw else { continue };
        if raw.len() > cordy_daemon::hub::MAX_FRAME_READ_BYTES {
            tracing::debug!(daemon_id = %identity.daemon_id, "daemon websocket frame over 64 KiB read limit");
            break;
        }
        hub.handle_frame(&client, &raw).await;
    }

    // Reader exited: stop the writer, then unregister (cancels connection
    // context so async RPC handlers stop too).
    writer.abort();
    hub.unregister(client.id);
}
