//! Production data layer for the Feishu integration.
//!
//! ChannelStore is the Feishu-flavored facade over the channel_* queries
//! (PB-3515 generalized lark_* into channel_*). It adds the feishu-specific
//! store methods, each backed by a channel_* query and translating at the
//! JSONB-config boundary ([`crate::store`]).
//!
//! The methods take and return the crate's flat domain types
//! ([`Installation`](crate::store::Installation),
//! [`UserBinding`](crate::store::UserBinding), …) and the param structs in
//! [`crate::params`]. This store reads and writes only channel_*, never
//! lark_*.
//!
//! Port notes:
//!
//! - Go's `pgx.ErrNoRows` becomes [`ErrNoRows`]; call sites that matched the
//!   sentinel (`errors.Is(err, pgx.ErrNoRows)`) downcast instead. Methods
//!   whose only "error" was a missing row return it unchanged so behavior
//!   carries over 1:1.
//! - Go's `WithTx` disappears: every query in `cordy_db::queries` is
//!   executor-generic, so transactional callers (registration finalize)
//!   either use the `_with(executor, …)` variants below or pass the tx
//!   connection directly.
//! - `record_channel_inbound_drop` keeps installation_id optional so an
//!   installation-less event remains SQL NULL, matching Go's invalid
//!   pgtype.UUID representation.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use cordy_db::dbid;
use cordy_db::queries::channel::{
    acquire_channel_ws_lease, backfill_channel_installation_region_to_feishu_lark,
    claim_channel_inbound_dedup, consume_channel_binding_token, create_channel_binding_token,
    create_channel_outbound_card_message, create_channel_user_binding,
    get_channel_chat_session_binding, get_channel_chat_session_binding_by_session,
    get_channel_installation, get_channel_installation_by_app_id,
    get_channel_installation_in_workspace, get_channel_installation_owner_by_app_id,
    get_channel_outbound_card_by_task, get_channel_user_binding_by_user_id,
    list_active_channel_installations, list_channel_installations_by_workspace,
    mark_channel_inbound_dedup_processed, reclaim_dead_channel_installation_by_app_id,
    record_channel_inbound_drop, release_channel_inbound_dedup, release_channel_ws_lease,
    set_channel_installation_config, set_channel_installation_status,
    update_channel_chat_session_binding_reply_target, update_channel_outbound_card_status,
    upsert_channel_installation, GetChannelInstallationOwnerByAppIDRow,
};
use cordy_db::queries::member::get_member_by_user_and_workspace;

use crate::params::{
    AcquireWsLeaseParams, ClaimInboundDedupParams, CreateBindingTokenParams,
    CreateOutboundCardMessageParams, CreateUserBindingParams, GetChatSessionBindingParams,
    GetInstallationInWorkspaceParams, GetUserBindingByOpenIdParams,
    MarkInboundDedupProcessedParams, RecordInboundDropParams, ReleaseInboundDedupParams,
    ReleaseWsLeaseParams, SetInstallationBotUnionIdParams, SetInstallationStatusParams,
    UpdateChatSessionBindingReplyTargetParams, UpdateOutboundCardStatusParams,
    UpsertInstallationParams,
};
use crate::store::{
    binding_token_from_row, chat_session_binding_from_row, dedup_from_row, encode_binding_config,
    encode_install_config, installation_from_row, outbound_card_from_row, user_binding_from_row,
    BindingTokenRow, ChatSessionBinding, InboundMessageDedup, Installation, OutboundCardMessage,
    UserBinding,
};

/// The channel_type discriminator for every row this Feishu-backed store
/// reads or writes.
pub const CHANNEL_TYPE_FEISHU: &str = "feishu";

/// Mirrors Go's `pgx.ErrNoRows`: no row matched. Call sites that branched on
/// `errors.Is(err, pgx.ErrNoRows)` downcast this instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("lark store: no rows")]
pub struct ErrNoRows;

/// Reports whether err is the store's not-found sentinel.
pub fn is_no_rows(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<ErrNoRows>().is_some())
}

/// The Postgres SQLSTATE for a unique-constraint violation. A rebind upsert
/// that trips the (channel_type, config->>'app_id') index after the
/// dead-owner reclaim ran means a LIVE owner still holds the slot.
pub const PG_UNIQUE_VIOLATION: &str = "23505";

/// Reports whether err carries a Postgres unique-constraint violation.
pub fn is_unique_violation(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<sqlx::postgres::PgDatabaseError>()
            .is_some_and(|pg| pg.code() == PG_UNIQUE_VIOLATION)
    })
}

