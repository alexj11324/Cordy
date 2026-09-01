//! The Channel contract: the platform-agnostic interface every IM
//! integration implements.
//!

use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

use crate::generation::LeaseGeneration;

/// Identifies an inbound channel platform — the discriminator the
/// [`crate::registry::Registry`] keys on and the value persisted in the
/// `channel_type` column of the generalized channel_* tables. Use the
/// lower-case platform slug ("feishu", "slack", "wecom", …); keep it
/// stable, it is durable data.
///
/// Port note: Go uses a string type; Rust keeps the open-string newtype
/// so unknown future platforms round-trip without a schema change.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Type(pub String);

impl Type {
    /// The Feishu / Lark adapter. It serves both the mainland Feishu
    /// cloud and the Lark international cloud; the cloud (region) is
    /// per-installation config, not a separate Type.
    pub fn feishu() -> Type {
        Type("feishu".to_string())
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The platform-agnostic contract every IM integration implements.
///
/// An adapter keeps ALL platform specifics behind these methods: the core
/// supervisor calls [`connect`](Self::connect)/[`disconnect`](Self::disconnect)
/// to manage the link, [`send`](Self::send) to deliver an outbound reply,
/// and reads [`capabilities`](Self::capabilities) to decide how to
/// render; it never touches platform SDKs or wire formats.
///
/// Inbound is intentionally NOT on this trait. A Channel pushes
/// normalized [`InboundMessage`](crate::message::InboundMessage) values
/// into the core router via the wiring established at construction (the
/// adapter owns its receive loop); the core does not poll the Channel.
///
/// Port note: Go's `Connect(ctx) error` returns nil on graceful ctx
/// cancellation and an error when the link drops. Rust folds both into
/// `Result`: adapters return `Ok(())` on cancellation (mirroring the nil
/// return) and `Err` for a dropped link — the supervisor treats `Err` as
/// "this attempt failed" and reconnects under backoff.
#[async_trait]
pub trait Channel: Send + Sync {
    /// Reports the platform discriminator. It MUST equal the
    /// [`Type`] the Channel was registered under, and is stable for the
    /// lifetime of the instance.
    fn r#type(&self) -> Type;

    /// Establishes the platform link (e.g. dials the outbound WebSocket
    /// long-conn, or starts the inbound HTTP listener) and then runs the
    /// receive loop until the link ends. The connection mode is the
    /// implementation's choice and invisible to the core.
    ///
    /// While connect runs, the adapter delivers each inbound message by
    /// invoking the [`InboundHandler`](crate::handler::InboundHandler) it
    /// captured at construction ([`Config::handler`]). send may be called
    /// concurrently from another task for the lifetime of the
    /// connection. Implementations MUST tolerate repeated connect calls
    /// on different contexts: the supervisor may connect, return, and
    /// connect again after backoff.
    async fn connect(&self, ctx: tokio_util::sync::CancellationToken) -> anyhow::Result<()>;

    /// Tears the platform link down and releases its resources. It is
    /// safe to call after a failed connect and safe to call more than
    /// once; a Channel that is already disconnected returns `Ok(())`.
    async fn disconnect(&self) -> anyhow::Result<()>;

    /// Delivers a single outbound message and returns the platform's
    /// identifier for the delivered message. An `Err` is reserved for
    /// real delivery failures (network, auth, rate limit) that the caller
    /// may retry.
    async fn send(
        &self,
        out: crate::message::OutboundMessage,
    ) -> anyhow::Result<crate::message::SendResult>;

    /// Declares what this Channel supports. It is a pure declaration with
    /// no side effects and a stable result; callers read it to choose a
    /// rendering and degrade on their own.
    fn capabilities(&self) -> crate::capability::Capability;
}

/// The normalized per-installation configuration a
/// [`Factory`] consumes.
///
/// `raw` is the platform's own credential/config blob (Feishu's app_id /
/// encrypted app_secret / tenant_key / region, Slack's bot/app tokens,
/// …), carried opaquely so the foundation never grows a per-platform
/// field. It maps directly onto the channel_type column + JSONB config of
/// a channel_installation row (PB-3515 decision §3).
#[derive(Clone, Default)]
pub struct Config {
    /// The platform discriminator.
    pub r#type: Type,
    /// The platform's opaque credential/config blob.
    pub raw: Value,

    /// The `channel_installation.id` row this Channel is being built
    /// from. `None` when a build path doesn't have an installation
    /// (nothing in-tree does today, but Factory implementations should
    /// tolerate it rather than assume it is present). WeCom uses it to
    /// key its per-connection wsSender into a shared registry the
    /// OutboundReplier looks up by; Feishu and Slack don't currently read
    /// it.
    ///
    /// Port note: Go holds `pgtype.UUID` (nullable); Rust uses `Option`.
    pub id: Option<Uuid>,

    /// The shared inbound entry point the engine injects so the built
    /// Channel can deliver normalized inbound messages into the core (see
    /// [`crate::handler::InboundHandler`]). A Factory captures it and
    /// invokes it from the Channel's receive loop. It may be `None` when
    /// a Channel is built purely for its outbound send path (no inbound
    /// delivery needed).
    pub handler: Option<crate::handler::InboundHandler>,

    /// Lease-owner generation fencing this build and every derived handle.
    pub generation: Option<std::sync::Arc<LeaseGeneration>>,

    /// Token-fenced durable runtime observation sink. Long-lived adapters
    /// report healthy only after their provider handshake or first successful
    /// poll, never merely because the Supervisor owns a lease.
    pub runtime_health: Option<crate::RuntimeHealthReporter>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the credential blob.
        f.debug_struct("Config")
            .field("type", &self.r#type)
            .field("id", &self.id)
            .field("handler_set", &self.handler.is_some())
            .field(
                "generation",
                &self
                    .generation
                    .as_ref()
                    .map(|generation| generation.epoch()),
            )
            .field("runtime_health_set", &self.runtime_health.is_some())
            .finish_non_exhaustive()
    }
}

/// Builds a Channel from its per-installation [`Config`]. Each adapter
/// registers exactly one Factory under its Type; the
/// [`Registry`](crate::registry::Registry) calls it to instantiate a
/// per-installation Channel. A Factory should validate `raw` and return
/// an error rather than a half-built Channel.
///
/// Port note: Go's `func(cfg Config) (Channel, error)` becomes an async
/// closure erased into a boxed future; adapters write
/// `Arc::new(move |cfg| Box::pin(async move { … }))`.
pub type BuiltChannel = std::sync::Arc<dyn Channel>;
pub type FactoryFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<BuiltChannel>> + Send>>;
pub type Factory = std::sync::Arc<dyn Fn(Config) -> FactoryFuture + Send + Sync>;
