//! The channel-agnostic inbound pipeline.
//!
//! (928 lines) — the generalization of the Feishu-only lark.Dispatcher.
//! It is the single shared `InboundHandler` the Supervisor injects into
//! every Channel: a Channel translates its platform payload into an
//! InboundMessage and calls [`Router::handle`], which routes by
//! ChannelType to that platform's registered resolver set and runs the
//! same ordered pipeline for every platform — installation route →
//! two-phase dedup → group @bot filter → identity + membership → ensure
//! session → append+mark → /issue → durable debounced run trigger +
//! detached media binding — then drives the detached outbound replier +
//! typing indicator.
//!
//! The core contains no platform specifics: everything platform-shaped
//! lives behind the resolver traits. Adding a platform is "register a
//! ResolverSet", not "edit the Router".

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use patchbay_channel::InboundMessage;

use crate::batcher::PendingBatcher;
use crate::issue_command::{issue_description_from_command_body, parse_issue_command};
use crate::resolvers::*;

/// Caps a single detached replier/typing call. It runs off the connector
/// ACK path, so it must stay strictly under the platform ACK deadline
/// (Lark: 3s). Defaults to 2.5s.
pub const DEFAULT_REPLY_TIMEOUT: Duration = Duration::from_millis(2500);

/// Default [`RouterConfig::media_timeout`]. Exported so the channel-media
/// settle invariant test can assert the reconciler's settle delay dwarfs
/// every pipeline budget.
pub const DEFAULT_MEDIA_TIMEOUT: Duration = Duration::from_secs(45);

/// Dedup finalization is a short durability step after the claimed
/// pipeline has selected a terminal outcome. The request context may
/// already be cancelled (notably after repeated route conflicts), so it
/// runs uncancelled with this bounded budget instead of stranding a claim
/// until stale reclaim.
const DEDUP_FINALIZE_TIMEOUT: Duration = Duration::from_secs(1);

/// Bounds the detached flush (session reload + enqueue + notice), which
/// runs on its own fresh context.
const CHAT_RUN_FLUSH_TIMEOUT: Duration = Duration::from_secs(10);

/// Bounds one media attachment-binding finalize transaction.
pub(crate) const MEDIA_FINALIZE_TIMEOUT: Duration = Duration::from_secs(5);

/// Tunes the Router. Zero values default.
#[derive(Clone, Default)]
pub struct RouterConfig {
    /// See [`DEFAULT_REPLY_TIMEOUT`].
    pub reply_timeout: Duration,
    /// Caps detached best-effort media download, upload, and attachment
    /// binding for one message. The budget starts at append time (it must
    /// match the persisted fire_at fallback), so it also spans any wait
    /// behind earlier media in the same session and for a global
    /// concurrency slot. See [`DEFAULT_MEDIA_TIMEOUT`].
    pub media_timeout: Duration,
    /// Caps concurrent media resolutions across all sessions, bounding
    /// burst memory (unknown-length uploads buffer up to the 100 MiB
    /// resource cap each) and platform download pressure. Per-session
    /// ordering is unaffected. Defaults to 8.
    pub media_concurrency: usize,
}

/// Tells dispatch how to land the claim row after process_claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DedupFinalize {
    None,
    Mark,
    Release,
}

struct MediaQueueEntry {
    tail: Arc<tokio::sync::Notify>,
}

#[derive(Default)]
struct TrackedJobs {
    closed: AtomicBool,
    handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

struct AbortJobsOnDrop {
    handles: Vec<tokio::task::AbortHandle>,
    armed: bool,
}

struct AbortTrackedJobsOnDrop {
    jobs: Arc<TrackedJobs>,
    armed: bool,
}

impl Drop for AbortJobsOnDrop {
    fn drop(&mut self) {
        if self.armed {
            for handle in &self.handles {
                handle.abort();
            }
        }
    }
}

impl Drop for AbortTrackedJobsOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.jobs.abort_now();
        }
    }
}

impl TrackedJobs {
    fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) -> bool {
        if self.closed.load(Ordering::SeqCst) {
            return false;
        }
        let mut handles = self
            .handles
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if self.closed.load(Ordering::SeqCst) {
            return false;
        }
        handles.retain(|handle| !handle.is_finished());
        handles.push(tokio::spawn(future));
        true
    }

    async fn shutdown(&self, timeout: Duration) -> bool {
        self.closed.store(true, Ordering::SeqCst);
        let mut handles = {
            let mut handles = self
                .handles
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            std::mem::take(&mut *handles)
        };
        let mut abort_on_drop = AbortJobsOnDrop {
            handles: handles.iter().map(|handle| handle.abort_handle()).collect(),
            armed: true,
        };
        let deadline = tokio::time::Instant::now() + timeout;
        while let Some(mut handle) = handles.pop() {
            if tokio::time::timeout_at(deadline, &mut handle)
                .await
                .is_err()
            {
                handle.abort();
                for handle in &abort_on_drop.handles {
                    handle.abort();
                }
                let _ = handle.await;
                for handle in handles {
                    let _ = handle.await;
                }
                abort_on_drop.armed = false;
                return false;
            }
        }
        abort_on_drop.armed = false;
        true
    }

    fn abort_now(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let handles = self
            .handles
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for handle in handles.iter() {
            handle.abort();
        }
    }
}

/// Polls resolver work inside its owning media job. Cancellation and deadline
/// drop the future in-place; no detached timer or resolver task survives the
/// tracked job.
async fn run_media_until<F, T>(
    parent: &CancellationToken,
    child: &CancellationToken,
    deadline: std::time::Instant,
    future: F,
) -> Option<T>
where
    F: Future<Output = T>,
{
    tokio::pin!(future);
    tokio::select! {
        biased;
        _ = parent.cancelled() => {
            child.cancel();
            None
        }
        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
            child.cancel();
            None
        }
        value = &mut future => Some(value),
    }
}

/// The inbound pipeline driver. Construct with [`Router::new`], register
/// per-platform [`ResolverSet`]s, then install as the shared
/// `InboundHandler`.
pub struct Router {
    sets: Mutex<HashMap<patchbay_channel::Type, Arc<ResolverSet>>>,

    issues: Arc<dyn IssueCreator>,
    tasks: Arc<dyn TaskEnqueuer>,
    reader: Arc<dyn SessionReader>,

    batcher: Mutex<Option<Arc<PendingBatcher>>>,

