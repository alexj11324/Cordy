//! The shared, channel-agnostic chat-session service every IM adapter
//! reuses (PB-3516).
//!
//! Database-backed session operations
//! (ChatSession: EnsureSession / MarkPendingFresh / AppendUserMessage /
//! BindMediaRefs). Lifted out of the Feishu-specific lark.chatSessionService
//! so adding an IM never re-implements the session/append/`/issue`
//! machinery — the platform adapter contributes only a channel_type, its
//! session titles, and the command-parse source.
//!
//! Port note: Go abstracts `*db.Queries` + `*pgxpool.Pool` behind
//! SessionQueries/TxStarter interfaces so tests can fake them. Rust keeps
//! one `PgPool` handle (sqlx transactions borrow it directly); integration
//! tests run against the real database per the workspace convention.

use sqlx::PgPool;
use uuid::Uuid;

#[async_trait::async_trait]
pub trait AppendFence: Send + Sync {
    async fn before_write(&self, tx: &mut sqlx::PgConnection) -> anyhow::Result<()>;
}

use cordy_channel::{ChatType, MediaRef, MsgType};
use cordy_db::dbid;
use cordy_db::queries::attachment::{create_attachment, link_attachments_to_chat_message};
use cordy_db::queries::channel::{
    claim_channel_media_pending_objects_for_bind as claim_media_intents,
    create_channel_chat_session_binding, get_channel_chat_session_binding,
    mark_channel_chat_session_pending_fresh, mark_channel_inbound_dedup_processed,
    update_channel_chat_session_binding_reply_target,
};
use cordy_db::queries::chat::{
    create_chat_message, create_chat_session, lock_chat_session_for_append, touch_chat_session,
    update_chat_message_content_for_channel_media,
};
use cordy_db::queries::issue::{
    lock_issue_for_channel_media_bind, materialize_issue_channel_media_markdown,
};
use cordy_db::queries::workspace::lock_workspace_for_chat_session_create;

use crate::issue_command::parse_issue_command;
use crate::resolvers::{AppendResult, ResolverError};
use crate::session_media::{
    compose_inline_media_body, compose_issue_command_media_description, default_media_filename,
    inline_attachment_markdown, InlineMediaReplacement,
};

/// SQLSTATE unique_violation — the create-race arbiter.
const PG_UNIQUE_VIOLATION: &str = "23505";

/// Marks a durable control-plane turn handled synchronously by Router.
/// Public Chat projections omit it, and the task batch seal does too so
/// the agent cannot execute the command again on a later turn.
const CHANNEL_COMMAND_MESSAGE_KIND: &str = "channel_command";

/// Per-platform display titles a freshly created chat_session gets (the
/// first message has not been appended yet, so the title cannot be derived
/// from content). The adapter supplies its own wording.
#[derive(Debug, Clone, Default)]
pub struct SessionTitles {
    pub group: String,
    pub direct: String,
    pub fallback: String,
}

impl SessionTitles {
    fn for_type(&self, ct: &ChatType) -> &str {
        match ct.0.as_str() {
            "group" => &self.group,
            "p2p" => &self.direct,
            _ => &self.fallback,
        }
    }
}

/// The channel-agnostic input for [`ChatSession::ensure_session`].
///
/// `binding_key` is the SESSION-ISOLATION key (stored as channel_chat_id;
/// one chat_session per (installation_id, binding_key)). It is
/// intentionally NOT the same thing as "the chat to reply into": the
/// adapter composes it so that distinct conversations get distinct
/// sessions — Feishu passes the chat id; Slack passes the channel id for a
/// DM, and the channel id PLUS the thread root for a channel/thread. A raw
/// platform chat id must never be passed straight through as the key for a
/// threaded platform.
#[derive(Debug, Clone)]
pub struct EnsureSessionInput {
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub installation_id: Uuid,
    pub sender: Uuid,
    pub binding_key: String,
    /// Opaque platform routing the key alone cannot carry; `None` stores
    /// `{}` (Go nil → `{}`).
    pub binding_config: Option<serde_json::Value>,
    pub chat_type: ChatType,
}

