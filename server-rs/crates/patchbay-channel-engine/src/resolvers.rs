//! The pluggable seams the Router runs the inbound pipeline through.
//!
//! Everything platform-specific lives behind these traits; a platform
//! registers a [`ResolverSet`] and the channel-agnostic Router stays
//! unchanged. The Feishu implementation is the first ResolverSet.

use async_trait::async_trait;
use uuid::Uuid;

use crate::issue_command::IssueCommand;
use patchbay_channel::{InboundMessage, MediaRef, MsgType};

/// Categorizes what the Router decided to do with an inbound message.
/// Values match the legacy lark outcomes 1:1 so behavior and dashboards
/// carry over unchanged. Open string newtype; constructors mirror the Go
/// constants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome(pub String);

impl Outcome {
    pub fn dropped() -> Self {
        Self("dropped".to_string())
    }
    pub fn needs_binding() -> Self {
        Self("needs_binding".to_string())
    }
    pub fn ingested() -> Self {
        Self("ingested".to_string())
    }
    pub fn fresh_pending() -> Self {
        Self("fresh_pending".to_string())
    }
    pub fn issue_usage() -> Self {
        Self("issue_usage".to_string())
    }
    pub fn agent_offline() -> Self {
        Self("agent_offline".to_string())
    }
    pub fn agent_archived() -> Self {
        Self("agent_archived".to_string())
    }
    pub fn hub_command() -> Self {
        Self("hub_command".to_string())
    }
}

/// Enumerates the drop-audit categories. Values match the legacy lark
/// drop reasons 1:1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropReason(pub String);

impl DropReason {
    pub fn unbound_user() -> Self {
        Self("unbound_user".to_string())
    }
    pub fn non_workspace_member() -> Self {
        Self("non_workspace_member".to_string())
    }
    pub fn not_addressed_in_group() -> Self {
        Self("not_addressed_in_group".to_string())
    }
    pub fn duplicate() -> Self {
        Self("duplicate".to_string())
    }
    pub fn revoked_installation() -> Self {
        Self("revoked_installation".to_string())
    }
    pub fn invalid_event() -> Self {
        Self("invalid_event".to_string())
    }
}

/// The typed verdict the Router produces for one inbound message,
/// consumed by the outbound side (OutboundReplier / typing). Mirrors the
/// legacy lark DispatchResult.
#[derive(Debug, Clone, Default)]
pub struct Result {
    pub outcome: Option<Outcome>,
    pub drop_reason: Option<DropReason>,
    pub installation_id: Option<Uuid>,
    pub chat_session_id: Option<Uuid>,
    /// The platform-native sender id (e.g. Lark open_id), so the replier
    /// can target a binding prompt back to the sender.
    pub sender: String,
    /// A synchronous control-plane response, such as the Agent list or a
    /// confirmation after `/agents 2`. Adapters deliver this before matching
    /// the normal outcome-specific notices.
    pub reply_text: Option<String>,
    pub issue_id: Option<Uuid>,
    pub issue_number: i32,
    pub issue_identifier: String,
    pub issue_title: String,
    /// Marks an /issue command that did not create a new issue because
    /// the shared duplicate guard found the active issue above. Repliers
    /// render this as a business conflict, never as an internal error.
    pub issue_duplicate: bool,
    /// Marks a title-less /issue whose current inbound message also
    /// carried downloadable media. Repliers use it to tell the sender to
    /// include that media again with the corrected command.
    pub issue_usage_had_media: bool,
    /// Reports whether this ingest scheduled a normal chat run. It is
    /// Router-internal state: repliers must continue to use outcome.
    ///
    /// Port note: Go keeps this unexported on the struct; Rust exposes it
    /// `pub(crate)`-shaped via a doc hint — adapters should not branch on
    /// it.
    pub run_scheduled: bool,
}

/// The channel-agnostic installation context the Router needs after
/// routing. `platform` carries the adapter's own installation value
/// opaquely so the set's other ports (binder, replier, typing) reuse it
/// without a re-fetch; the Router never reads it.
///
/// Port note: Go's `Platform any` becomes `Arc<dyn Any + Send + Sync>`.
#[derive(Clone)]
pub struct ResolvedInstallation {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    /// Adapter-owned optimistic fence for platforms that route one
    /// installation to multiple agents. Zero means no such fence.
    pub route_revision: i64,
    pub installer_user_id: Uuid,
    pub active: bool,
    pub platform: std::sync::Arc<dyn std::any::Any + Send + Sync>,
}