    reply_timeout: Duration,
    media_timeout: Duration,
    /// Cancels detached media processing on drain.
    media_ctx: CancellationToken,
    /// Global media concurrency slots.
    media_sem: Arc<tokio::sync::Semaphore>,
    media_queues: Arc<Mutex<HashMap<String, Arc<MediaQueueEntry>>>>,
    jobs: Arc<TrackedJobs>,
    stopping: Mutex<bool>,
}

impl Router {
    /// Builds a Router around the shared (platform-agnostic) services:
    /// the IssueCreator + TaskEnqueuer that /issue and chat runs go
    /// through, and a SessionReader for the debounced flush. Register a
    /// platform's ResolverSet before handling messages.
    pub fn new(
        issues: Arc<dyn IssueCreator>,
        tasks: Arc<dyn TaskEnqueuer>,
        reader: Arc<dyn SessionReader>,
        cfg: RouterConfig,
    ) -> Arc<Self> {
        let reply_timeout = if cfg.reply_timeout.is_zero() {
            DEFAULT_REPLY_TIMEOUT
        } else {
            cfg.reply_timeout
        };
        let media_timeout = if cfg.media_timeout.is_zero() {
            DEFAULT_MEDIA_TIMEOUT
        } else {
            cfg.media_timeout
        };
        let media_concurrency = if cfg.media_concurrency == 0 {
            8
        } else {
            cfg.media_concurrency
        };
        Arc::new(Self {
            sets: Mutex::new(HashMap::new()),
            issues,
            tasks,
            reader,
            batcher: Mutex::new(None),
            reply_timeout,
            media_timeout,
            media_ctx: CancellationToken::new(),
            media_sem: Arc::new(tokio::sync::Semaphore::new(media_concurrency)),
            media_queues: Arc::new(Mutex::new(HashMap::new())),
            jobs: Arc::new(TrackedJobs::default()),
            stopping: Mutex::new(false),
        })
    }

    /// Binds a platform's ResolverSet under `t`. Call at boot, before any
    /// message is handled. Registering an empty Type or a set missing a
    /// required resolver is ignored (mirrors the Go nil-field guard).
    pub fn register(&self, t: patchbay_channel::Type, set: ResolverSet) {
        if t.0.is_empty()
            || set.installation.is_none()
            || set.identity.is_none()
            || set.dedup.is_none()
            || set.session.is_none()
            || set.audit.is_none()
        {
            tracing::warn!(
                channel_type = %t,
                "channel router: ignoring incomplete resolver set"
            );
            return;
        }
        self.sets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(t, Arc::new(set));
    }

    /// Installs the debouncer in front of the per-session run trigger.
    /// Call once at boot. A zero window uses
    /// [`DEFAULT_CHAT_RUN_BATCH_WINDOW`]. Without it, runs fire inline.
    pub fn enable_run_batching(&self, window: Duration) {
        *self.batcher.lock().unwrap_or_else(|e| e.into_inner()) = Some(PendingBatcher::new(window));
    }