/// Wraps a pool so the lark package's DB seams resolve to channel_* rows.
#[derive(Clone)]
pub struct ChannelStore {
    pool: PgPool,
}

impl ChannelStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The underlying pool, for callers composing their own transactions.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Reports whether user_id is currently a member of workspace_id. With
    /// the lark_user_binding → member foreign key removed (PB-3515 §4), a
    /// binding row no longer proves membership, so the inbound identity step
    /// calls this to re-check it explicitly. No rows → not a member.
    pub async fn is_workspace_member(
        &self,
        workspace_id: Uuid,
        user_id: Uuid,
    ) -> anyhow::Result<bool> {
        Ok(
            get_member_by_user_and_workspace(&self.pool, user_id, workspace_id)
                .await?
                .is_some(),
        )
    }

    // ---- installation ----

    pub async fn get_lark_installation_by_app_id(
        &self,
        app_id: &str,
    ) -> anyhow::Result<Installation> {
        let Some(row) =
            get_channel_installation_by_app_id(&self.pool, CHANNEL_TYPE_FEISHU, app_id).await?
        else {
            return Err(ErrNoRows.into());
        };
        installation_from_row(row)
    }

    pub async fn get_lark_installation(&self, id: Uuid) -> anyhow::Result<Installation> {
        let Some(row) = get_channel_installation(&self.pool, id, CHANNEL_TYPE_FEISHU).await? else {
            return Err(ErrNoRows.into());
        };
        installation_from_row(row)
    }

    pub async fn get_lark_installation_in_workspace(
        &self,
        arg: GetInstallationInWorkspaceParams,
    ) -> anyhow::Result<Installation> {
        let Some(row) = get_channel_installation_in_workspace(
            &self.pool,
            arg.id,
            arg.workspace_id,
            CHANNEL_TYPE_FEISHU,
        )
        .await?
        else {
            return Err(ErrNoRows.into());
        };
        installation_from_row(row)
    }

    pub async fn list_lark_installations_by_workspace(
        &self,
        workspace_id: Uuid,
    ) -> anyhow::Result<Vec<Installation>> {
        installations_from_rows(
            list_channel_installations_by_workspace(&self.pool, workspace_id, CHANNEL_TYPE_FEISHU)
                .await?,
        )
    }

    pub async fn list_active_lark_installations(&self) -> anyhow::Result<Vec<Installation>> {
        installations_from_rows(
            list_active_channel_installations(&self.pool, CHANNEL_TYPE_FEISHU).await?,
        )
    }

    pub async fn list_lark_installations_missing_bot_union_id_after(
        &self,
        after: Option<(DateTime<Utc>, Uuid)>,
        limit: i64,
    ) -> anyhow::Result<Vec<Installation>> {
        let (created_at, id) = after.unzip();
        installations_from_rows(
            cordy_db::queries::channel::list_active_channel_installations_missing_bot_union_id_after(
                &self.pool,
                CHANNEL_TYPE_FEISHU,
                created_at,
                id,
                limit,
            )
            .await?,
        )
    }

    pub async fn upsert_lark_installation(
        &self,
        arg: UpsertInstallationParams,
    ) -> anyhow::Result<Installation> {
        upsert_lark_installation_with(&self.pool, arg).await
    }

    /// Frees the (feishu, config->>'app_id') routing slot before a rebind by
    /// removing a DEAD prior owner of the same Lark/Feishu app — a revoked
    /// placeholder left by a DIFFERENT agent in this workspace, or an ORPHAN
    /// whose owning workspace/agent has been deleted (#4810) — together with
    /// every dependent row of that installation, in a single statement. A
    /// live owner is deliberately left in place: the SAME agent's own revoked
    /// row (reactivated by the follow-up upsert), and any ACTIVE owner whose
    /// agent still exists — including an ARCHIVED agent, since archiving is
    /// reversible — so the upsert surfaces a conflict instead of silently
    /// stealing the bot. See the full contract, and the TOCTOU /
    /// EvalPlanQual reasoning, on ReclaimDeadChannelInstallationByAppID.
    ///
    /// "Nothing was dead" is a no-op, not a failure (Go swallowed
    /// pgx.ErrNoRows here).
    pub async fn reclaim_dead_installation_by_app_id(
        &self,
        workspace_id: Uuid,
        agent_id: Uuid,
        app_id: &str,
    ) -> anyhow::Result<()> {
        reclaim_dead_installation_with(&self.pool, workspace_id, agent_id, app_id).await
    }

    /// Returns the current owner of the (feishu, app_id) routing slot so the
    /// caller can build an accurate rebind-conflict message. Called after
    /// [`Self::reclaim_dead_installation_by_app_id`], so a row here is a live
    /// owner; None means the slot is free.
    pub async fn installation_owner_by_app_id(
        &self,
        app_id: &str,
    ) -> anyhow::Result<Option<GetChannelInstallationOwnerByAppIDRow>> {
        get_channel_installation_owner_by_app_id(&self.pool, CHANNEL_TYPE_FEISHU, app_id).await
    }

    pub async fn set_lark_installation_status(
        &self,
        arg: SetInstallationStatusParams,
    ) -> anyhow::Result<()> {
        set_channel_installation_status(&self.pool, arg.id, &arg.status).await?;
        Ok(())
    }

    /// Folds bot_union_id into the JSONB config via a read-modify-write
    /// through set_channel_installation_config (channel_installation has no
    /// dedicated union_id column). This is the operator union_id backfill,
    /// keyed by id and effectively single-writer, so the non-atomic RMW is
    /// safe — the same shape the channel.sql comment documents for this
    /// query.
    pub async fn set_lark_installation_bot_union_id(
        &self,
        arg: SetInstallationBotUnionIdParams,
    ) -> anyhow::Result<()> {
        let Some(row) = get_channel_installation(&self.pool, arg.id, CHANNEL_TYPE_FEISHU).await?
        else {
            return Err(ErrNoRows.into());
        };
        let mut inst = installation_from_row(row)?;
        inst.bot_union_id = arg.bot_union_id;
        let cfg = encode_install_config(&inst)?;
        set_channel_installation_config(&self.pool, arg.id, &cfg).await?;
        Ok(())
    }

    pub async fn stamp_lark_installation_bot_union_id_if_missing(
        &self,
        id: Uuid,
        bot_union_id: &str,
    ) -> anyhow::Result<bool> {
        cordy_db::queries::channel::set_channel_installation_bot_union_id_if_missing(
            &self.pool,
            id,
            CHANNEL_TYPE_FEISHU,
            bot_union_id,
        )
        .await
    }

    pub async fn backfill_lark_installation_region_to_lark(&self) -> anyhow::Result<u64> {
        backfill_channel_installation_region_to_feishu_lark(&self.pool).await
    }

    // ---- WS lease ----

    /// Fences the WS supervisor lease. [`ErrNoRows`] means the lease was NOT
    /// acquired (another live owner holds it).
    pub async fn acquire_lark_ws_lease(
        &self,
        arg: AcquireWsLeaseParams,
    ) -> anyhow::Result<Installation> {
        let Some(row) = acquire_channel_ws_lease(
            &self.pool,
            arg.new_token.as_deref(),
            arg.new_expires_at,
            arg.id,
        )
        .await?
        else {
            return Err(ErrNoRows.into());
        };
        installation_from_row(row)
    }

    pub async fn release_lark_ws_lease(&self, arg: ReleaseWsLeaseParams) -> anyhow::Result<()> {
        release_channel_ws_lease(&self.pool, arg.id, arg.current_token.as_deref()).await?;
        Ok(())
    }

    // ---- user binding ----

    pub async fn get_lark_user_binding_by_open_id(
        &self,
        arg: GetUserBindingByOpenIdParams,
    ) -> anyhow::Result<UserBinding> {
        let Some(row) = get_channel_user_binding_by_user_id(
            &self.pool,
            arg.installation_id,
            &arg.channel_user_id,
        )
        .await?
        else {
            return Err(ErrNoRows.into());
        };
        user_binding_from_row(row)
    }

    pub async fn create_lark_user_binding(
        &self,
        arg: CreateUserBindingParams,
    ) -> anyhow::Result<UserBinding> {
        create_lark_user_binding_with(&self.pool, arg).await
    }

    // ---- chat session binding ----

    pub async fn get_lark_chat_session_binding(
        &self,
        arg: GetChatSessionBindingParams,
    ) -> anyhow::Result<ChatSessionBinding> {
        let Some(row) =
            get_channel_chat_session_binding(&self.pool, arg.installation_id, &arg.channel_chat_id)
                .await?
        else {
            return Err(ErrNoRows.into());
        };
        Ok(chat_session_binding_from_row(row))
    }

    pub async fn get_lark_chat_session_binding_by_session(
        &self,
        chat_session_id: Uuid,
    ) -> anyhow::Result<ChatSessionBinding> {
        let Some(row) = get_channel_chat_session_binding_by_session(
            &self.pool,
            chat_session_id,
            CHANNEL_TYPE_FEISHU,
        )
        .await?
        else {
            return Err(ErrNoRows.into());
        };
        Ok(chat_session_binding_from_row(row))
    }

    pub async fn update_lark_chat_session_binding_reply_target(
        &self,
        arg: UpdateChatSessionBindingReplyTargetParams,
    ) -> anyhow::Result<()> {
        update_channel_chat_session_binding_reply_target(
            &self.pool,
            arg.chat_session_id,
            arg.last_message_id.as_deref(),
            arg.last_thread_id.as_deref(),
        )
        .await?;
        Ok(())
    }

    // ---- inbound dedup ----

    /// Claims the two-phase idempotency row. [`ErrNoRows`] is the DUPLICATE
    /// signal: already processed or claimed in flight.
    pub async fn claim_lark_inbound_dedup(
        &self,
        arg: ClaimInboundDedupParams,
    ) -> anyhow::Result<InboundMessageDedup> {
        let Some(row) =
            claim_channel_inbound_dedup(&self.pool, arg.installation_id, &arg.message_id).await?
        else {
            return Err(ErrNoRows.into());
        };
        Ok(dedup_from_row(row))
    }

    pub async fn mark_lark_inbound_dedup_processed(
        &self,
        arg: MarkInboundDedupProcessedParams,
    ) -> anyhow::Result<u64> {
        mark_channel_inbound_dedup_processed(
            &self.pool,
            arg.installation_id,
            &arg.message_id,
            arg.claim_token,
        )
        .await
    }

    pub async fn release_lark_inbound_dedup(
        &self,
        arg: ReleaseInboundDedupParams,
    ) -> anyhow::Result<u64> {
        release_channel_inbound_dedup(
            &self.pool,
            arg.installation_id,
            &arg.message_id,
            arg.claim_token,
        )
        .await
    }

    // ---- audit ----

    /// Writes a non-content drop audit row. An installation-less event passes
    /// None so the audit row preserves SQL NULL.
    pub async fn record_lark_inbound_drop(
        &self,
        arg: RecordInboundDropParams,
    ) -> anyhow::Result<()> {
        record_channel_inbound_drop(
            &self.pool,
            CHANNEL_TYPE_FEISHU,
            &arg.event_type,
            &arg.drop_reason,
            arg.installation_id,
            arg.channel_chat_id.as_deref(),
            arg.channel_event_id.as_deref(),
            arg.channel_message_id.as_deref(),
            dbid::new_v7(),
        )
        .await?;
        Ok(())
    }

    // ---- binding token ----

    pub async fn create_lark_binding_token(
        &self,
        arg: CreateBindingTokenParams,
    ) -> anyhow::Result<BindingTokenRow> {
        let Some(row) = create_channel_binding_token(
            &self.pool,
            &arg.token_hash,
            arg.workspace_id,
            arg.installation_id,
            CHANNEL_TYPE_FEISHU,
            &arg.channel_user_id,
            Some(arg.expires_at),
        )
        .await?
        else {
            return Err(ErrNoRows.into());
        };
        Ok(binding_token_from_row(row))
    }

    pub async fn consume_lark_binding_token(
        &self,
        token_hash: &str,
    ) -> anyhow::Result<BindingTokenRow> {
        let Some(row) = consume_channel_binding_token(&self.pool, token_hash).await? else {
            return Err(ErrNoRows.into());
        };
        Ok(binding_token_from_row(row))
    }

    // ---- outbound card ----

    pub async fn get_lark_outbound_card_by_task(
        &self,
        task_id: Uuid,
    ) -> anyhow::Result<OutboundCardMessage> {
        let Some(row) =
            get_channel_outbound_card_by_task(&self.pool, task_id, CHANNEL_TYPE_FEISHU).await?
        else {
            return Err(ErrNoRows.into());
        };
        Ok(outbound_card_from_row(row))
    }

    pub async fn create_lark_outbound_card_message(
        &self,
        arg: CreateOutboundCardMessageParams,
    ) -> anyhow::Result<OutboundCardMessage> {
        let Some(task_id) = arg.task_id else {
            // The generated insert requires a task; card rows are always
            // task-keyed in this adapter (the Patcher is the only writer).
            anyhow::bail!("lark store: outbound card requires task_id");
        };
        let Some(row) = create_channel_outbound_card_message(
            &self.pool,
            arg.chat_session_id,
            CHANNEL_TYPE_FEISHU,
            &arg.channel_chat_id,
            &arg.channel_card_message_id,
            &arg.status,
            task_id,
        )
        .await?
        else {
            return Err(ErrNoRows.into());
        };
        Ok(outbound_card_from_row(row))
    }

    pub async fn update_lark_outbound_card_status(
        &self,
        arg: UpdateOutboundCardStatusParams,
    ) -> anyhow::Result<()> {
        update_channel_outbound_card_status(&self.pool, arg.id, &arg.status).await?;
        Ok(())
    }
}