/// The channel-agnostic input for [`ChatSession::append_user_message`].
/// `body` is the full stored text (including any platform enrichment);
/// `command_text` is the user's OWN typed text used for /issue parsing
/// (empty falls back to body). `claim_token` is the dedup owner-fence.
/// `message_id`/`thread_id` are the REAL platform ids of this trigger —
/// the outbound reply target recorded on the binding, NOT the session
/// binding key.
#[derive(Debug, Clone)]
pub struct AppendInput {
    pub session_id: Uuid,
    pub sender: Uuid,
    pub installation_id: Uuid,
    pub body: String,
    pub command_text: String,
    pub message_id: String,
    pub thread_id: String,
    pub claim_token: Option<Uuid>,
    pub media_pending_seconds: f64,
    pub force_fresh: bool,
}

/// Links already-uploaded media to either an /issue target or a durable
/// chat message in a short database-only transaction. A valid
/// `issue_description_base` permits inline replacement only while the
/// issue still has its exact creation-time description; otherwise issue
/// media appends as a concurrency-safe fallback.
#[derive(Debug, Clone)]
pub struct BindMediaInput {
    pub message_id: Uuid,
    pub session_id: Uuid,
    pub workspace_id: Uuid,
    pub sender: Uuid,
    pub issue_id: Option<Uuid>,
    pub issue_description_base: Option<String>,
    pub issue_command_text: String,
    pub body: String,
    pub media_refs: Vec<MediaRef>,
}

/// The shared chat-session service. One instance is built per channel_type
/// (so the binding rows carry the right discriminator); the logic is
/// otherwise platform-neutral.
#[derive(Clone)]
pub struct ChatSession {
    pool: PgPool,
    channel_type: cordy_channel::Type,
    titles: SessionTitles,
}

impl ChatSession {
    /// Builds the shared service over the pool. The dedup Mark runs inside
    /// append_user_message's transaction so the durable write and the Mark
    /// commit (or roll back) together.
    pub fn new(pool: PgPool, channel_type: cordy_channel::Type, titles: SessionTitles) -> Self {
        Self {
            pool,
            channel_type,
            titles,
        }
    }

    /// Returns the chat_session.id bound to (installation, binding_key),
    /// creating it (with its channel_chat_session_binding) on first
    /// contact. The race between two concurrent first messages is resolved
    /// by the UNIQUE (installation_id, channel_chat_id) constraint: the
    /// loser re-reads the winner's row.
    pub async fn ensure_session(&self, input: &EnsureSessionInput) -> anyhow::Result<Uuid> {
        match get_channel_chat_session_binding(
            &self.pool,
            input.installation_id,
            &input.binding_key,
        )
        .await
        {
            Ok(Some(existing)) => return Ok(existing.chat_session_id),
            Ok(None) => {}
            Err(err) => return Err(anyhow::anyhow!("lookup chat session binding: {err:#}")),
        }

        match self.create_session_and_binding(input).await {
            Ok(id) => Ok(id),
            Err(err) => {
                if is_unique_violation(&err) {
                    let existing = get_channel_chat_session_binding(
                        &self.pool,
                        input.installation_id,
                        &input.binding_key,
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("race re-read after unique violation: {e:#}"))?;
                    match existing {
                        Some(row) => Ok(row.chat_session_id),
                        None => Err(err),
                    }
                } else {
                    Err(err)
                }
            }
        }
    }