impl Default for ResolvedInstallation {
    fn default() -> Self {
        Self {
            id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            agent_id: Uuid::nil(),
            route_revision: 0,
            installer_user_id: Uuid::nil(),
            active: false,
            platform: std::sync::Arc::new(()),
        }
    }
}

impl std::fmt::Debug for ResolvedInstallation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedInstallation")
            .field("id", &self.id)
            .field("workspace_id", &self.workspace_id)
            .field("agent_id", &self.agent_id)
            .field("route_revision", &self.route_revision)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

/// The sender mapped to a Patchbay user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedIdentity {
    pub user_id: Uuid,
}

/// Inputs for [`SessionBinder::ensure_session`]. `sender` is the resolved
/// session creator (the sole human for p2p, the installer for group chats
/// — the Router decides which and passes it here).
pub struct EnsureSessionParams {
    pub installation: ResolvedInstallation,
    pub sender: Uuid,
    pub message: InboundMessage,
}

/// Carries the inputs for [`SessionBinder::append_message`].
/// `claim_token` is the dedup owner-fence token; the binder runs the dedup
/// Mark INSIDE its chat_message+session tx so the durable write and the
/// Mark commit atomically. `media_pending_seconds` persists the
/// placeholder fallback budget; the append transaction turns it into a
/// DB-clock deadline (now() + budget) so every now()-based consumer reads
/// the same clock that wrote it.
pub struct AppendParams {
    pub session_id: Uuid,
    pub sender: Uuid,
    pub installation_id: Uuid,
    pub agent_id: Uuid,
    pub route_revision: i64,
    pub message: InboundMessage,
    pub claim_token: Option<Uuid>,
    pub media_pending_seconds: f64,
}

/// Reports what append_message decided.
#[derive(Debug, Clone, Default)]
pub struct AppendResult {
    /// The durable chat_message row created by append_message. Detached
    /// media processing uses it to link attachments after the connector
    /// ACK path has completed.
    pub message_id: Option<Uuid>,
    /// Set when the message was an /issue command.
    pub issue_command: Option<IssueCommand>,
    /// True when append_message finalized the dedup claim in its own tx;
    /// the Router then skips the post-pipeline finalize.
    pub dedup_marked: bool,
}

/// Carries stored media references to the post-append attachment
/// transaction. `message_id` is the durable chat_message whose pending
/// marker the binder clears. `issue_id` selects issue ownership for an
/// /issue turn; otherwise the references bind to the message.
/// `issue_description_base` is valid only for an issue created by this
/// turn and lets the binder replace inline placeholders iff nobody edited
/// the description first. Media downloads must never run inside this
/// transaction.
pub struct BindMediaParams {
    pub message_id: Uuid,
    pub session_id: Uuid,
    pub workspace_id: Uuid,
    pub sender: Uuid,
    pub issue_id: Option<Uuid>,
    /// Go holds pgtype.Text (valid flag); None = invalid.
    pub issue_description_base: Option<String>,
    pub issue_command_text: String,
    pub body: String,
    pub media_refs: Vec<MediaRef>,
}

/// Sentinel errors the resolvers return so the Router can map them to the
/// right product outcome instead of an infrastructure failure.
///
/// Port note: Go sentinels become typed error variants; messages mirror
/// the Go strings.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolverError {
    /// No installation matches the message's routing key → invalid_event
    /// drop.
    #[error("engine: installation not found")]
    InstallationNotFound,
    /// The sender has no identity binding → needs_binding.
    #[error("engine: sender unbound")]
    SenderUnbound,
    /// The sender is bound but not a workspace member →
    /// non_workspace_member drop.
    #[error("engine: sender not a workspace member")]
    SenderNotMember,
    /// A platform-specific route resolves to an archived agent. The route
    /// remains intact so restoring the agent restores delivery; the
    /// Router returns the normal archived-agent product outcome without
    /// creating a session or enqueueing work while the target is
    /// unavailable.
    #[error("engine: routed agent is archived")]
    TargetAgentArchived,
    /// Asks the Router to resolve the platform route again and retry the
    /// same claimed message. The durable append must return this before
    /// writing when an administrator changed the route revision
    /// concurrently.
    #[error("engine: route changed")]
    RouteChanged,
    /// Claim found the message already processed / in flight → duplicate
    /// drop.
    #[error("engine: duplicate message")]
    Duplicate,
    /// A concurrent reclaim rotated the dedup token mid-flight → treated
    /// as a duplicate.
    #[error("engine: dedup claim lost")]
    ClaimLost,
}

