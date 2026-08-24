//! Installation-scoped, cross-replica WeCom outbound RPC over Redis Streams.
//!
//! WeCom has no stateless outbound API: only the process holding an
//! installation's WebSocket can send. This relay advertises that process with
//! a short-lived owner key, routes a durable request to its node stream, and
//! correlates the result through a bounded Redis result key. A stable request
//! id plus a claim key makes replay idempotent. Requests may move to a new
//! owner only before any holder has claimed them; after a possible socket
//! write, a missing result is deliberately ambiguous and is never retried.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use redis::AsyncCommands as _;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::senders_registry::SendersRegistry;
use crate::ws_sender::{is_ack_timeout, is_write_attempted};

const OWNER_TTL: Duration = Duration::from_secs(8);
const OWNER_REFRESH: Duration = Duration::from_secs(2);
const READ_BLOCK: Duration = Duration::from_secs(1);
const RESULT_TTL: Duration = Duration::from_secs(10 * 60);
const CLAIM_TTL: Duration = Duration::from_secs(6 * 60);
const STREAM_TTL: Duration = Duration::from_secs(60 * 60);
const STREAM_MAX_LEN: i64 = 2_000;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_REQUEST_BYTES: usize = 256 * 1024;
const REDIS_OP_TIMEOUT: Duration = Duration::from_secs(1);
const REDIS_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const RESULT_STORE_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Default)]
pub struct RelayMetricsSnapshot {
    pub published: u64,
    pub publish_errors: u64,
    pub received: u64,
    pub completed: u64,
    pub replayed: u64,
    pub owner_misses: u64,
    pub rollovers: u64,
    pub claim_handoffs: u64,
    pub timeouts: u64,
    pub ambiguous: u64,
    pub transport_errors: u64,
}

