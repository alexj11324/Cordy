//! The shared, channel-agnostic inbound entry point.
//!
//! Port of `server/internal/integrations/channel/handler.go`.

use std::sync::Arc;

use crate::message::InboundMessage;

/// The shared, channel-agnostic entry point a Channel invokes for every
/// inbound message it receives. The engine supervisor injects ONE
/// handler into every Channel it builds (via
/// [`Config::handler`](crate::channel::Config::handler)), mirroring the
/// reference design's single set_message_handler wiring: the engine's
/// inbound processing is written once and every platform adapter funnels
/// into it. The adapter owns its receive loop and calls the handler; the
/// core never polls the Channel.
///
/// Contract:
///
/// - A non-nil error signals an INFRASTRUCTURE failure (the core could
///   not process the message at all — DB down, dispatcher
///   misconfigured, etc.). The adapter should treat it like a failed
///   delivery: surface it to ops and let the supervisor's
///   reconnect/backoff handle it. It MUST NOT be used for product
///   outcomes.
/// - A nil error means the message was accepted and classified. The
///   message may still have been dropped for a legitimate product reason
///   (dedup hit, unbound sender, group filter) — that is NOT an error.
///   Any outbound reply the verdict implies (binding card, offline
///   notice, typing indicator) is the handler's own responsibility,
///   detached from the adapter's ACK path.
///
/// The handler is deliberately fire-and-classify (no return value beyond
/// error): the adapter does not branch on the outcome, so coupling it to
/// a platform-specific result type would defeat the abstraction.
///
/// Port note: Go's `func(ctx, InboundMessage) error` becomes a cloneable
/// `Arc<dyn AsyncFn…>`-shaped alias; adapters clone the handle into
/// their receive-loop tasks.
#[derive(Clone)]
pub struct InboundHandler(Arc<dyn InboundHandlerFut>);

trait InboundHandlerFut:
    Fn(tokio_util::sync::CancellationToken, InboundMessage) -> HandlerFuture + Send + Sync
{
}
impl<T> InboundHandlerFut for T where
    T: Fn(tokio_util::sync::CancellationToken, InboundMessage) -> HandlerFuture + Send + Sync
{
}

/// Port of Go `context.Context`: the engine passes a
/// [`CancellationToken`] so adapters can observe shutdown while the
/// handler runs.
pub type HandlerFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>;

impl InboundHandler {
    /// Wraps an async closure into the shared handler handle.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(tokio_util::sync::CancellationToken, InboundMessage) -> HandlerFuture
            + Send
            + Sync
            + 'static,
    {
        Self(Arc::new(f))
    }

    /// Invokes the handler. A thin `call` keeps adapter receive loops
    /// readable.
    pub async fn call(
        &self,
        ctx: tokio_util::sync::CancellationToken,
        msg: InboundMessage,
    ) -> anyhow::Result<()> {
        (self.0)(ctx, msg).await
    }
}

impl std::fmt::Debug for InboundHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InboundHandler")
    }
}