/// Routes an inbound message to its installation. The adapter reads
/// whatever platform routing key it needs from the message (source or
/// raw). Return [`ResolverError::InstallationNotFound`] when none matches;
/// return a [`ResolvedInstallation`] with `active = false` when it exists
/// but is revoked.
#[async_trait]
pub trait InstallationResolver: Send + Sync {
    async fn resolve_installation(
        &self,
        msg: &InboundMessage,
    ) -> anyhow::Result<ResolvedInstallation>;
}

/// Maps the message sender to a Patchbay user within the installation,
/// re-checking workspace membership. Return
/// [`ResolverError::SenderUnbound`] or
/// [`ResolverError::SenderNotMember`] for the product cases.
#[async_trait]
pub trait IdentityResolver: Send + Sync {
    async fn resolve_sender(
        &self,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
    ) -> anyhow::Result<ResolvedIdentity>;
}

/// Runs only after the group-addressing and sender identity/membership
/// gates have passed. Platforms use this optional seam for durable
/// discovery that must never be triggered by rejected callbacks. It may
/// also finalize routing fields on the installation returned to the
/// remaining pipeline.
#[async_trait]
pub trait ValidatedInboundResolver: Send + Sync {
    async fn resolve_validated_inbound(
        &self,
        inst: ResolvedInstallation,
        identity: &ResolvedIdentity,
        msg: &InboundMessage,
    ) -> anyhow::Result<ResolvedInstallation>;
}

/// The two-phase idempotency seam. Claim mints an owner-fence token
/// ([`ResolverError::Duplicate`] when already processed / in flight);
/// mark/release are fenced on the token (a no-op on token mismatch is not
/// an error).
#[async_trait]
pub trait Deduper: Send + Sync {
    async fn claim(&self, installation_id: Uuid, message_id: &str) -> anyhow::Result<Uuid>;
    async fn mark(&self, installation_id: Uuid, message_id: &str, claim_token: Uuid);
    async fn release(&self, installation_id: Uuid, message_id: &str, claim_token: Uuid);
}

/// Ensures the chat_session and appends the message (with the in-tx dedup
/// Mark). append_message returns [`ResolverError::ClaimLost`] when the
/// token was rotated mid-flight.
#[async_trait]
pub trait SessionBinder: Send + Sync {
    /// Returns the stable per-platform conversation key used by the session
    /// binding table. Hub routing stores the selected Agent beside this key.
    fn binding_key(&self, msg: &InboundMessage) -> String {
        msg.source.chat_id.clone()
    }
    async fn ensure_session(&self, p: EnsureSessionParams) -> anyhow::Result<Uuid>;
    async fn mark_pending_fresh(&self, session_id: Uuid) -> anyhow::Result<()>;
    async fn append_message(&self, p: AppendParams) -> anyhow::Result<AppendResult>;
    async fn bind_media(&self, p: BindMediaParams) -> anyhow::Result<()>;
}

/// Resolves platform media after the user message and dedup mark are
/// durable. The Router runs it off the connector ACK path and binds any
/// returned MediaRefs; the independently scheduled task remains deferred
/// until binding finishes or the persisted deadline expires.
/// Implementations are best-effort: failures leave the stored placeholder
/// text intact and NEVER delete anything inline — every uploaded object
/// is covered by an intent-ledger row written before the PUT (see
/// MediaIntentLedger), and the asynchronous reconciler settles whatever
/// binding did not claim.
#[async_trait]
pub trait MediaResolver: Send + Sync {
    /// Reports whether msg references platform media that
    /// [`resolve_media`](Self::resolve_media) would fetch. The Router
    /// calls it synchronously on the connector ACK path to decide whether
    /// to persist a media deadline and queue a resolution job at all, so
    /// implementations must be pure in-memory checks (no I/O). A false
    /// result keeps the message on the plain ingest path: no marker, no
    /// deferred run, no semaphore slot.
    fn has_media(&self, msg: &InboundMessage) -> bool;