#[derive(Default)]
struct RelayMetrics {
    published: AtomicU64,
    publish_errors: AtomicU64,
    received: AtomicU64,
    completed: AtomicU64,
    replayed: AtomicU64,
    owner_misses: AtomicU64,
    rollovers: AtomicU64,
    claim_handoffs: AtomicU64,
    timeouts: AtomicU64,
    ambiguous: AtomicU64,
    transport_errors: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayEvent {
    pub event_type: String,
    pub workspace_id: String,
    pub actor_type: String,
    pub actor_id: String,
    pub payload: serde_json::Value,
    pub task_id: String,
    pub chat_session_id: String,
}

impl From<&cordy_events::Event> for RelayEvent {
    fn from(event: &cordy_events::Event) -> Self {
        Self {
            event_type: event.event_type.clone(),
            workspace_id: event.workspace_id.clone(),
            actor_type: event.actor_type.clone(),
            actor_id: event.actor_id.clone(),
            payload: event.payload.clone(),
            task_id: event.task_id.clone(),
            chat_session_id: event.chat_session_id.clone(),
        }
    }
}

impl From<RelayEvent> for cordy_events::Event {
    fn from(event: RelayEvent) -> Self {
        Self {
            event_type: event.event_type,
            workspace_id: event.workspace_id,
            actor_type: event.actor_type,
            actor_id: event.actor_id,
            payload: event.payload,
            task_id: event.task_id,
            chat_session_id: event.chat_session_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RelayPayload {
    Text {
        chat_id: String,
        chat_type: i64,
        text: String,
    },
    ChatDone {
        event: RelayEvent,
    },
    InboxNew {
        event: RelayEvent,
    },
    Attachments {
        message_id: Uuid,
        workspace_id: Uuid,
        chat_id: String,
        chat_type: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayRequest {
    version: u8,
    request_id: String,
    source_node: String,
    installation_id: Uuid,
    expires_at_ms: u64,
    payload: RelayPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayResult {
    request_id: String,
    status: RelayStatus,
    error: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RelayStatus {
    Delivered,
    Failed,
    Unknown,
    Expired,
}

#[derive(Debug, thiserror::Error)]
#[error("wecom outbound relay result is unknown: {0}")]
pub struct RelayAmbiguous(pub String);

pub trait RelayClock: Send + Sync {
    fn unix_millis(&self) -> u64;
}

#[derive(Debug, Default)]
pub struct SystemRelayClock;

impl RelayClock for SystemRelayClock {
    fn unix_millis(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

#[async_trait]
pub trait RelayEventHandler: Send + Sync {
    async fn handle_chat_done(&self, event: cordy_events::Event) -> anyhow::Result<()>;
    async fn handle_inbox_new(&self, event: cordy_events::Event) -> anyhow::Result<()>;
    async fn handle_attachments(
        &self,
        installation_id: Uuid,
        message_id: Uuid,
        workspace_id: Uuid,
        chat_id: String,
        chat_type: i64,
    ) -> anyhow::Result<()>;
}

/// One process's relay endpoint. Every replica creates one only when Redis is
/// configured; single-replica/no-Redis deployments keep the direct registry
/// path and do not pay for relay traffic.
pub struct OutboundRelay {
    client: redis::Client,
    connection: tokio::sync::OnceCell<redis::aio::ConnectionManager>,
    namespace: String,
    node_id: String,
    senders: Arc<SendersRegistry>,
    clock: Arc<dyn RelayClock>,
    metrics: RelayMetrics,
}

impl OutboundRelay {
    pub fn new(
        redis_url: &str,
        namespace: &str,
        senders: Arc<SendersRegistry>,
    ) -> anyhow::Result<Self> {
        Self::new_with_clock(redis_url, namespace, senders, Arc::new(SystemRelayClock))
    }

    pub fn new_with_clock(
        redis_url: &str,
        namespace: &str,
        senders: Arc<SendersRegistry>,
        clock: Arc<dyn RelayClock>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            client: redis::Client::open(redis_url)?,
            connection: tokio::sync::OnceCell::new(),
            namespace: if namespace.trim().is_empty() {
                "cordy:wecom:outbound".to_string()
            } else {
                namespace.trim().trim_end_matches(':').to_string()
            },
            node_id: Uuid::now_v7().to_string(),
            senders,
            clock,
            metrics: RelayMetrics::default(),
        })
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    pub fn metrics(&self) -> RelayMetricsSnapshot {
        RelayMetricsSnapshot {
            published: self.metrics.published.load(Ordering::Relaxed),
            publish_errors: self.metrics.publish_errors.load(Ordering::Relaxed),
            received: self.metrics.received.load(Ordering::Relaxed),
            completed: self.metrics.completed.load(Ordering::Relaxed),
            replayed: self.metrics.replayed.load(Ordering::Relaxed),
            owner_misses: self.metrics.owner_misses.load(Ordering::Relaxed),
            rollovers: self.metrics.rollovers.load(Ordering::Relaxed),
            claim_handoffs: self.metrics.claim_handoffs.load(Ordering::Relaxed),
            timeouts: self.metrics.timeouts.load(Ordering::Relaxed),
            ambiguous: self.metrics.ambiguous.load(Ordering::Relaxed),
            transport_errors: self.metrics.transport_errors.load(Ordering::Relaxed),
        }
    }

    pub fn start(
        self: &Arc<Self>,
        handler: Arc<dyn RelayEventHandler>,
        cancel: CancellationToken,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let ownership = Arc::clone(self);
        let ownership_cancel = cancel.clone();
        let ownership_task = tokio::spawn(async move {
            ownership.ownership_loop(ownership_cancel).await;
        });
        let consumer = Arc::clone(self);
        let consumer_task = tokio::spawn(async move {
            consumer.consumer_loop(handler, cancel).await;
        });
        vec![ownership_task, consumer_task]
    }

    pub async fn send_text(
        &self,
        ctx: &CancellationToken,
        installation_id: Uuid,
        chat_id: &str,
        chat_type: i64,
        text: &str,
    ) -> anyhow::Result<()> {
        self.request(
            ctx,
            installation_id,
            RelayPayload::Text {
                chat_id: chat_id.to_string(),
                chat_type,
                text: text.to_string(),
            },
            Duration::from_secs(10),
        )
        .await
    }

    pub async fn forward_chat_done(
        &self,
        ctx: &CancellationToken,
        installation_id: Uuid,
        event: &cordy_events::Event,
    ) -> anyhow::Result<()> {
        self.request(
            ctx,
            installation_id,
            RelayPayload::ChatDone {
                event: RelayEvent::from(event),
            },
            Duration::from_secs(10),
        )
        .await
    }

    pub async fn forward_inbox_new(
        &self,
        ctx: &CancellationToken,
        installation_id: Uuid,
        event: &cordy_events::Event,
    ) -> anyhow::Result<()> {
        self.request(
            ctx,
            installation_id,
            RelayPayload::InboxNew {
                event: RelayEvent::from(event),
            },
            Duration::from_secs(10),
        )
        .await
    }

    pub async fn forward_attachments(
        &self,
        ctx: &CancellationToken,
        installation_id: Uuid,
        message_id: Uuid,
        workspace_id: Uuid,
        chat_id: &str,
        chat_type: i64,
    ) -> anyhow::Result<()> {
        self.request(
            ctx,
            installation_id,
            RelayPayload::Attachments {
                message_id,
                workspace_id,
                chat_id: chat_id.to_string(),
                chat_type,
            },
            Duration::from_secs(10),
        )
        .await
    }

    async fn request(
        &self,
        ctx: &CancellationToken,
        installation_id: Uuid,
        payload: RelayPayload,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let request_id = Uuid::now_v7().to_string();
        let deadline = tokio::time::Instant::now() + timeout;
        let request = RelayRequest {
            version: 1,
            request_id: request_id.clone(),
            source_node: self.node_id.clone(),
            installation_id,
            expires_at_ms: self
                .clock
                .unix_millis()
                .saturating_add(timeout.as_millis() as u64),
            payload,
        };
        let encoded = serde_json::to_string(&request)?;
        if encoded.len() > MAX_REQUEST_BYTES {
            anyhow::bail!("wecom outbound relay request exceeds {MAX_REQUEST_BYTES} bytes");
        }
        let mut published_to = HashSet::new();
        let mut claim_seen = false;
        let mut may_have_published = false;
        let mut previous_owner = None;

        loop {
            if ctx.is_cancelled() {
                return self.interrupted_result(&request_id, may_have_published || claim_seen);
            }
            if tokio::time::Instant::now() >= deadline {
                self.metrics.timeouts.fetch_add(1, Ordering::Relaxed);
                if claim_seen || may_have_published {
                    self.metrics.ambiguous.fetch_add(1, Ordering::Relaxed);
                    return Err(RelayAmbiguous(request_id).into());
                }
                anyhow::bail!("wecom outbound relay timed out before a holder claimed the request");
            }
            match redis_op(self.read_result(&request_id)).await {
                Ok(Some(result)) => return self.decode_result(result),
                Ok(None) => {}
                Err(error) => {
                    self.record_transport_error("read result", &error);
                    if !wait_request(ctx).await {
                        return self
                            .interrupted_result(&request_id, may_have_published || claim_seen);
                    }
                    continue;
                }
            }
            match redis_op(self.claim_exists(&request_id)).await {
                // Claims outlive the ten-second request by several minutes,
                // so a claim that disappears during this loop was explicitly
                // CAS-released by a holder that proved it had not written.
                // Clear the sticky observation and allow successor routing.
                Ok(exists) => claim_seen = exists,
                Err(error) => {
                    self.record_transport_error("read claim", &error);
                    if !wait_request(ctx).await {
                        return self
                            .interrupted_result(&request_id, may_have_published || claim_seen);
                    }
                    continue;
                }
            }
            if !claim_seen {
                match redis_op(self.owner(installation_id)).await {
                    Ok(Some((route, node))) => {
                        if previous_owner.as_ref().is_some_and(|old| old != &route) {
                            self.metrics.rollovers.fetch_add(1, Ordering::Relaxed);
                        }
                        previous_owner = Some(route.clone());
                        // Track the generation-qualified owner, not only the
                        // node. A socket may reconnect on the same process;
                        // that successor must receive a request its previous
                        // generation rejected before claiming.
                        if published_to.insert(route.clone()) {
                            // XADD can succeed server-side while its response
                            // is lost. From this point cancellation/timeout is
                            // conservative unless the durable result says
                            // otherwise.
                            may_have_published = true;
                            if let Err(error) =
                                redis_op(self.publish(&node, &route, &request_id, &encoded)).await
                            {
                                published_to.remove(&route);
                                self.record_transport_error("publish request", &error);
                            }
                        }
                    }
                    Ok(None) => {
                        self.metrics.owner_misses.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(error) => self.record_transport_error("resolve owner", &error),
                }
            }
            tokio::select! {
                _ = ctx.cancelled() => return self.interrupted_result(&request_id, may_have_published || claim_seen),
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
            }
        }
    }

    async fn publish(
        &self,
        owner: &str,
        route: &str,
        request_id: &str,
        payload: &str,
    ) -> anyhow::Result<()> {
        let stream = self.node_stream(owner);
        let mut conn = self.connection().await?;
        let result: redis::RedisResult<String> = redis::cmd("XADD")
            .arg(&stream)
            .arg("MAXLEN")
            .arg("~")
            .arg(STREAM_MAX_LEN)
            .arg("*")
            .arg("request_id")
            .arg(request_id)
            .arg("route")
            .arg(route)
            .arg("payload")
            .arg(payload)
            .query_async(&mut conn)
            .await;
        match result {
            Ok(_) => {
                self.metrics.published.fetch_add(1, Ordering::Relaxed);
                let _: redis::RedisResult<bool> =
                    conn.expire(&stream, STREAM_TTL.as_secs() as i64).await;
                Ok(())
            }
            Err(error) => {
                self.metrics.publish_errors.fetch_add(1, Ordering::Relaxed);
                Err(error.into())
            }
        }
    }

    async fn consumer_loop(
        self: Arc<Self>,
        handler: Arc<dyn RelayEventHandler>,
        cancel: CancellationToken,
    ) {
        let stream = self.node_stream(&self.node_id);
        let group = format!("node:{}", self.node_id);
        while !cancel.is_cancelled() {
            // XREADGROUP BLOCK must have a dedicated socket. Sharing the
            // multiplexed command connection would stall owner heartbeats,
            // publishes and result polling behind the blocking read.
            let mut conn = match tokio::time::timeout(
                REDIS_CONNECT_TIMEOUT,
                self.client.get_multiplexed_async_connection(),
            )
            .await
            {
                Ok(Ok(conn)) => conn,
                Ok(Err(error)) => {
                    tracing::warn!(%error, "wecom outbound relay: Redis connection failed");
                    sleep_or_cancel(&cancel, Duration::from_secs(1)).await;
                    continue;
                }
                Err(_) => {
                    tracing::warn!("wecom outbound relay: Redis connection timed out");
                    sleep_or_cancel(&cancel, Duration::from_secs(1)).await;
                    continue;
                }
            };
            let created: redis::RedisResult<()> = redis::cmd("XGROUP")
                .arg("CREATE")
                .arg(&stream)
                .arg(&group)
                // The node id is unique per process. Starting at zero closes
                // the tiny publish-before-group-create window without
                // replaying another process's old traffic.
                .arg("0")
                .arg("MKSTREAM")
                .query_async(&mut conn)
                .await;
            if let Err(error) = created {
                if !error.to_string().contains("BUSYGROUP") {
                    tracing::warn!(%error, "wecom outbound relay: create consumer group failed");
                    sleep_or_cancel(&cancel, Duration::from_secs(1)).await;
                    continue;
                }
            }
            // Recover this consumer's unacked request first (for example a
            // result SET failed after execution). Only block on new traffic
            // when the pending list is empty.
            let pending: redis::RedisResult<redis::Value> = redis::cmd("XREADGROUP")
                .arg("GROUP")
                .arg(&group)
                .arg(&self.node_id)
                .arg("COUNT")
                .arg(32)
                .arg("STREAMS")
                .arg(&stream)
                .arg("0")
                .query_async(&mut conn)
                .await;
            let read = match pending {
                Ok(value)
                    if !cordy_realtime::sharded_stream_relay::parse_xread_response(&value)
                        .is_empty() =>
                {
                    Ok(value)
                }
                Ok(_) => {
                    redis::cmd("XREADGROUP")
                        .arg("GROUP")
                        .arg(&group)
                        .arg(&self.node_id)
                        .arg("COUNT")
                        .arg(32)
                        .arg("BLOCK")
                        .arg(READ_BLOCK.as_millis() as usize)
                        .arg("STREAMS")
                        .arg(&stream)
                        .arg(">")
                        .query_async(&mut conn)
                        .await
                }
                Err(error) => Err(error),
            };
            match read {
                Ok(value) => {
                    for (_, messages) in
                        cordy_realtime::sharded_stream_relay::parse_xread_response(&value)
                    {
                        for (message_id, fields) in messages {
                            let fields: HashMap<_, _> = fields.into_iter().collect();
                            if let Some(payload) = fields.get("payload") {
                                self.metrics.received.fetch_add(1, Ordering::Relaxed);
                                if !self
                                    .consume_one(
                                        &handler,
                                        payload,
                                        fields.get("route").map(String::as_str),
                                    )
                                    .await
                                {
                                    continue;
                                }
                            }
                            let _: redis::RedisResult<i64> = redis::cmd("XACK")
                                .arg(&stream)
                                .arg(&group)
                                .arg(message_id)
                                .query_async(&mut conn)
                                .await;
                        }
                    }
                }
                Err(error) if !cancel.is_cancelled() => {
                    tracing::warn!(%error, "wecom outbound relay: read failed");
                    sleep_or_cancel(&cancel, Duration::from_secs(1)).await;
                }
                Err(_) => {}
            }
        }
    }

    async fn consume_one(
        &self,
        handler: &Arc<dyn RelayEventHandler>,
        raw: &str,
        target_route: Option<&str>,
    ) -> bool {
        if raw.len() > MAX_REQUEST_BYTES {
            tracing::warn!(bytes = raw.len(), "wecom outbound relay: oversized request");
            return true;
        }
        let request: RelayRequest = match serde_json::from_str(raw) {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!(%error, "wecom outbound relay: invalid request");
                return true;
            }
        };
        if request.version != 1 {
            return true;
        }
        let Some((sender, local_route)) = self.local_sender_route(request.installation_id) else {
            match target_route {
                Some(route) => self.clear_owner_route(request.installation_id, route).await,
                None => self.clear_stale_owner(request.installation_id).await,
            }
            return true;
        };
        let target_route = target_route.unwrap_or(&local_route);
        if target_route != local_route.as_str() {
            // This copy was published to a predecessor generation. It has not
            // claimed or written anything, so acknowledge it and leave the
            // source free to publish the same request to the successor.
            self.clear_owner_route(request.installation_id, target_route)
                .await;
            return true;
        }
        match redis_op(self.read_result(&request.request_id)).await {
            Ok(Some(_)) => {
                self.metrics.replayed.fetch_add(1, Ordering::Relaxed);
                return true;
            }
            Ok(None) => {}
            Err(error) => {
                self.record_transport_error("consumer read result", &error);
                return false;
            }
        }
        if !redis_op(self.claim(&request.request_id, target_route))
            .await
            .unwrap_or(false)
        {
            self.metrics.replayed.fetch_add(1, Ordering::Relaxed);
            // Another stream copy (possibly on the rollover successor) owns
            // the execution. Do not acknowledge this copy until its durable
            // result exists: executing again could duplicate a WS send.
            tokio::time::sleep(POLL_INTERVAL).await;
            return false;
        }
        if !self.route_is_current(request.installation_id, target_route) {
            return self
                .release_unwritten_claim(&request.request_id, target_route)
                .await;
        }
        let result = if request.expires_at_ms <= self.clock.unix_millis() {
            RelayResult {
                request_id: request.request_id.clone(),
                status: RelayStatus::Expired,
                error: "request expired before execution".to_string(),
            }
        } else {
            let remaining = Duration::from_millis(
                request
                    .expires_at_ms
                    .saturating_sub(self.clock.unix_millis())
                    .max(1),
            );
            match tokio::time::timeout(remaining, self.execute(handler, &request, sender)).await {
                Ok(Ok(())) => RelayResult {
                    request_id: request.request_id.clone(),
                    status: RelayStatus::Delivered,
                    error: String::new(),
                },
                Ok(Err(error))
                    if !is_write_attempted(&error)
                        && !is_ack_timeout(&error)
                        && !self.route_is_current(request.installation_id, target_route) =>
                {
                    return self
                        .release_unwritten_claim(&request.request_id, target_route)
                        .await;
                }
                Ok(Err(error)) => RelayResult {
                    request_id: request.request_id.clone(),
                    status: if is_write_attempted(&error) || is_ack_timeout(&error) {
                        RelayStatus::Unknown
                    } else {
                        RelayStatus::Failed
                    },
                    error: format!("{error:#}"),
                },
                Err(_) => RelayResult {
                    request_id: request.request_id.clone(),
                    status: RelayStatus::Unknown,
                    error: "holder execution exceeded request deadline".to_string(),
                },
            }
        };
        for attempt in 1..=RESULT_STORE_ATTEMPTS {
            match redis_op(self.store_result(&result)).await {
                Ok(()) => {
                    self.metrics.completed.fetch_add(1, Ordering::Relaxed);
                    return true;
                }
                Err(error) => {
                    self.record_transport_error("store result", &error);
                    tracing::warn!(
                        %error,
                        attempt,
                        request_id = %request.request_id,
                        "wecom outbound relay: store result failed"
                    );
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            }
        }
        false
    }

    async fn execute(
        &self,
        handler: &Arc<dyn RelayEventHandler>,
        request: &RelayRequest,
        sender: Arc<crate::ws_sender::WsSender>,
    ) -> anyhow::Result<()> {
        match &request.payload {
            RelayPayload::Text {
                chat_id,
                chat_type,
                text,
            } => {
                sender
                    .send_text_ctx(&CancellationToken::new(), chat_id, *chat_type, text)
                    .await
            }
            RelayPayload::ChatDone { event } => {
                handler.handle_chat_done(event.clone().into()).await
            }
            RelayPayload::InboxNew { event } => {
                handler.handle_inbox_new(event.clone().into()).await
            }
            RelayPayload::Attachments {
                message_id,
                workspace_id,
                chat_id,
                chat_type,
            } => {
                handler
                    .handle_attachments(
                        request.installation_id,
                        *message_id,
                        *workspace_id,
                        chat_id.clone(),
                        *chat_type,
                    )
                    .await
            }
        }
    }

    async fn ownership_loop(self: Arc<Self>, cancel: CancellationToken) {
        let mut advertised: HashMap<Uuid, String> = HashMap::new();
        loop {
            if cancel.is_cancelled() {
                break;
            }
            let snapshot = self.senders.ownership_snapshot();
            let current: HashMap<_, _> = snapshot.into_iter().collect();
            for (installation_id, generation) in &current {
                if let Err(error) = redis_op(self.advertise(*installation_id, generation)).await {
                    tracing::warn!(%error, %installation_id, "wecom outbound relay: owner heartbeat failed");
                }
            }
            for (installation_id, generation) in &advertised {
                if !current.contains_key(installation_id) {
                    self.clear_owner(*installation_id, generation).await;
                }
            }
            advertised = current;
            sleep_or_cancel(&cancel, OWNER_REFRESH).await;
        }
        for (installation_id, generation) in advertised {
            self.clear_owner(installation_id, &generation).await;
        }
    }

    async fn advertise(&self, installation_id: Uuid, generation: &str) -> anyhow::Result<()> {
        let key = self.owner_key(installation_id);
        let value = format!("{generation}|{}", self.node_id);
        let script = redis::Script::new(
            "local cur=redis.call('GET',KEYS[1]); if (not cur) or string.sub(cur,1,36) <= ARGV[2] then redis.call('PSETEX',KEYS[1],ARGV[1],ARGV[3]); return 1 end; return 0",
        );
        let mut conn = self.connection().await?;
        let _: i64 = script
            .key(key)
            .arg(OWNER_TTL.as_millis() as u64)
            .arg(generation)
            .arg(value)
            .invoke_async(&mut conn)
            .await?;
        Ok(())
    }

    async fn clear_owner(&self, installation_id: Uuid, generation: &str) {
        let value = format!("{generation}|{}", self.node_id);
        self.clear_owner_route(installation_id, &value).await;
    }

    async fn clear_owner_route(&self, installation_id: Uuid, route: &str) {
        let key = self.owner_key(installation_id);
        let script = redis::Script::new(
            "if redis.call('GET',KEYS[1]) == ARGV[1] then return redis.call('DEL',KEYS[1]) end return 0",
        );
        let operation = async {
            let mut conn = self.connection().await?;
            let _: i64 = script.key(key).arg(route).invoke_async(&mut conn).await?;
            Ok::<(), anyhow::Error>(())
        };
        let _ = redis_op(operation).await;
    }

    async fn clear_stale_owner(&self, installation_id: Uuid) {
        let key = self.owner_key(installation_id);
        let suffix = format!("|{}", self.node_id);
        let script = redis::Script::new(
            "local cur=redis.call('GET',KEYS[1]); if cur and string.sub(cur,-string.len(ARGV[1])) == ARGV[1] then return redis.call('DEL',KEYS[1]) end return 0",
        );
        let operation = async {
            let mut conn = self.connection().await?;
            let _: i64 = script.key(key).arg(suffix).invoke_async(&mut conn).await?;
            Ok::<(), anyhow::Error>(())
        };
        let _ = redis_op(operation).await;
    }

    async fn owner(&self, installation_id: Uuid) -> anyhow::Result<Option<(String, String)>> {
        let mut conn = self.connection().await?;
        let value: Option<String> = conn.get(self.owner_key(installation_id)).await?;
        let Some(value) = value else {
            return Ok(None);
        };
        let Some((_, node)) = value.split_once('|') else {
            return Ok(None);
        };
        let node = node.to_string();
        Ok(Some((value, node)))
    }

    async fn claim(&self, request_id: &str, route: &str) -> anyhow::Result<bool> {
        let mut conn = self.connection().await?;
        let reply: Option<String> = redis::cmd("SET")
            .arg(self.claim_key(request_id))
            .arg(route)
            .arg("NX")
            .arg("PX")
            .arg(CLAIM_TTL.as_millis() as u64)
            .query_async(&mut conn)
            .await?;
        Ok(reply.is_some())
    }

    async fn release_claim(&self, request_id: &str, route: &str) -> anyhow::Result<bool> {
        let script = redis::Script::new(
            "if redis.call('GET',KEYS[1]) == ARGV[1] then redis.call('DEL',KEYS[1]); return 1 end return 0",
        );
        let mut conn = self.connection().await?;
        let released: i64 = script
            .key(self.claim_key(request_id))
            .arg(route)
            .invoke_async(&mut conn)
            .await?;
        Ok(released == 1)
    }

    async fn release_unwritten_claim(&self, request_id: &str, route: &str) -> bool {
        match redis_op(self.release_claim(request_id, route)).await {
            Ok(true) => {
                self.metrics.claim_handoffs.fetch_add(1, Ordering::Relaxed);
                true
            }
            Ok(false) => {
                tracing::warn!(
                    request_id,
                    route,
                    "wecom outbound relay: unwritten claim changed before release"
                );
                false
            }
            Err(error) => {
                self.record_transport_error("release unwritten claim", &error);
                false
            }
        }
    }

    async fn claim_exists(&self, request_id: &str) -> anyhow::Result<bool> {
        let mut conn = self.connection().await?;
        Ok(conn.exists(self.claim_key(request_id)).await?)
    }

    fn local_sender_route(
        &self,
        installation_id: Uuid,
    ) -> Option<(Arc<crate::ws_sender::WsSender>, String)> {
        self.senders
            .get_routed(installation_id)
            .map(|(sender, generation)| (sender, format!("{generation}|{}", self.node_id)))
    }

    fn route_is_current(&self, installation_id: Uuid, route: &str) -> bool {
        self.local_sender_route(installation_id)
            .is_some_and(|(_, current)| current == route)
    }

    async fn store_result(&self, result: &RelayResult) -> anyhow::Result<()> {
        let encoded = serde_json::to_string(result)?;
        let mut conn = self.connection().await?;
        let _: () = redis::cmd("SET")
            .arg(self.result_key(&result.request_id))
            .arg(encoded)
            .arg("PX")
            .arg(RESULT_TTL.as_millis() as u64)
            .query_async(&mut conn)
            .await?;
        Ok(())
    }

    async fn read_result(&self, request_id: &str) -> anyhow::Result<Option<RelayResult>> {
        let mut conn = self.connection().await?;
        let raw: Option<String> = conn.get(self.result_key(request_id)).await?;
        raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
            .transpose()
    }

    fn decode_result(&self, result: RelayResult) -> anyhow::Result<()> {
        match result.status {
            RelayStatus::Delivered => Ok(()),
            RelayStatus::Failed | RelayStatus::Expired => {
                anyhow::bail!("wecom outbound relay failed: {}", result.error)
            }
            RelayStatus::Unknown => {
                self.metrics.ambiguous.fetch_add(1, Ordering::Relaxed);
                Err(RelayAmbiguous(result.error).into())
            }
        }
    }

    fn record_transport_error(&self, operation: &str, error: &anyhow::Error) {
        self.metrics
            .transport_errors
            .fetch_add(1, Ordering::Relaxed);
        tracing::warn!(%error, operation, "wecom outbound relay: Redis operation failed; retrying");
    }

    fn interrupted_result(&self, request_id: &str, ambiguous: bool) -> anyhow::Result<()> {
        if ambiguous {
            self.metrics.ambiguous.fetch_add(1, Ordering::Relaxed);
            Err(RelayAmbiguous(request_id.to_string()).into())
        } else {
            anyhow::bail!("wecom outbound relay cancelled before publish")
        }
    }

    async fn connection(&self) -> anyhow::Result<redis::aio::ConnectionManager> {
        Ok(self
            .connection
            .get_or_try_init(|| self.client.get_connection_manager())
            .await?
            .clone())
    }

    fn owner_key(&self, installation_id: Uuid) -> String {
        format!("{}:owner:{installation_id}", self.namespace)
    }
    fn node_stream(&self, node_id: &str) -> String {
        format!("{}:node:{node_id}", self.namespace)
    }
    fn claim_key(&self, request_id: &str) -> String {
        format!("{}:claim:{request_id}", self.namespace)
    }
    fn result_key(&self, request_id: &str) -> String {
        format!("{}:result:{request_id}", self.namespace)
    }
}

async fn sleep_or_cancel(cancel: &CancellationToken, duration: Duration) {
    tokio::select! {
        _ = cancel.cancelled() => {}
        _ = tokio::time::sleep(duration) => {}
    }
}

async fn wait_request(cancel: &CancellationToken) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(POLL_INTERVAL) => true,
    }
}

async fn redis_op<T>(
    future: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    tokio::time::timeout(REDIS_OP_TIMEOUT, future)
        .await
        .map_err(|_| anyhow::anyhow!("Redis operation timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    fn sender() -> Arc<crate::ws_sender::WsSender> {
        struct NoConn;

        #[async_trait]
        impl crate::ws_sender::WsConn for NoConn {
            async fn read_message(&self, _: Option<std::time::Instant>) -> anyhow::Result<Vec<u8>> {
                anyhow::bail!("no reads")
            }

            async fn write_message(
                &self,
                _: String,
                _: Option<std::time::Instant>,
            ) -> anyhow::Result<()> {
                anyhow::bail!("no writes")
            }

            async fn close(&self) {}
        }

        Arc::new(crate::ws_sender::WsSender::new(Arc::new(NoConn)))
    }

    #[test]
    fn event_envelope_round_trips_without_losing_routing_fields() {
        let event = cordy_events::Event {
            event_type: "chat:done".into(),
            workspace_id: Uuid::now_v7().to_string(),
            actor_type: "agent".into(),
            actor_id: Uuid::now_v7().to_string(),
            payload: serde_json::json!({"content": "done"}),
            task_id: Uuid::now_v7().to_string(),
            chat_session_id: Uuid::now_v7().to_string(),
        };
        let restored: cordy_events::Event = RelayEvent::from(&event).into();
        assert_eq!(restored.event_type, event.event_type);
        assert_eq!(restored.workspace_id, event.workspace_id);
        assert_eq!(restored.actor_type, event.actor_type);
        assert_eq!(restored.actor_id, event.actor_id);
        assert_eq!(restored.payload, event.payload);
        assert_eq!(restored.task_id, event.task_id);
        assert_eq!(restored.chat_session_id, event.chat_session_id);
    }

    #[test]
    fn keys_are_namespaced_and_installation_scoped() {
        let relay = OutboundRelay::new(
            "redis://127.0.0.1/",
            "test:relay:",
            Arc::new(SendersRegistry::new()),
        )
        .unwrap();
        let installation = Uuid::now_v7();
        assert_eq!(
            relay.owner_key(installation),
            format!("test:relay:owner:{installation}")
        );
        assert_eq!(
            relay.node_stream(relay.node_id()),
            format!("test:relay:node:{}", relay.node_id())
        );
    }

    #[test]
    fn ambiguous_result_is_not_reported_as_a_retryable_failure() {
        let relay =
            OutboundRelay::new("redis://127.0.0.1/", "", Arc::new(SendersRegistry::new())).unwrap();
        let error = relay
            .decode_result(RelayResult {
                request_id: "request-1".into(),
                status: RelayStatus::Unknown,
                error: "write attempted, ack absent".into(),
            })
            .unwrap_err();
        assert!(error.downcast_ref::<RelayAmbiguous>().is_some());
        assert_eq!(relay.metrics().ambiguous, 1);
    }

    #[test]
    fn local_routes_are_bound_to_the_exact_sender_generation() {
        let senders = Arc::new(SendersRegistry::new());
        let relay = OutboundRelay::new("redis://127.0.0.1/", "", senders.clone()).unwrap();
        let installation = Uuid::now_v7();
        let old = cordy_channel::LeaseGeneration::standalone();
        senders.set(installation, sender(), old.clone());
        let old_route = format!("{}|{}", old.epoch(), relay.node_id());
        assert!(relay.route_is_current(installation, &old_route));

        let new = cordy_channel::LeaseGeneration::standalone();
        senders.set(installation, sender(), new.clone());
        let new_route = format!("{}|{}", new.epoch(), relay.node_id());
        assert!(!relay.route_is_current(installation, &old_route));
        assert!(relay.route_is_current(installation, &new_route));
    }
}