    /// Cancels detached media processing, flushes debounced run triggers,
    /// and joins media/reply work under one shared deadline. Returns whether
    /// everything completed. Call on shutdown AFTER the Supervisor has
    /// stopped delivering events; timed-out media retains its durable
    /// placeholder fallback and every unfinished task is aborted.
    ///
    /// Port note: Go tracks goroutines via WaitGroups; Rust awaits the
    /// batcher flush here — detached media jobs are cancellation-driven
    /// via `media_ctx`, so their DB finalize either ran or was skipped by
    /// design (placeholder fallback).
    pub async fn drain(&self, timeout: Duration) -> bool {
        *self.stopping.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.media_ctx.cancel();
        let mut abort_on_drop = AbortTrackedJobsOnDrop {
            jobs: self.jobs.clone(),
            armed: true,
        };
        let deadline = tokio::time::Instant::now() + timeout;

        let batcher = self
            .batcher
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(batcher) = batcher {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if tokio::time::timeout(remaining, batcher.flush_all())
                .await
                .is_err()
            {
                let _ = self.jobs.shutdown(Duration::ZERO).await;
                return false;
            }
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let completed = self.jobs.shutdown(remaining).await;
        abort_on_drop.armed = false;
        completed
    }

    /// The shared inbound handler. Runs the pipeline and then drives the
    /// detached outbound side; returns Err only for infrastructure
    /// failures (the adapter reconnects). Product outcomes (dropped,
    /// needs-binding, …) are not errors.
    pub async fn handle(&self, msg: InboundMessage) -> anyhow::Result<()> {
        // Preserve the user's original normalized text before any shared
        // command rewrites. Session binders pass this source to command
        // classifiers while text remains the agent-readable body.
        let mut msg = msg;
        if msg.command_text.is_empty() {
            msg.command_text = msg.text.clone();
        }

        // /new is a channel-wide product command, not an adapter
        // capability. Parse the original command source here even when an
        // adapter already set force_fresh, so bare-command classification
        // stays identical across platforms. Only rewrite text when the
        // adapter has not already stripped the directive; Feishu enriches
        // that stripped body before it reaches us.
        let mut bare_fresh = false;
        if let Some(body) = crate::fresh_command::parse_fresh_session_command(&msg.command_text) {
            let adapter_already_stripped = msg.force_fresh;
            msg.force_fresh = true;
            bare_fresh = body.is_empty();
            if !adapter_already_stripped {
                msg.text = body;
            }
        }

        let set = {
            let sets = self.sets.lock().unwrap_or_else(|e| e.into_inner());
            sets.get(&msg.source.channel_type).cloned()
        };
        let Some(set) = set else {
            tracing::error!(
                channel_type = %msg.source.channel_type,
                "channel router: no resolver set"
            );
            anyhow::bail!("channel router: no resolver set for channel type");
        };

        let (res, inst) = self.dispatch(&set, msg.clone(), bare_fresh).await?;
        let default_inst = ResolvedInstallation::default();
        tracing::debug!(
            channel_type = %msg.source.channel_type,
            event_id = %msg.event_id,
            outcome = ?res.outcome.as_ref().map(|o| &o.0),
            drop_reason = ?res.drop_reason.as_ref().map(|d| &d.0),
            "channel router: dispatch outcome"
        );

        // Typing indicator on ingest, detached so the reaction HTTP call
        // never blocks the connector ACK path.
        if res.outcome.as_ref() == Some(&Outcome::ingested()) && res.run_scheduled {
            if let Some(typing) = &set.typing {
                let typing = typing.clone();
                let inst = inst.clone().unwrap_or_default();
                let msg = msg.clone();
                let session_id = res.chat_session_id;
                let timeout = self.reply_timeout;
                self.jobs.spawn(async move {
                    let _ = tokio::time::timeout(
                        timeout,
                        typing.on_ingested(
                            CancellationToken::new(),
                            &inst,
                            &msg,
                            session_id.unwrap_or_default(),
                        ),
                    )
                    .await;
                });
            }
        }
        self.schedule_reply(&set, inst.as_ref().unwrap_or(&default_inst), &msg, &res)
            .await;
        Ok(())
    }

    /// Runs the pipeline and returns the typed result plus the resolved
    /// installation (needed by the outbound side). Mirrors
    /// lark.Dispatcher.Handle.
    #[allow(clippy::too_many_lines)]
    async fn dispatch(
        &self,
        set: &Arc<ResolverSet>,
        msg: InboundMessage,
        bare_fresh: bool,
    ) -> anyhow::Result<(Result, Option<ResolvedInstallation>)> {
        // 1. Route to installation. The adapter maps the platform routing
        //    key (carried on the message) to its installation row. These
        //    drop branches run BEFORE the dedup claim because they have no
        //    valid installation to attach a claim to.
        let inst = match set
            .installation
            .as_ref()
            .unwrap()
            .resolve_installation(&msg)
            .await
        {
            Ok(inst) => inst,
            Err(err)
                if err
                    .downcast_ref::<ResolverError>()
                    .is_some_and(|e| *e == ResolverError::InstallationNotFound) =>
            {
                if let Some(audit) = &set.audit {
                    audit
                        .record_drop(Uuid::nil(), &msg, &DropReason::invalid_event())
                        .await;
                }
                return Ok((
                    Result {
                        outcome: Some(Outcome::dropped()),
                        drop_reason: Some(DropReason::invalid_event()),
                        ..Default::default()
                    },
                    None,
                ));
            }
            Err(err) => {
                return Err(err.context("resolve installation"));
            }
        };
        if !inst.active {
            let res = self
                .drop(set, &msg, inst.id, DropReason::revoked_installation())
                .await;
            return Ok((res, Some(inst)));
        }

        // 2. Two-phase dedup claim with owner fencing — before group
        //    filter and identity so a reconnect replay cannot re-trigger a
        //    binding prompt, re-write a drop audit, or re-touch the
        //    session. Empty MessageID means there is no key to dedup by;
        //    skip the claim.
        let mut claim_token: Option<Uuid> = None;
        if !msg.message_id.is_empty() {
            match set
                .dedup
                .as_ref()
                .unwrap()
                .claim(inst.id, &msg.message_id)
                .await
            {
                Ok(token) => claim_token = Some(token),
                Err(err)
                    if err
                        .downcast_ref::<ResolverError>()
                        .is_some_and(|e| *e == ResolverError::Duplicate) =>
                {
                    let res = self.drop(set, &msg, inst.id, DropReason::duplicate()).await;
                    return Ok((res, Some(inst)));
                }
                Err(err) => {
                    return Err(err.context("dedup claim"));
                }
            }
        }

        let (mut res, finalize, mut err) = self
            .process_claimed(set, msg.clone(), &inst, claim_token, bare_fresh)
            .await;

        if claim_token.is_some() && finalize != DedupFinalize::None {
            // Bounded, uncancelled finalize (Go context.WithoutCancel).
            let _ = DEDUP_FINALIZE_TIMEOUT;
            self.apply_finalize(set, &inst, &msg.message_id, claim_token, finalize)
                .await;
        }

        // ClaimLost: another worker holds the claim → duplicate.
        if err.as_ref().is_some_and(|e| {
            e.downcast_ref::<ResolverError>()
                .is_some_and(|e| *e == ResolverError::ClaimLost)
        }) {
            err = None;
            res = self.drop(set, &msg, inst.id, DropReason::duplicate()).await;
        }
        if let Some(e) = err {
            return Err(e.context("channel router: dispatch"));
        }
        Ok((res, Some(inst)))
    }

    /// Runs the post-dedup pipeline. Mirrors lark.Dispatcher.
    /// processClaimed; see its boundary contract per step.
    #[allow(clippy::too_many_lines)]
    async fn process_claimed(
        &self,
        set: &Arc<ResolverSet>,
        msg: InboundMessage,
        inst: &ResolvedInstallation,
        claim_token: Option<Uuid>,
        bare_fresh: bool,
    ) -> (Result, DedupFinalize, Option<anyhow::Error>) {
        let none = Result::default();
        // 3. Group-mention filter (group chats only), before identity so
        //    an unbound user's idle group chatter never spams a binding
        //    card.
        if msg.source.chat_type.0 == patchbay_channel::ChatType::group().0 && !msg.addressed_to_bot
        {
            let res = self
                .drop(set, &msg, inst.id, DropReason::not_addressed_in_group())
                .await;
            return (res, DedupFinalize::Mark, None);
        }

        // 4. Identity check: map the platform sender to a Patchbay user and
        //    re-verify workspace membership (no binding->member FK;
        //    PB-3515 §4).
        let identity = match set
            .identity
            .as_ref()
            .unwrap()
            .resolve_sender(inst, &msg)
            .await
        {
            Ok(id) => id,
            Err(err)
                if err
                    .downcast_ref::<ResolverError>()
                    .is_some_and(|e| *e == ResolverError::SenderUnbound) =>
            {
                if let Some(audit) = &set.audit {
                    audit
                        .record_drop(inst.id, &msg, &DropReason::unbound_user())
                        .await;
                }
                return (
                    Result {
                        outcome: Some(Outcome::needs_binding()),
                        drop_reason: Some(DropReason::unbound_user()),
                        installation_id: Some(inst.id),
                        sender: msg.source.sender_id.clone(),
                        ..Default::default()
                    },
                    DedupFinalize::Mark,
                    None,
                );
            }
            Err(err)
                if err
                    .downcast_ref::<ResolverError>()
                    .is_some_and(|e| *e == ResolverError::SenderNotMember) =>
            {
                let res = self
                    .drop(set, &msg, inst.id, DropReason::non_workspace_member())
                    .await;
                return (res, DedupFinalize::Mark, None);
            }
            Err(err) => {
                return (
                    none,
                    DedupFinalize::Release,
                    Some(anyhow::anyhow!("resolve sender: {err:#}")),
                );
            }
        };

        // Platform discovery that exposes a conversation to workspace
        // members must run only after both the @bot gate and sender
        // membership validation.
        let mut inst = inst.clone();
        if let Some(validated) = &set.validated {
            match validated
                .resolve_validated_inbound(inst.clone(), &identity, &msg)
                .await
            {
                Ok(next) => inst = next,
                Err(err)
                    if err
                        .downcast_ref::<ResolverError>()
                        .is_some_and(|e| *e == ResolverError::TargetAgentArchived) =>
                {
                    return (
                        Result {
                            outcome: Some(Outcome::agent_archived()),
                            installation_id: Some(inst.id),
                            sender: msg.source.sender_id.clone(),
                            ..Default::default()
                        },
                        DedupFinalize::Mark,
                        None,
                    );
                }
                Err(err) => {
                    return (
                        none,
                        DedupFinalize::Release,
                        Some(anyhow::anyhow!("resolve validated inbound route: {err:#}")),
                    );
                }
            }
        }

        // Workspace-scoped installations route inside the channel itself.
        // The settings page only creates the platform connection; the first
        // ordinary message uses the current/default Agent, while `/agents`
        // lists or switches Agents without becoming an Agent turn.
        let binder = set.session.as_ref().unwrap();
        let mut hub_route = None;
        if inst.agent_id.is_nil() {
            let Some(hub) = &set.hub else {
                return (
                    none,
                    DedupFinalize::Release,
                    Some(anyhow::anyhow!(
                        "workspace installation has no channel hub router"
                    )),
                );
            };
            let binding_key = binder.binding_key(&msg);
            let route = match hub
                .resolve(&inst, &identity, &msg, &binding_key)
                .await
            {
                Ok(route) => route,
                Err(err) => {
                    return (
                        none,
                        DedupFinalize::Release,
                        Some(anyhow::anyhow!("resolve channel hub route: {err:#}")),
                    );
                }
            };
            let Some(agent_id) = route.agent_id else {
                return (
                    Result {
                        outcome: Some(Outcome::hub_command()),
                        reply_text: route.reply_text,
                        installation_id: Some(inst.id),
                        sender: msg.source.sender_id.clone(),
                        ..Default::default()
                    },
                    DedupFinalize::Mark,
                    None,
                );
            };
            if route.handled && !route.ensure_session {
                return (
                    Result {
                        outcome: Some(Outcome::hub_command()),
                        reply_text: route.reply_text,
                        installation_id: Some(inst.id),
                        sender: msg.source.sender_id.clone(),
                        ..Default::default()
                    },
                    DedupFinalize::Mark,
                    None,
                );
            }
            inst.agent_id = agent_id;
            hub_route = Some((route, binding_key));
        }

        // 5-6. Resolve the chat_session, then append the message and dedup
        //      Mark as the durable transition point. A platform route
        //      fence may reject the append when a concurrent reassignment
        //      committed first. Resolve the latest route and retry this
        //      same claimed message in-process: DingTalk has already ACKed
        //      the callback and will not redeliver it.
        let session_creator = if msg.source.chat_type.0 == patchbay_channel::ChatType::group().0 {
            inst.installer_user_id
        } else {
            identity.user_id
        };

        // The media budget is persisted only when the message actually
        // carries media: a plain text message must never wait behind the
        // media semaphore or fall back to the 45s deadline after a crash.
        let parsed_command = parse_issue_command(&msg.command_text);
        let issue_needs_usage = parsed_command.as_ref().is_some_and(|c| c.title.is_empty());
        let has_media = set.media.as_ref().is_some_and(|m| m.has_media(&msg));
        let resolve_media = !issue_needs_usage && has_media;
        // The local monotonic budget starts BEFORE the append: the DB
        // anchors its fallback at now() during the insert, so a
        // post-commit local start would end Δ(append latency) AFTER the
        // durable deadline. Starting here keeps local-gives-up ≤
        // durable-fallback-fires.
        let local_media_deadline = std::time::Instant::now() + self.media_timeout;
        let media_pending_seconds = if resolve_media {
            self.media_timeout.as_secs_f64()
        } else {
            0.0
        };

        let session_id: Uuid;
        #[allow(unused_variables)]
        let append_res: AppendResult;
        #[allow(unused_assignments)]
        loop {
            match binder
                .ensure_session(EnsureSessionParams {
                    installation: inst.clone(),
                    sender: session_creator,
                    message: msg.clone(),
                })
                .await
            {
                Ok(id) => {
                    session_id = id;
                    break;
                }
                Err(err)
                    if err
                        .downcast_ref::<ResolverError>()
                        .is_some_and(|e| *e == ResolverError::RouteChanged) =>
                {
                    match self.refresh_route(set, &identity, &msg, &mut inst).await {
                        RefreshOutcome::Refreshed => continue,
                        RefreshOutcome::Archived(result) => {
                            return (result, DedupFinalize::Mark, None)
                        }
                        RefreshOutcome::Failed(err) => {
                            return (none, DedupFinalize::Release, Some(err));
                        }
                    }
                }
                Err(err) => {
                    // Single tx; an error rolled it back, nothing landed.
                    return (
                        none,
                        DedupFinalize::Release,
                        Some(anyhow::anyhow!("ensure chat session: {err:#}")),
                    );
                }
            }
        }

        if let Some((route, binding_key)) = &hub_route {
            if let Some(agent_id) = route.agent_id {
                if let Some(hub) = &set.hub {
                    if let Err(err) = hub
                        .persist_route(
                            inst.id,
                            inst.workspace_id,
                            binding_key,
                            session_id,
                            agent_id,
                        )
                        .await
                    {
                        return (
                            none,
                            DedupFinalize::Release,
                            Some(anyhow::anyhow!("persist channel hub route: {err:#}")),
                        );
                    }
                }
            }
            if route.handled {
                return (
                    Result {
                        outcome: Some(Outcome::hub_command()),
                        reply_text: route.reply_text.clone(),
                        installation_id: Some(inst.id),
                        chat_session_id: Some(session_id),
                        sender: msg.source.sender_id.clone(),
                        ..Default::default()
                    },
                    DedupFinalize::Mark,
                    None,
                );
            }
        }

        if bare_fresh {
            // ForceFresh is a task-dispatch property. A bare command has no
            // useful task to dispatch, so remember the intent and apply it
            // to the next real message instead of writing or running an
            // empty turn.
            if let Err(err) = binder.mark_pending_fresh(session_id).await {
                return (
                    none,
                    DedupFinalize::Release,
                    Some(anyhow::anyhow!("persist fresh command: {err:#}")),
                );
            }
            return (
                Result {
                    outcome: Some(Outcome::fresh_pending()),
                    installation_id: Some(inst.id),
                    chat_session_id: Some(session_id),
                    sender: msg.source.sender_id.clone(),
                    ..Default::default()
                },
                DedupFinalize::Mark,
                None,
            );
        }

        let append_res = loop {
            match binder
                .append_message(AppendParams {
                    session_id,
                    sender: identity.user_id,
                    installation_id: inst.id,
                    agent_id: inst.agent_id,
                    route_revision: inst.route_revision,
                    message: msg.clone(),
                    claim_token,
                    media_pending_seconds,
                })
                .await
            {
                Ok(res) => break res,
                Err(err)
                    if err
                        .downcast_ref::<ResolverError>()
                        .is_some_and(|e| *e == ResolverError::RouteChanged) =>
                {
                    match self.refresh_route(set, &identity, &msg, &mut inst).await {
                        RefreshOutcome::Refreshed => continue,
                        RefreshOutcome::Archived(result) => {
                            return (result, DedupFinalize::Mark, None)
                        }
                        RefreshOutcome::Failed(err) => {
                            return (none, DedupFinalize::Release, Some(err));
                        }
                    }
                }
                Err(err)
                    if err
                        .downcast_ref::<ResolverError>()
                        .is_some_and(|e| *e == ResolverError::ClaimLost) =>
                {
                    return (none, DedupFinalize::None, Some(err));
                }
                Err(err) => {
                    return (
                        none,
                        DedupFinalize::Release,
                        Some(anyhow::anyhow!("append user message: {err:#}")),
                    );
                }
            }
        };

        // Post-append paths must NOT Release (chat_message + Mark already
        // committed). Mark-again is a no-op, so finalizeNone — unless the
        // binder did not Mark in-tx (defensive), then fall back to a
        // post-pipeline Mark.
        let post_append_finalize = if append_res.dedup_marked {
            DedupFinalize::None
        } else {
            DedupFinalize::Mark
        };

        let mut res = Result {
            outcome: Some(Outcome::ingested()),
            installation_id: Some(inst.id),
            chat_session_id: Some(session_id),
            sender: msg.source.sender_id.clone(),
            ..Default::default()
        };
        let mut media_issue: Option<Uuid> = None;

        // 7. /issue command, if present. chat_message is already durable;
        //    all error returns from here signal finalizeNone-or-defensive-Mark.
        if let Some(cmd) = &append_res.issue_command {
            if cmd.title.is_empty() {
                res.outcome = Some(Outcome::issue_usage());
                res.issue_usage_had_media = has_media;
                return (res, post_append_finalize, None);
            }
            let mut cmd = cmd.clone();
            if resolve_media {
                // CommandText intentionally omits adapter-generated media
                // placeholders so image-before-command layouts still
                // classify as /issue. Restore the description from the
                // full normalized body after classification so the created
                // issue retains the inline positions that detached media
                // binding will materialize.
                cmd.description = issue_description_from_command_body(
                    &msg.text,
                    &msg.command_text,
                    &cmd.description,
                );
            }
            // One lookup feeds both the broadcast payload's identifier and
            // the chat reply's.
            let prefix = self.issue_prefix(inst.workspace_id).await;
            let assigned_run_fire_at = resolve_media.then(|| {
                // The generic deferred-task sweeper is the crash fallback.
                // Leave room after the media deadline for the bounded
                // attachment finalizer so it cannot race an issue agent
                // reading the newly-created issue.
                chrono::Utc::now()
                    + chrono::Duration::from_std(self.media_timeout + MEDIA_FINALIZE_TIMEOUT)
                        .unwrap_or_default()
            });
            let issue_res = self
                .issues
                .create_issue_for_router(RouterIssueCreateParams {
                    workspace_id: inst.workspace_id,
                    title: cmd.title.clone(),
                    description: cmd.description.clone(),
                    assignee_agent_id: inst.agent_id,
                    creator_user_id: identity.user_id,
                    origin_type: set.origin_type.clone(),
                    origin_session_id: session_id,
                    assigned_run_fire_at,
                })
                .await;
            let issue_res = match issue_res {
                Ok(r) => r,
                Err(err) => {
                    return (
                        none,
                        post_append_finalize,
                        Some(anyhow::anyhow!("create issue from command: {err:#}")),
                    );
                }
            };
            if let Some(dup_id) = issue_res.duplicate_issue_id {
                res.issue_id = Some(dup_id);
                res.issue_number = issue_res.issue_number;
                res.issue_title = issue_res.issue_title.clone();
                res.issue_identifier = format!("{prefix}{}", issue_res.issue_number);
                res.issue_duplicate = true;
                // A duplicate is a terminal product outcome, not an
                // infrastructure failure and not a chat prompt. Finalize
                // the durable chat message's media state without resolving
                // it — no new issue exists to consume the media.
                if resolve_media {
                    self.enqueue_media_job(
                        set,
                        &inst,
                        &identity,
                        append_res.message_id,
                        &msg,
                        session_id,
                        None,
                        String::new(),
                        String::new(),
                        None,
                        false,
                        local_media_deadline,
                    );
                }
                return (res, post_append_finalize, None);
            }
            res.issue_id = issue_res.issue_id;
            media_issue = issue_res.issue_id;
            res.issue_number = issue_res.issue_number;
            res.issue_title = issue_res.issue_title.clone();
            // Same renderer the broadcast payload uses, so a degraded
            // prefix can't show the chat "#42" while the realtime list
            // shows "-42".
            res.issue_identifier = format!("{prefix}{}", issue_res.issue_number);
            // IssueService.Create already enqueues the assigned agent's
            // issue task; scheduling the command as a chat run too would
            // execute the same /issue input again. Synchronous issue
            // commands are terminal.
            if resolve_media {
                self.enqueue_media_job(
                    set,
                    &inst,
                    &identity,
                    append_res.message_id,
                    &msg,
                    session_id,
                    media_issue,
                    cmd.description.clone(),
                    msg.command_text.clone(),
                    issue_res.assigned_task_id,
                    true,
                    local_media_deadline,
                );
            }
            return (res, post_append_finalize, None);
        }

        // 8. Debounce the run trigger. The synchronous outcome is ingested
        //    with no task id — the task row is created at flush.
        //    identity.user_id is THIS message's sender (the task
        //    initiator), deliberately not the session creator (group
        //    sessions are creator=installer). Latest sender in a window
        //    wins (PB-2645).
        //
        //    SkipAgentRun lets an adapter opt this message out of the
        //    agent turn — used by wecom for standalone /issue commands
        //    where the engine has already done the meaningful work and an
        //    agent reply would just quote the slash command back.
        if !msg.skip_agent_run {
            self.schedule_run(set, &inst, &msg, session_id, identity.user_id);
            res.run_scheduled = true;
        }
        if resolve_media {
            self.enqueue_media_job(
                set,
                &inst,
                &identity,
                append_res.message_id,
                &msg,
                session_id,
                media_issue,
                String::new(),
                String::new(),
                None,
                true,
                local_media_deadline,
            );
        }
        (res, post_append_finalize, None)
    }

    /// Route-refresh helper for the ErrRouteChanged retry loops.
    async fn refresh_route(
        &self,
        set: &Arc<ResolverSet>,
        identity: &ResolvedIdentity,
        msg: &InboundMessage,
        inst: &mut ResolvedInstallation,
    ) -> RefreshOutcome {
        let Some(validated) = &set.validated else {
            // Go returns ErrRouteChanged when Validated is nil, which the
            // caller surfaces as a release-and-fail.
            return RefreshOutcome::Failed(anyhow::anyhow!("{}", ResolverError::RouteChanged));
        };
        match validated
            .resolve_validated_inbound(std::mem::take(inst), identity, msg)
            .await
        {
            Ok(next) => {
                *inst = next;
                RefreshOutcome::Refreshed
            }
            Err(err)
                if err
                    .downcast_ref::<ResolverError>()
                    .is_some_and(|e| *e == ResolverError::TargetAgentArchived) =>
            {
                RefreshOutcome::Archived(Result {
                    outcome: Some(Outcome::agent_archived()),
                    installation_id: Some(inst.id),
                    sender: msg.source.sender_id.clone(),
                    ..Default::default()
                })
            }
            Err(err) => {
                RefreshOutcome::Failed(anyhow::anyhow!("refresh validated inbound route: {err:#}"))
            }
        }
    }

    /// Detaches remote media I/O from handle while preserving message
    /// order within a chat session. Run scheduling is independent and
    /// durable: the task service defers a task to the persisted media
    /// deadline, then media completion promotes it early.
    #[allow(clippy::too_many_arguments)]
    fn enqueue_media_job(
        &self,
        set: &Arc<ResolverSet>,
        inst: &ResolvedInstallation,
        identity: &ResolvedIdentity,
        chat_message_id: Option<Uuid>,
        msg: &InboundMessage,
        session_id: Uuid,
        issue_id: Option<Uuid>,
        issue_description_base: String,
        issue_command_text: String,
        issue_task_id: Option<Uuid>,
        resolve_remote: bool,
        deadline: std::time::Instant,
    ) {
        let key = session_id.to_string();
        let (predecessor, completion) = {
            let stopping = *self.stopping.lock().unwrap_or_else(|e| e.into_inner());
            if stopping {
                return;
            }
            let completion = Arc::new(tokio::sync::Notify::new());
            let predecessor = self
                .media_queues
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    key.clone(),
                    Arc::new(MediaQueueEntry {
                        tail: completion.clone(),
                    }),
                )
                .map(|entry| entry.tail.clone());
            (predecessor, completion)
        };

        let router_media_ctx = self.media_ctx.clone();
        let sem = self.media_sem.clone();
        let set = Arc::clone(set);
        let inst = inst.clone();
        let identity = *identity;
        let msg = msg.clone();
        let media_timeout = self.media_timeout;
        let _ = media_timeout;
        let issues = self.issues.clone();
        let tasks = self.tasks.clone();
        let media_queues = self.media_queues.clone();

        self.jobs.spawn(async move {
            // Both queue waits are bounded by the message's own deadline:
            // in a media burst an already-expired job must skip straight
            // to the empty finalize (marker clear + promotion), which also
            // unblocks the session's later messages.
            let expiry = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
            tokio::pin!(expiry);
            let mut expired = false;
            // Wait behind the previous job for this session.
            if let Some(predecessor) = predecessor {
                tokio::select! {
                    _ = predecessor.notified() => {}
                    _ = router_media_ctx.cancelled() => {}
                    _ = &mut expiry => expired = true,
                }
            }
            let mut resolved = msg.clone();
            if !expired && resolve_remote {
                // Global concurrency slot; cancelled while queued means we
                // proceed without one and only run the bounded DB finalize,
                // preserving prompt marker clearing on shutdown.
                let slot = tokio::select! {
                    s = sem.acquire() => s.ok(),
                    _ = router_media_ctx.cancelled() => None,
                    _ = &mut expiry => { expired = true; None },
                };
                if let Some(_slot) = slot {
                    if !router_media_ctx.is_cancelled() {
                        if let (Some(media), Some(chat_message_id)) = (&set.media, chat_message_id)
                        {
                            let media_ctx = router_media_ctx.child_token();
                            let resolve = media.resolve_media(
                                media_ctx.clone(),
                                &inst,
                                &identity,
                                session_id,
                                chat_message_id,
                                resolved.clone(),
                            );
                            match run_media_until(&router_media_ctx, &media_ctx, deadline, resolve)
                                .await
                            {
                                Some(value) => resolved = value,
                                None => expired = true,
                            }
                        }
                    }
                }
            }
            if resolve_remote && (expired || router_media_ctx.is_cancelled()) {
                // Refs resolved before the deadline already sit in object
                // storage but will not gain an attachment row. Nothing is
                // deleted here — their intent-ledger rows were written
                // before the uploads, and the reconciler reclaims
                // unreferenced objects after the settle delay.
                tracing::warn!(
                    channel_type = %msg.source.channel_type,
                    event_id = %msg.event_id,
                    message_id = %msg.message_id,
                    "channel router: media resolution incomplete; using placeholder"
                );
                resolved.media_refs = Vec::new();
            } else if !resolve_remote {
                resolved.media_refs = Vec::new();
            }

            // Bounded finalize transaction (Go: fresh context + 5s).
            let finalize_budget = MEDIA_FINALIZE_TIMEOUT;
            let bind_ok = match chat_message_id {
                Some(message_id) => {
                    let bind = set.session.as_ref().map(|s| {
                        s.bind_media(BindMediaParams {
                            message_id,
                            session_id,
                            workspace_id: inst.workspace_id,
                            sender: identity.user_id,
                            issue_id,
                            issue_description_base: (!issue_description_base.is_empty())
                                .then_some(issue_description_base.clone()),
                            issue_command_text: issue_command_text.clone(),
                            body: resolved.text.clone(),
                            media_refs: resolved.media_refs.clone(),
                        })
                    });
                    match bind {
                        Some(b) => tokio::time::timeout(finalize_budget, b)
                            .await
                            .unwrap_or(Err(anyhow::anyhow!("media finalize timed out"))),
                        None => Ok(()),
                    }
                }
                None => Ok(()),
            };
            if let Err(err) = &bind_ok {
                // Never delete inline: the attachments may or may not have
                // landed (an ambiguous commit), but the intent rows are
                // deleted in the SAME transaction, so the ledger already
                // reflects whichever outcome is durable and the reconciler
                // settles the objects.
                tracing::warn!(
                    channel_type = %msg.source.channel_type,
                    event_id = %msg.event_id,
                    message_id = %msg.message_id,
                    error = %err,
                    "channel router: media attachment binding failed"
                );
            }
            if bind_ok.is_ok() && issue_id.is_some() && !resolved.media_refs.is_empty() {
                if let Some(issue) = issue_id {
                    issues
                        .publish_attachments_changed(issue, identity.user_id)
                        .await;
                }
            }
            if let Some(task_id) = issue_task_id {
                if let Err(err) = tasks.promote_deferred_channel_issue_task(task_id).await {
                    tracing::warn!(
                        event_id = %msg.event_id,
                        task_id = %task_id,
                        error = %err,
                        "channel router: media-ready issue task promotion failed"
                    );
                }
            }
            if let Err(err) = tasks
                .promote_channel_chat_tasks_if_media_ready(session_id)
                .await
            {
                tracing::warn!(
                    event_id = %msg.event_id,
                    error = %err,
                    "channel router: media-ready task promotion failed"
                );
            }
            // Wake the next queued job for this session and drop the queue
            // entry when this job is its tail (finishMediaQueue). Arc
            // pointer-equality identifies the tail.
            completion.notify_waiters();
            let mut map = media_queues.lock().unwrap_or_else(|e| e.into_inner());
            let is_tail = map
                .get(&session_id.to_string())
                .is_some_and(|e| Arc::ptr_eq(&e.tail, &completion));
            if is_tail {
                map.remove(&session_id.to_string());
            }
        });
    }