/// Transaction-capable core of [`ChannelStore::upsert_lark_installation`]:
/// registration finalize runs reclaim + upsert + installer-bind inside ONE
/// transaction, passing `&mut *tx` here.
pub async fn upsert_lark_installation_with(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    arg: UpsertInstallationParams,
) -> anyhow::Result<Installation> {
    let cfg = encode_install_config(&Installation {
        app_id: arg.app_id.clone(),
        app_secret_encrypted: arg.app_secret_encrypted.clone(),
        tenant_key: arg.tenant_key.clone(),
        bot_open_id: arg.bot_open_id.clone(),
        bot_union_id: arg.bot_union_id.clone(),
        region: arg.region.clone(),
        ..Installation::default()
    })?;
    let Some(row) = upsert_channel_installation(
        executor,
        arg.workspace_id,
        arg.agent_id,
        CHANNEL_TYPE_FEISHU,
        &cfg,
        arg.installer_user_id,
    )
    .await?
    else {
        return Err(ErrNoRows.into());
    };
    installation_from_row(row)
}

/// Transaction-capable core of
/// [`ChannelStore::reclaim_dead_installation_by_app_id`].
pub async fn reclaim_dead_installation_with(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    agent_id: Uuid,
    app_id: &str,
) -> anyhow::Result<()> {
    // No rows just means nothing was dead — a no-op, not a failure.
    reclaim_dead_channel_installation_by_app_id(
        executor,
        CHANNEL_TYPE_FEISHU,
        app_id,
        workspace_id,
        agent_id,
    )
    .await?;
    Ok(())
}