    async fn create_session_and_binding(&self, input: &EnsureSessionInput) -> anyhow::Result<Uuid> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| anyhow::anyhow!("begin tx: {e:#}"))?;
        // FOR KEY SHARE on the workspace row before creating the session —
        // the creator half of the #5219 delete/create protocol, so a
        // channel session cannot be created into a workspace mid-delete.
        lock_workspace_for_chat_session_create(&mut *tx, input.workspace_id)
            .await
            .map_err(|e| anyhow::anyhow!("lock workspace for chat session create: {e:#}"))?;

        let session = create_chat_session(
            &mut *tx,
            input.workspace_id,
            input.agent_id,
            input.sender,
            self.titles.for_type(&input.chat_type),
            // is_agent_intro: channel sessions are user-initiated.
            false,
            // project_id: channel sessions live outside projects; the
            // query treats the zero UUID as NULL via its COALESCE guard.
            Uuid::nil(),
            dbid::new_v7(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("create chat session: {e:#}"))?;
        let Some(session) = session else {
            anyhow::bail!("create chat session: no row returned");
        };
        let binding_config = input
            .binding_config
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        create_channel_chat_session_binding(
            &mut *tx,
            session.id,
            input.installation_id,
            &self.channel_type.0,
            &input.binding_key,
            &input.chat_type.0,
            &binding_config,
        )
        .await
        .map_err(|e| anyhow::anyhow!("create channel binding: {e:#}"))?;
        tx.commit()
            .await
            .map_err(|e| anyhow::anyhow!("commit: {e:#}"))?;
        Ok(session.id)
    }

    /// Persists a bare `/new` command. Non-bare `/new` messages mark the
    /// same flag inside append_user_message's transaction instead.
    pub async fn mark_pending_fresh(&self, session_id: Uuid) -> anyhow::Result<()> {
        mark_channel_chat_session_pending_fresh(&self.pool, session_id)
            .await
            .map_err(|e| anyhow::anyhow!("mark pending fresh: {e:#}"))?;
        Ok(())
    }

    /// Writes the user message into the chat_session (touching it and
    /// recording the reply target), runs the in-tx dedup Mark when a claim
    /// token is supplied, and returns the durable message id plus the
    /// parsed `/issue` command when present. Returns
    /// [`ResolverError::ClaimLost`] when a concurrent reclaim rotated the
    /// dedup token mid-flight, in which case the whole transaction rolls
    /// back (no chat_message lands).
    pub async fn append_user_message(&self, input: &AppendInput) -> anyhow::Result<AppendResult> {
        self.append_user_message_fenced(input, None).await
    }

    pub async fn append_user_message_fenced(
        &self,
        input: &AppendInput,
        fence: Option<&dyn AppendFence>,
    ) -> anyhow::Result<AppendResult> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| anyhow::anyhow!("begin tx: {e:#}"))?;
        // Keep the repo-wide teardown order: chat_session before any
        // adapter-owned route fence. FOR KEY SHARE is sufficient to
        // serialize deletion without blocking normal non-key session
        // updates or debounced task enqueueing.
        lock_chat_session_for_append(&mut *tx, input.session_id)
            .await
            .map_err(|e| anyhow::anyhow!("lock chat session for append: {e:#}"))?;
        if let Some(fence) = fence {
            fence.before_write(&mut tx).await?;
        }

        let command_source = if input.command_text.is_empty() {
            input.body.as_str()
        } else {
            input.command_text.as_str()
        };
        let cmd = parse_issue_command(command_source);

        // channel_ingested is the immutable provenance the cancel path
        // gates on: it must be stamped in the same transaction as the
        // message so no later binding deletion can strip it.
        let msg = create_chat_message(
            &mut *tx,
            input.session_id,
            "user",
            &input.body,
            // task_id: user turns are unowned at write time.
            None,
            None,
            None,
            cmd.as_ref().map(|_| CHANNEL_COMMAND_MESSAGE_KIND),
            // quick_actions: none on ingest.
            &serde_json::json!([]),
            (input.media_pending_seconds > 0.0).then_some(input.media_pending_seconds),
            Some(true),
            dbid::new_v7(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("create chat message: {e:#}"))?;
        let Some(msg) = msg else {
            anyhow::bail!("create chat message: no row returned");
        };
        touch_chat_session(&mut *tx, input.session_id)
            .await
            .map_err(|e| anyhow::anyhow!("touch chat session: {e:#}"))?;
        if input.force_fresh {
            mark_channel_chat_session_pending_fresh(&mut *tx, input.session_id)
                .await
                .map_err(|e| anyhow::anyhow!("mark pending fresh: {e:#}"))?;
        }

        // Record the latest trigger so the decoupled outbound patcher can
        // thread its reply back into the originating topic.
        if !input.message_id.is_empty() {
            update_channel_chat_session_binding_reply_target(
                &mut *tx,
                input.session_id,
                (!input.message_id.is_empty()).then_some(input.message_id.as_str()),
                (!input.thread_id.is_empty()).then_some(input.thread_id.as_str()),
            )
            .await
            .map_err(|e| anyhow::anyhow!("update reply target: {e:#}"))?;
        }

        let mut marked_in_tx = false;
        if let Some(token) = input.claim_token {
            if !input.message_id.is_empty() {
                let rows = mark_channel_inbound_dedup_processed(
                    &mut *tx,
                    input.installation_id,
                    &input.message_id,
                    token,
                )
                .await
                .map_err(|e| anyhow::anyhow!("mark dedup processed: {e:#}"))?;
                if rows == 0 {
                    // Another worker re-claimed the dedup row; roll back
                    // via the dropped tx so no second chat_message lands.
                    return Err(ResolverError::ClaimLost.into());
                }
                marked_in_tx = true;
            }
        }

        tx.commit()
            .await
            .map_err(|e| anyhow::anyhow!("commit: {e:#}"))?;
        Ok(AppendResult {
            message_id: Some(msg.id),
            issue_command: cmd,
            dedup_marked: marked_in_tx,
        })
    }

    /// Creates attachment rows owned by IssueID when present, otherwise
    /// links them to the existing durable chat message. It also clears the
    /// message's media-pending marker. A failure rolls back the attachment
    /// rows, then clears the marker separately so the placeholder can be
    /// promoted immediately for graceful degradation.
    pub async fn bind_media_refs(&self, input: &BindMediaInput) -> anyhow::Result<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| anyhow::anyhow!("begin media tx: {e:#}"))?;
        if !input.media_refs.is_empty() {
            if let Err(err) = self.bind_media_refs_inner(&mut tx, input).await {
                // Explicit rollback, then clear the pending marker on the
                // pool so the placeholder can be promoted immediately.
                tx.rollback().await.ok();
                self.clear_media_pending(&self.pool, input).await?;
                return Err(err);
            }
        }
        if let Err(err) = self.clear_media_pending(&mut *tx, input).await {
            tx.rollback().await.ok();
            return Err(err);
        }
        // An ambiguous commit needs no adjudication: the intent-ledger rows
        // were deleted in this same transaction, so commit landed ⇔
        // intents gone, atomically.
        tx.commit()
            .await
            .map_err(|e| anyhow::anyhow!("commit media: {e:#}"))?;
        Ok(())
    }

    async fn clear_media_pending<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(
        &self,
        executor: E,
        input: &BindMediaInput,
    ) -> anyhow::Result<()> {
        cordy_db::queries::chat::clear_chat_message_channel_media_pending(
            executor,
            input.message_id,
            input.session_id,
        )
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("clear chat message media pending: {e:#}"))
    }

    async fn bind_media_refs_inner(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        input: &BindMediaInput,
    ) -> anyhow::Result<()> {
        for r#ref in &input.media_refs {
            if r#ref.storage_url.is_empty() {
                anyhow::bail!("bind media refs: storage_url is required");
            }
            if r#ref.storage_key.is_empty() {
                anyhow::bail!("bind media refs: storage_key is required");
            }
        }
        if let Some(issue_id) = input.issue_id {
            lock_issue_for_channel_media_bind(&mut **tx, issue_id, input.workspace_id)
                .await
                .map_err(|e| anyhow::anyhow!("validate issue media target: {e:#}"))?;
        }
        let keys: Vec<String> = input
            .media_refs
            .iter()
            .map(|r| r.storage_key.clone())
            .collect();
        // Claim the intent-ledger rows inside this same transaction:
        // commit landed <=> intents gone, atomically. A key the reconciler
        // already moved to 'deleting' is not returned and its ref must NOT
        // attach.
        let claimed_keys = claim_media_intents(&mut **tx, &keys, input.workspace_id)
            .await
            .map_err(|e| anyhow::anyhow!("claim media intents: {e:#}"))?;
        let claimed: std::collections::HashSet<&str> =
            claimed_keys.iter().map(String::as_str).collect();

        struct CreatedMedia {
            id: Uuid,
            r#ref: MediaRef,
            filename: String,
        }
        let mut created: Vec<CreatedMedia> = Vec::with_capacity(input.media_refs.len());
        let mut ids: Vec<Uuid> = Vec::with_capacity(input.media_refs.len());
        for r#ref in &input.media_refs {
            if !claimed.contains(r#ref.storage_key.as_str()) {
                tracing::warn!(
                    storage_key = %r#ref.storage_key,
                    "channel media: intent claimed by reconciler; skipping attach"
                );
                continue;
            }
            let id = dbid::new_v7();
            let content_type = if r#ref.mime_type.is_empty() {
                "application/octet-stream"
            } else {
                r#ref.mime_type.as_str()
            };
            let filename = if r#ref.filename.is_empty() {
                default_media_filename(&r#ref.r#type.0, &id.to_string(), content_type)
            } else {
                r#ref.filename.clone()
            };
            // Issue-owned attachments detach from the session.
            let chat_session_id = if input.issue_id.is_some() {
                None
            } else {
                Some(input.session_id)
            };
            let att = create_attachment(
                &mut **tx,
                id,
                input.workspace_id,
                "member",
                input.sender,
                &filename,
                &r#ref.storage_url,
                content_type,
                r#ref.size_bytes,
                input.issue_id,
                None,
                chat_session_id,
                None,
            )
            .await
            .map_err(|e| anyhow::anyhow!("create channel attachment: {e:#}"))?;
            let Some(att) = att else {
                anyhow::bail!("create channel attachment: no row returned");
            };
            let att_id = att
                .id
                .ok_or_else(|| anyhow::anyhow!("create channel attachment: row missing id"))?;
            ids.push(att_id);
            created.push(CreatedMedia {
                id: att_id,
                r#ref: r#ref.clone(),
                filename,
            });
        }
        if ids.is_empty() {
            return Ok(());
        }
        if let Some(issue_id) = input.issue_id {
            let mut issue_markdown: Vec<String> = Vec::with_capacity(created.len());
            let mut replacements: Vec<InlineMediaReplacement> = Vec::new();
            for media in &created {
                let block = cordy_util::channel_media::block(
                    &media.id.to_string(),
                    &media.filename,
                    media.r#ref.r#type.0 == MsgType::image().0,
                );
                issue_markdown.push(block.clone());
                if !media.r#ref.inline_placeholder.is_empty() {
                    replacements.push(InlineMediaReplacement {
                        placeholder: media.r#ref.inline_placeholder.clone(),
                        index: media.r#ref.inline_index as i32,
                        markdown: block,
                    });
                }
            }
            let (base, description): (Option<&str>, String) = match &input.issue_description_base {
                Some(base_str) => {
                    let (composed, changed) = compose_issue_command_media_description(
                        &input.body,
                        &input.issue_command_text,
                        &replacements,
                        base_str,
                    );
                    if changed {
                        (Some(base_str.as_str()), composed)
                    } else {
                        // No inline change: append-only materialization.
                        (None, String::new())
                    }
                }
                None => (None, String::new()),
            };
            materialize_issue_channel_media_markdown(
                &mut **tx,
                base,
                &description,
                Some(&issue_markdown.join("\n\n")),
                issue_id,
                input.workspace_id,
            )
            .await
            .map_err(|e| anyhow::anyhow!("materialize issue channel media markdown: {e:#}"))?;
            return Ok(());
        }
        let linked = link_attachments_to_chat_message(
            &mut **tx,
            input.message_id,
            input.session_id,
            input.workspace_id,
            "member",
            input.sender,
            ids.clone(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("link chat attachments: {e:#}"))?;
        let linked: std::collections::HashSet<Uuid> = linked.into_iter().flatten().collect();

        let mut replacements: Vec<InlineMediaReplacement> = Vec::new();
        for media in &created {
            if !linked.contains(&media.id) || media.r#ref.inline_placeholder.is_empty() {
                continue;
            }
            replacements.push(InlineMediaReplacement {
                placeholder: media.r#ref.inline_placeholder.clone(),
                index: media.r#ref.inline_index as i32,
                markdown: inline_attachment_markdown(&media.r#ref, media.id),
            });
        }
        let (body, changed) = compose_inline_media_body(&input.body, &replacements);
        if changed {
            let rows = update_chat_message_content_for_channel_media(
                &mut **tx,
                &body,
                input.message_id,
                input.session_id,
            )
            .await
            .map_err(|e| anyhow::anyhow!("update chat message inline media: {e:#}"))?;
            if rows != 1 {
                anyhow::bail!("update chat message inline media: updated {rows} rows");
            }
        }
        Ok(())
    }
}

fn is_unique_violation(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<sqlx::postgres::PgDatabaseError>()
            .is_some_and(|pg| pg.code() == PG_UNIQUE_VIOLATION)
    })
}

// Re-exported for the Router wiring slice (Go: NewDBMediaIntentLedger).
pub use cordy_db::queries::channel::record_channel_media_pending_object;