    /// Hands the per-session run trigger to the debouncer (or fires it
    /// inline when batching is disabled).
    fn schedule_run(
        &self,
        set: &Arc<ResolverSet>,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        session_id: Uuid,
        initiator_user_id: Uuid,
    ) {
        let fresh = msg.force_fresh;
        let batcher = self
            .batcher
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(batcher) = batcher else {
            self.jobs.spawn(flush_chat_run(FlushJob {
                reader: self.reader.clone(),
                tasks: self.tasks.clone(),
                set: Arc::clone(set),
                inst: inst.clone(),
                msg: msg.clone(),
                session_id,
                initiator_user_id,
                force_fresh: fresh,
            }));
            return;
        };
        let set2 = Arc::clone(set);
        let inst2 = inst.clone();
        let msg2 = msg.clone();
        let reader = self.reader.clone();
        let tasks = self.tasks.clone();
        let jobs = self.jobs.clone();
        batcher.schedule(&session_id.to_string(), move || {
            // A later message may replace this closure inside the debounce
            // window. AppendMessage persists ForceFresh on the channel
            // binding, and EnqueueChatTask consumes it transactionally, so
            // correctness does not depend on which closure wins.
            let set = set2.clone();
            let inst = inst2.clone();
            let msg = msg2.clone();
            let reader = reader.clone();
            let tasks = tasks.clone();
            jobs.spawn(async move {
                flush_chat_run(FlushJob {
                    reader,
                    tasks,
                    set,
                    inst,
                    msg,
                    session_id,
                    initiator_user_id,
                    force_fresh: fresh,
                })
                .await;
            });
        });
    }