    /// Downloads the platform media and uploads it to object storage.
    /// `chat_message_id` is the durable chat_message that owns the
    /// pending intent; the Router decides whether the resulting refs
    /// belong to that message or to an issue created from the same turn.
    /// Returns the (possibly placeholder-preserving) resolved message.
    async fn resolve_media(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        sender: &ResolvedIdentity,
        session_id: Uuid,
        chat_message_id: Uuid,
        msg: InboundMessage,
    ) -> InboundMessage;
}

// CancellationToken re-export for resolver signatures.
use tokio_util::sync::CancellationToken;

/// Persists upload intent BEFORE the object is written. The row is the
/// only artifact any failure path leaves behind: upload error, resolve
/// deadline, bind failure, ambiguous commit, or a crash all simply leave
/// it for the reconciler, which settles it long after any in-flight PUT
/// or COMMIT can still land. This is what makes "did my side effect
/// happen?" a question nobody has to answer inline.
#[async_trait]
pub trait MediaIntentLedger: Send + Sync {
    /// Upserts the intent row. `false` means the key has left 'pending'
    /// (the reconciler owns it) — the caller must skip the upload entirely
    /// rather than resurrect the row.
    async fn record_pending_media_object(
        &self,
        p: RecordPendingMediaObjectParams,
    ) -> anyhow::Result<bool>;
}

/// Identifies one intended object. `storage_url` is the URL the
/// attachment row will carry (pure function of the key), so the
/// reconciler can check for a durable reference. `installation_id` is an
/// ops-diagnostic only.
#[derive(Debug, Clone)]
pub struct RecordPendingMediaObjectParams {
    pub storage_key: String,
    pub workspace_id: Uuid,
    pub chat_message_id: Uuid,
    pub storage_url: String,
    pub installation_id: Uuid,
}

/// Records a dropped inbound event (no message body — drop-audit policy).
/// `inst_id` may be the nil UUID for installation-less events.
#[async_trait]
pub trait Auditor: Send + Sync {
    async fn record_drop(&self, inst_id: Uuid, msg: &InboundMessage, reason: &DropReason);
}

/// Delivers the verdict-driven reply (binding prompt, offline / archived
/// notice, /issue confirmation). Optional; absent disables outbound
/// replies. Driven off the ACK critical path by the Router.
///
/// Port note: Go's Reply returns nothing (fire-and-forget); the async fn
/// mirrors that contract.
#[async_trait]
pub trait OutboundReplier: Send + Sync {
    async fn reply(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        res: &Result,
    );
}

/// Shows a "processing" indicator when a message is ingested and clears
/// it once the message reaches a terminal outcome. Optional; absent
/// disables it.
#[async_trait]
pub trait TypingNotifier: Send + Sync {
    /// Shows the indicator for a successfully ingested message.
    async fn on_ingested(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        session_id: Uuid,
    );
    /// Clears the indicator for a session whose run trigger produced no
    /// task (agent offline / archived, or an enqueue failure). In that
    /// case no task lifecycle event is ever published, so the platform's
    /// own bus-driven clear (on chat-done / task-failed) would never fire
    /// and the indicator would stick. The Router calls this from the
    /// debounced flush. Idempotent: a session with no indicator is a
    /// no-op.
    async fn on_settled(&self, ctx: CancellationToken, session_id: Uuid);
}

/// The per-platform bundle the Router runs the pipeline through.
/// installation/identity/dedup/session/audit are required;
/// replier/typing/media/validated are optional. `origin_type` is the
/// issue.origin_type label written for /issue commands from this channel
/// (Feishu: "lark_chat").
///
/// Port note: Go checks nil interface fields at Register time; Rust uses
/// Options with the same validation rules.
#[derive(Default)]
pub struct ResolverSet {
    pub installation: Option<std::sync::Arc<dyn InstallationResolver>>,
    pub identity: Option<std::sync::Arc<dyn IdentityResolver>>,
    pub validated: Option<std::sync::Arc<dyn ValidatedInboundResolver>>,
    pub dedup: Option<std::sync::Arc<dyn Deduper>>,
    pub session: Option<std::sync::Arc<dyn SessionBinder>>,
    pub media: Option<std::sync::Arc<dyn MediaResolver>>,
    pub audit: Option<std::sync::Arc<dyn Auditor>>,
    pub replier: Option<std::sync::Arc<dyn OutboundReplier>>,
    pub typing: Option<std::sync::Arc<dyn TypingNotifier>>,
    pub hub: Option<std::sync::Arc<dyn crate::hub::HubRouter>>,
    pub origin_type: String,
}

