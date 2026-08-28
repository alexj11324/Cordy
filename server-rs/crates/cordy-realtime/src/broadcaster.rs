//! Producer-facing realtime abstractions and the `RelayPublisher` interface.

use async_trait::async_trait;

/// Scope types recognised by the broadcaster. Producers and consumers should
/// use these constants rather than raw strings so a typo can never silently
/// route an event to a non-existent room.
pub const SCOPE_WORKSPACE: &str = "workspace";
pub const SCOPE_USER: &str = "user";
pub const SCOPE_TASK: &str = "task";
pub const SCOPE_CHAT: &str = "chat";
/// Routes daemon wakeup frames through the Redis relay. Consumed by the
/// daemon WebSocket hub, not by browser clients.
pub const SCOPE_DAEMON_RUNTIME: &str = "daemon_runtime";

/// The abstraction every realtime event producer should depend on instead of
/// the concrete Hub.
///
/// Phase 1 (PB-1138) extends the surface with [`Broadcaster::broadcast_to_scope`]
/// so events can be fanned out to high-frequency per-resource scopes
/// (`task:{id}`, `chat:{id}`) instead of the whole workspace. The legacy
/// methods continue to work and route through it under the hood.
///
/// Methods are async because concrete relays perform network I/O; the Hub's
/// channel-based implementation completes without blocking.
#[async_trait]
pub trait Broadcaster: Send + Sync {
    /// Fans a message out to every connection currently subscribed to
    /// `{scopeType, scopeID}` on this node.
    async fn broadcast_to_scope(&self, scope_type: &str, scope_id: &str, message: &[u8]);

    /// Back-compat shortcut for `BroadcastToScope("workspace", ...)`.
    async fn broadcast_to_workspace(&self, workspace_id: &str, message: &[u8]) {
        self.broadcast_to_scope(SCOPE_WORKSPACE, workspace_id, message)
            .await;
    }

    /// Back-compat shortcut for `BroadcastToScope("user", ...)`. The optional
    /// `exclude_workspace` argument is preserved for the `member:added` dedup
    /// path: connections whose workspace matches are skipped.
    async fn send_to_user(&self, user_id: &str, message: &[u8], exclude_workspace: Option<&str>);

    /// Fans a message out to every connection on this node. Used for
    /// daemon:* events that have no workspace scope.
    async fn broadcast(&self, message: &[u8]);
}

/// Consumes daemon-runtime scoped relay frames.
pub trait DaemonRuntimeDeliverer: Send + Sync {
    fn deliver_daemon_runtime(&self, scope_id: &str, frame: &[u8], event_id: &str);
}

/// Implemented by Redis relay backends that can publish a caller-supplied
/// event id for local/Redis loopback deduplication.
///
/// Async because implementations perform Redis network I/O (Go blocks the
/// calling goroutine instead).
#[async_trait]
pub trait RelayPublisher: Send + Sync {
    async fn publish_with_id(
        &self,
        scope_type: &str,
        scope_id: &str,
        exclude: &str,
        frame: &[u8],
        event_id: &str,
    ) -> anyhow::Result<()>;
}