    /// Detaches the OutboundReplier from the ACK critical path. The reply
    /// runs with a fresh timeout so it is independent of the inbound emit
    /// context (which the adapter cancels when its receive loop exits). A
    /// missing replier short-circuits — no task.
    async fn schedule_reply(
        &self,
        set: &Arc<ResolverSet>,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        res: &Result,
    ) {
        let Some(replier) = &set.replier else {
            return;
        };
        let replier = replier.clone();
        let inst = inst.clone();
        let msg = msg.clone();
        let res = res.clone();
        let timeout = self.reply_timeout;
        self.jobs.spawn(async move {
            let _ = tokio::time::timeout(
                timeout,
                replier.reply(CancellationToken::new(), &inst, &msg, &res),
            )
            .await;
        });
    }

    /// Reads the workspace's issue key (the "PB" in PB-42). A read
    /// failure is not worth failing issue creation over, so it degrades
    /// to empty and only the rendered identifier suffers.
    async fn issue_prefix(&self, workspace_id: Uuid) -> String {
        match self.reader.get_workspace_issue_prefix(workspace_id).await {
            Ok(prefix) => prefix,
            Err(err) => {
                tracing::warn!(
                    workspace_id = %workspace_id,
                    error = %err,
                    "channel engine: workspace lookup for issue prefix failed"
                );
                String::new()
            }
        }
    }

