//! Connector seams — port of `server/internal/integrations/lark/connector.go`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::feishu_types::{DispatchResult, InboundMessage};
use crate::store::Installation;

/// The per-message callback an EventConnector calls for each decoded inbound
/// message. It dispatches the normalized message and returns the typed
/// outcome plus any infrastructure error.
///
/// The connector reacts only to the error: a non-nil error is a real infra
/// failure (DB down, dispatcher misconfigured) and the connector should
/// surface it and let the engine reconnect under backoff; an Ok means the
/// message was accepted and classified (it may still have been dropped for a
/// product reason — that is not an error, and any outbound reply the verdict
/// implies is handled off the ACK path by the runtime). The connector MUST
/// NOT bypass emit by writing to the DB directly; emit is the only ingress
/// path.
pub type EmitFuture =
    Pin<Box<dyn Future<Output = anyhow::Result<DispatchResult>> + Send>>;

pub type EventEmitter =
    Arc<dyn Fn(CancellationToken, InboundMessage) -> EmitFuture + Send + Sync>;

/// The per-installation Feishu transport: it opens the Lark long connection,
/// decodes events, normalizes them into [`InboundMessage`], and calls emit
/// for each. [`run`](EventConnector::run) MUST block until either ctx is
/// cancelled (returns Ok) or the connection ends and cannot be recovered
/// locally (returns Err). Implementations MUST tolerate repeated run calls on
/// different contexts — the engine may run, return, and run again after
/// backoff.
#[async_trait]
pub trait EventConnector: Send + Sync {
    async fn run(
        &self,
        ctx: CancellationToken,
        inst: Installation,
        emit: EventEmitter,
    ) -> anyhow::Result<()>;
}

/// Builds an EventConnector. Kept for the bootstrap / fallback path (the
/// noop factory); the real WS connector is a single shared instance whose run
/// is parameterized by the installation.
#[allow(clippy::type_complexity)]
pub struct ConnectorFactory(
    pub Arc<dyn Fn(Installation) -> anyhow::Result<Arc<dyn EventConnector>> + Send + Sync>,
);
