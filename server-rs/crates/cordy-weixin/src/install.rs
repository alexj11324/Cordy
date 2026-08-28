use cordy_db::models::ChannelInstallation;
use cordy_db::queries::channel::{
    create_channel_user_binding, delete_channel_installation_for_replacement,
    get_channel_installation_owner_by_app_id, list_channel_installations_by_workspace,
    lock_channel_installation_agent_slot, lock_channel_installation_app_id_slot,
    reclaim_dead_channel_installation_by_app_id, upsert_channel_installation,
};
use sqlx::PgPool;
use uuid::Uuid;

const PG_UNIQUE_VIOLATION: &str = "23505";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("weixin: this account is already connected in this workspace")]
    SameWorkspace,
    #[error("weixin: this account is connected to an archived agent")]
    ArchivedAgent,
    #[error("weixin: this account is already connected to another workspace")]
    AnotherWorkspace,
}

#[derive(Debug, Clone)]
pub struct InstallParams {
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub installer_id: Uuid,
    pub bot_id: String,
    pub ilink_user_id: String,
    pub config: serde_json::Value,
}

pub async fn finalize(
    pool: &PgPool,
    params: &InstallParams,
) -> anyhow::Result<ChannelInstallation> {
    if params.bot_id.is_empty()
        || params
            .config
            .get("app_id")
            .and_then(serde_json::Value::as_str)
            != Some(params.bot_id.as_str())
    {
        anyhow::bail!("weixin: bot id does not match installation routing key");
    }
    let mut tx = pool.begin().await?;
    lock_channel_installation_agent_slot(
        &mut *tx,
        crate::TYPE_WEIXIN,
        params.workspace_id,
        params.agent_id,
    )
    .await?;
    lock_channel_installation_app_id_slot(&mut *tx, crate::TYPE_WEIXIN, &params.bot_id).await?;
    let authorized = sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS (
            SELECT 1 FROM member m
            JOIN agent a ON a.id = $3 AND a.workspace_id = m.workspace_id
            WHERE m.workspace_id = $1 AND m.user_id = $2
              AND (m.role IN ('owner', 'admin') OR a.owner_id = $2)
              AND a.archived_at IS NULL
        )"#,
    )
    .bind(params.workspace_id)
    .bind(params.installer_id)
    .bind(params.agent_id)
    .fetch_one(&mut *tx)
    .await?;
    if !authorized {
        anyhow::bail!("weixin: authorization changed during install");
    }
    reclaim_dead_channel_installation_by_app_id(
        &mut *tx,
        crate::TYPE_WEIXIN,
        &params.bot_id,
        params.workspace_id,
        params.agent_id,
    )
    .await?;

    // Reusing the same agent slot for a different iLink bot must not carry
    // user bindings, sessions, dedup rows, or the long-poll cursor forward.
    let current =
        list_channel_installations_by_workspace(&mut *tx, params.workspace_id, crate::TYPE_WEIXIN)
            .await?
            .into_iter()
            .find(|row| row.agent_id == params.agent_id);
    if let Some(current) = current.filter(|row| {
        row.config.get("app_id").and_then(serde_json::Value::as_str) != Some(params.bot_id.as_str())
    }) {
        delete_channel_installation_for_replacement(&mut *tx, current.id).await?;
    }
    let row = match upsert_channel_installation(
        &mut *tx,
        params.workspace_id,
        params.agent_id,
        crate::TYPE_WEIXIN,
        &params.config,
        params.installer_id,
    )
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => anyhow::bail!("weixin: installation upsert returned no row"),
        Err(error) if is_unique_violation(&error) => {
            let _ = tx.rollback().await;
            let owner =
                get_channel_installation_owner_by_app_id(pool, crate::TYPE_WEIXIN, &params.bot_id)
                    .await?;
            let kind = match owner {
                Some(owner) if owner.workspace_id != Some(params.workspace_id) => {
                    InstallError::AnotherWorkspace
                }
                Some(owner) if owner.agent_archived_at.is_some() => InstallError::ArchivedAgent,
                _ => InstallError::SameWorkspace,
            };
            return Err(kind.into());
        }
        Err(error) => return Err(error),
    };
    let bound = create_channel_user_binding(
        &mut *tx,
        params.workspace_id,
        params.installer_id,
        row.id,
        crate::TYPE_WEIXIN,
        &params.ilink_user_id,
        &serde_json::json!({}),
    )
    .await?;
    if bound.is_none() {
        anyhow::bail!("weixin: scanner account is bound to another Patchbay user");
    }
    tx.commit().await?;
    Ok(row)
}

fn is_unique_violation(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<sqlx::Error>().is_some_and(|error| {
            error
                .as_database_error()
                .is_some_and(|database| database.code().as_deref() == Some(PG_UNIQUE_VIOLATION))
        })
    })
}