    /// Flips the in-flight claim row to its terminal state, token-fenced.
    /// Best-effort: a transport failure cannot abort the outcome.
    async fn apply_finalize(
        &self,
        set: &Arc<ResolverSet>,
        inst: &ResolvedInstallation,
        message_id: &str,
        claim_token: Option<Uuid>,
        action: DedupFinalize,
    ) {
        let Some(dedup) = &set.dedup else { return };
        let Some(token) = claim_token else { return };
        let fut = match action {
            DedupFinalize::Mark => dedup.mark(inst.id, message_id, token),
            DedupFinalize::Release => dedup.release(inst.id, message_id, token),
            DedupFinalize::None => return,
        };
        let _ = tokio::time::timeout(DEDUP_FINALIZE_TIMEOUT, fut).await;
    }

    /// Records a drop audit and builds the dropped Result. Best-effort.
    async fn drop(
        &self,
        set: &Arc<ResolverSet>,
        msg: &InboundMessage,
        inst_id: Uuid,
        reason: DropReason,
    ) -> Result {
        if let Some(audit) = &set.audit {
            audit.record_drop(inst_id, msg, &reason).await;
        }
        Result {
            outcome: Some(Outcome::dropped()),
            drop_reason: Some(reason),
            installation_id: Some(inst_id),
            ..Default::default()
        }
    }
}