/// Mirrors Go `service.IssueCreateParams` for the /issue path: the fields
/// the Router actually sets (status "todo", priority "none", assignee
/// type "agent" are fixed by create_issue_for_router).
#[derive(Debug, Clone)]
pub struct RouterIssueCreateParams {
    pub workspace_id: Uuid,
    pub title: String,
    pub description: String,
    pub assignee_agent_id: Uuid,
    pub creator_user_id: Uuid,
    /// Empty string = Go's invalid pgtype.Text (omitted).
    pub origin_type: String,
    pub origin_session_id: Uuid,
    /// Media-carrying commands defer the assigned agent's run to this
    /// deadline (crash fallback for detached media binding).
    pub assigned_run_fire_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The narrow subset of the IssueService the Router needs for the /issue
/// command. Shared across platforms.
#[async_trait]
pub trait IssueCreator: Send + Sync {
    /// Mirrors Go IssueCreator.Create: the Router passes the /issue
    /// command's create params and receives the created (or duplicate)
    /// issue plus the deferred assigned-task id.
    async fn create_issue_for_router(
        &self,
        p: RouterIssueCreateParams,
    ) -> anyhow::Result<RouterIssueOutcome>;
    async fn publish_attachments_changed(&self, issue_id: Uuid, actor_id: Uuid);
}

/// The slice of Go `service.IssueCreateResult` the Router consumes.
#[derive(Debug, Clone, Default)]
pub struct RouterIssueOutcome {
    pub issue_id: Option<Uuid>,
    pub issue_number: i32,
    pub issue_title: String,
    /// Set when an active duplicate blocked creation (Go DuplicateIssue).
    pub duplicate_issue_id: Option<Uuid>,
    pub assigned_task_id: Option<Uuid>,
}

/// The narrow subset of TaskService the Router needs to trigger a chat
/// run. Shared across platforms.
#[async_trait]
pub trait TaskEnqueuer: Send + Sync {
    async fn enqueue_chat_task(
        &self,
        session_id: Uuid,
        initiator_user_id: Uuid,
        force_fresh_session: bool,
    ) -> anyhow::Result<Uuid>;
    async fn promote_channel_chat_tasks_if_media_ready(
        &self,
        session_id: Uuid,
    ) -> anyhow::Result<()>;
    async fn promote_deferred_channel_issue_task(&self, task_id: Uuid) -> anyhow::Result<()>;
}

/// Reads the rows the debounced flush + /issue identifier need. Shared
/// across platforms; backed by the channel-backed store.
#[async_trait]
pub trait SessionReader: Send + Sync {
    async fn get_chat_session_title(&self, id: Uuid) -> anyhow::Result<String>;
    async fn get_workspace_issue_prefix(&self, id: Uuid) -> anyhow::Result<String>;
}

/// Re-exported so adapter crates can build sets without naming tokio-util.
pub use tokio_util::sync::CancellationToken as ResolverCancellationToken;

/// Adapts the shared sqlx pool to [`MediaIntentLedger`] (Go
/// NewDBMediaIntentLedger). The state-guarded upsert returns no row once
/// the reconciler moved the key to 'deleting' — that is reported as
/// `Ok(false)` so callers skip the upload instead of resurrecting the row.
pub struct DbMediaIntentLedger {
    pool: sqlx::PgPool,
}

impl DbMediaIntentLedger {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MediaIntentLedger for DbMediaIntentLedger {
    async fn record_pending_media_object(
        &self,
        p: RecordPendingMediaObjectParams,
    ) -> anyhow::Result<bool> {
        patchbay_db::queries::channel::record_channel_media_pending_object(
            &self.pool,
            &p.storage_key,
            p.workspace_id,
            p.chat_message_id,
            &p.storage_url,
            p.installation_id,
        )
        .await
        .map(|row| row.is_some())
    }
}

/// Convenience constructor matching Go's MsgTypeImage comparisons in
/// binders.
pub fn is_image(t: &MsgType) -> bool {
    t.0 == "image"
}