/// Transaction-capable core of [`ChannelStore::create_lark_user_binding`]:
/// the installer-bind commits alongside the installation insert inside the
/// registration transaction.
pub async fn create_lark_user_binding_with(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    arg: CreateUserBindingParams,
) -> anyhow::Result<UserBinding> {
    let cfg = encode_binding_config(&UserBinding {
        union_id: arg.union_id.clone(),
        ..UserBinding::default()
    })?;
    let Some(row) = create_channel_user_binding(
        executor,
        arg.workspace_id,
        arg.cordy_user_id,
        arg.installation_id,
        CHANNEL_TYPE_FEISHU,
        &arg.channel_user_id,
        &cfg,
    )
    .await?
    else {
        // ON CONFLICT ... WHERE cordy_user_id mismatch → no row (Go:
        // pgx.ErrNoRows from the conditional upsert).
        return Err(ErrNoRows.into());
    };
    user_binding_from_row(row)
}

/// Transaction-capable core of [`ChannelStore::consume_lark_binding_token`]
/// for redeem_and_bind, which must run consume + membership + binding insert
/// in one transaction.
pub async fn consume_lark_binding_token_with(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
) -> anyhow::Result<BindingTokenRow> {
    let Some(row) = consume_channel_binding_token(executor, token_hash).await? else {
        return Err(ErrNoRows.into());
    };
    Ok(binding_token_from_row(row))
}

/// Maps a slice of channel_installation rows to domain Installations,
/// surfacing the first config-decode error.
fn installations_from_rows(
    rows: Vec<cordy_db::models::ChannelInstallation>,
) -> anyhow::Result<Vec<Installation>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(installation_from_row(row)?);
    }
    Ok(out)
}

/// Unused-import guards for the time types re-exported through signatures.
#[allow(unused)]
fn _time_marker(_: DateTime<Utc>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_rows_sentinel_is_detectable_through_anyhow() {
        let err: anyhow::Error = ErrNoRows.into();
        assert!(is_no_rows(&err));
        assert!(!is_no_rows(&anyhow::anyhow!("boom")));
    }

    #[test]
    fn unique_violation_detection() {
        // Constructed without a live Postgres: only the negative path is
        // asserted here; the positive path is covered by integration tests.
        assert!(!is_unique_violation(&anyhow::anyhow!("nope")));
    }

    #[test]
    fn channel_type_matches_go_constant() {
        assert_eq!(CHANNEL_TYPE_FEISHU, "feishu");
    }
}