/// RAII marker standing in for Go's mediaWg.Add/Done bookkeeping.
///
/// Port note: detached media jobs are observed through the bounded drain
/// select on `media_ctx` + batcher flush, not an explicit WaitGroup; this
/// struct is retained for the wiring slice's shutdown assertions.
#[allow(dead_code)]
struct JobGuard(Arc<std::sync::atomic::AtomicBool>);
#[allow(dead_code)]
impl Drop for JobGuard {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

enum RefreshOutcome {
    Refreshed,
    Archived(Result),
    Failed(anyhow::Error),
}

/// Sentinel outcomes of the debounced flush's EnqueueChatTask failure,
/// mapped to their product notices.
///
/// Port note: Go matches service.ErrChatTaskAgentNoRuntime /
/// ErrChatTaskAgentArchived sentinels; until patchbay-service exports them
/// the flush seam reports them through this enum via downcast. The
/// thiserror impl satisfies anyhow's Downcast requirements (Display +
/// Error).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FlushError {
    #[error("channel router: agent has no runtime")]
    AgentNoRuntime,
    #[error("channel router: agent is archived")]
    AgentArchived,
}

/// The debounced run-trigger body: reload session, enqueue exactly one
/// chat task for the window, and emit the offline/archived notice. Errors
/// are logged, not returned. The bundle struct keeps the arity at clippy's
/// threshold while mirroring the Go receiver+args shape.
struct FlushJob {
    reader: Arc<dyn SessionReader>,
    tasks: Arc<dyn TaskEnqueuer>,
    set: Arc<ResolverSet>,
    inst: ResolvedInstallation,
    msg: InboundMessage,
    session_id: Uuid,
    initiator_user_id: Uuid,
    force_fresh: bool,
}

async fn flush_chat_run(job: FlushJob) {
    let FlushJob {
        reader,
        tasks,
        set,
        inst,
        msg,
        session_id,
        initiator_user_id,
        force_fresh,
    } = job;
    let _ = tokio::time::timeout(CHAT_RUN_FLUSH_TIMEOUT, async {
        if reader.get_chat_session_title(session_id).await.is_err() {
            tracing::error!(
                chat_session_id = %session_id,
                "channel router: flush reload chat session failed"
            );
            clear_typing(&set, session_id).await;
            return;
        }
        if let Err(err) = tasks
            .enqueue_chat_task(session_id, initiator_user_id, force_fresh)
            .await
        {
            // No task was enqueued, so no task lifecycle event will ever
            // publish and the platform's bus-driven typing clear can never
            // fire. Clear the indicator here (before any notice) so the
            // "processing" reaction does not stick on the user's message.
            clear_typing(&set, session_id).await;
            match err.downcast_ref::<FlushError>() {
                Some(FlushError::AgentNoRuntime) => {
                    emit_flush_reply(&set, &inst, &msg, session_id, Outcome::agent_offline()).await;
                }
                Some(FlushError::AgentArchived) => {
                    emit_flush_reply(&set, &inst, &msg, session_id, Outcome::agent_archived())
                        .await;
                }
                None => {
                    tracing::error!(
                        chat_session_id = %session_id,
                        error = %err,
                        "channel router: flush enqueue chat task failed"
                    );
                }
            }
        }
    })
    .await
    .ok();
}

/// Asks the platform to drop the "processing" indicator for a session
/// whose flush produced no task run. A missing TypingNotifier is a no-op.
async fn clear_typing(set: &Arc<ResolverSet>, session_id: Uuid) {
    if let Some(typing) = &set.typing {
        typing
            .on_settled(CancellationToken::new(), session_id)
            .await;
    }
}

/// Delivers an offline/archived notice for a flushed run.
async fn emit_flush_reply(
    set: &Arc<ResolverSet>,
    inst: &ResolvedInstallation,
    msg: &InboundMessage,
    session_id: Uuid,
    outcome: Outcome,
) {
    let Some(replier) = &set.replier else { return };
    replier
        .reply(
            CancellationToken::new(),
            inst,
            msg,
            &Result {
                outcome: Some(outcome),
                installation_id: Some(inst.id),
                chat_session_id: Some(session_id),
                sender: msg.source.sender_id.clone(),
                ..Default::default()
            },
        )
        .await;
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[tokio::test]
    async fn tracked_jobs_abort_overdue_work_and_close_admission() {
        let jobs = TrackedJobs::default();
        assert!(jobs.spawn(std::future::pending()));

        assert!(!jobs.shutdown(Duration::from_millis(1)).await);
        assert!(!jobs.spawn(async {}));
        assert!(jobs
            .handles
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty());
    }

    #[tokio::test]
    async fn media_work_is_dropped_when_owner_is_cancelled() {
        let parent = CancellationToken::new();
        let child = parent.child_token();
        parent.cancel();

        let result = run_media_until(
            &parent,
            &child,
            std::time::Instant::now() + Duration::from_secs(30),
            std::future::pending::<()>(),
        )
        .await;

        assert!(result.is_none());
        assert!(child.is_cancelled());
    }

    #[tokio::test]
    async fn media_work_is_dropped_at_its_deadline() {
        let parent = CancellationToken::new();
        let child = parent.child_token();

        let result = run_media_until(
            &parent,
            &child,
            std::time::Instant::now(),
            std::future::pending::<()>(),
        )
        .await;

        assert!(result.is_none());
        assert!(child.is_cancelled());
    }
}
