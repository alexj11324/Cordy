//! One-shot production repairs for installations predating region and
//! bot_union_id fields.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::channel_store::ChannelStore;
use crate::client::ApiClient;
use crate::installation::{installation_credentials_for, CredentialsResolver};
use crate::store::Installation;

const PAGE_SIZE: i64 = 50;
const BOT_INFO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackfillReport {
    pub region_relabelled: u64,
    pub region_errors: u64,
    pub pages: u64,
    pub attempted: u64,
    pub filled: u64,
    pub missed: u64,
    pub errored: u64,
    pub raced: u64,
    pub cancelled: bool,
}

#[async_trait]
pub trait BackfillStore: Send + Sync {
    async fn relabel_legacy_regions(&self) -> anyhow::Result<u64>;
    async fn list_missing_union_ids(
        &self,
        after: Option<(DateTime<Utc>, Uuid)>,
        limit: i64,
    ) -> anyhow::Result<Vec<Installation>>;
    async fn stamp_union_id_if_missing(
        &self,
        installation_id: Uuid,
        union_id: &str,
    ) -> anyhow::Result<bool>;
}

#[async_trait]
impl BackfillStore for ChannelStore {
    async fn relabel_legacy_regions(&self) -> anyhow::Result<u64> {
        self.backfill_lark_installation_region_to_lark().await
    }

    async fn list_missing_union_ids(
        &self,
        after: Option<(DateTime<Utc>, Uuid)>,
        limit: i64,
    ) -> anyhow::Result<Vec<Installation>> {
        self.list_lark_installations_missing_bot_union_id_after(after, limit)
            .await
    }

    async fn stamp_union_id_if_missing(
        &self,
        installation_id: Uuid,
        union_id: &str,
    ) -> anyhow::Result<bool> {
        self.stamp_lark_installation_bot_union_id_if_missing(installation_id, union_id)
            .await
    }
}

pub struct InstallationBackfill {
    store: Arc<dyn BackfillStore>,
    api: Arc<dyn ApiClient>,
    credentials: Arc<dyn CredentialsResolver>,
    http_override: String,
    callback_override: String,
}

impl InstallationBackfill {
    pub fn new(
        store: Arc<dyn BackfillStore>,
        api: Arc<dyn ApiClient>,
        credentials: Arc<dyn CredentialsResolver>,
        http_override: impl Into<String>,
        callback_override: impl Into<String>,
    ) -> anyhow::Result<Self> {
        if !api.is_configured() {
            anyhow::bail!("lark backfill requires a configured API client");
        }
        Ok(Self {
            store,
            api,
            credentials,
            http_override: http_override.into(),
            callback_override: callback_override.into(),
        })
    }

    /// Runs one boot-time pass. Per-row failures are counted and left missing,
    /// which makes the next process start the bounded retry. Successfully
    /// stamped rows disappear from the paged selector and are never fetched
    /// again.
    pub async fn run_once(&self, cancel: CancellationToken) -> anyhow::Result<BackfillReport> {
        let mut report = BackfillReport::default();
        if is_lark_international_host(&self.http_override)
            || is_lark_international_host(&self.callback_override)
        {
            let relabelled = tokio::select! {
                _ = cancel.cancelled() => {
                    report.cancelled = true;
                    return Ok(report);
                }
                result = self.store.relabel_legacy_regions() => result,
            };
            match relabelled {
                Ok(rows) => report.region_relabelled = rows,
                Err(error) => {
                    report.region_errors += 1;
                    tracing::warn!(%error, "lark backfill: legacy region relabel failed");
                }
            }
        }

        let mut cursor = None;
        loop {
            let rows = tokio::select! {
                _ = cancel.cancelled() => {
                    report.cancelled = true;
                    return Ok(report);
                }
                result = self.store.list_missing_union_ids(cursor, PAGE_SIZE) => result?,
            };
            if rows.is_empty() {
                return Ok(report);
            }
            report.pages += 1;
            let last = rows
                .last()
                .map(|installation| (installation.created_at.to_owned(), installation.id));
            for installation in rows {
                if cancel.is_cancelled() {
                    report.cancelled = true;
                    return Ok(report);
                }
                report.attempted += 1;
                let credentials =
                    match installation_credentials_for(self.credentials.as_ref(), &installation) {
                        Ok(credentials) => credentials,
                        Err(error) => {
                            report.errored += 1;
                            tracing::warn!(
                                %error,
                                installation_id = %installation.id,
                                workspace_id = %installation.workspace_id,
                                app_id = %installation.app_id,
                                "lark backfill: decrypt app secret failed"
                            );
                            continue;
                        }
                    };
                let info = tokio::select! {
                    _ = cancel.cancelled() => {
                        report.cancelled = true;
                        return Ok(report);
                    }
                    result = tokio::time::timeout(
                        BOT_INFO_TIMEOUT,
                        self.api.get_bot_info(credentials),
                    ) => result,
                };
                let info = match info {
                    Ok(Ok(info)) => info,
                    Ok(Err(error)) => {
                        report.errored += 1;
                        tracing::warn!(
                            %error,
                            installation_id = %installation.id,
                            workspace_id = %installation.workspace_id,
                            app_id = %installation.app_id,
                            "lark backfill: get bot info failed"
                        );
                        continue;
                    }
                    Err(_) => {
                        report.errored += 1;
                        tracing::warn!(
                            installation_id = %installation.id,
                            workspace_id = %installation.workspace_id,
                            app_id = %installation.app_id,
                            "lark backfill: get bot info timed out"
                        );
                        continue;
                    }
                };
                if info.union_id.is_empty() {
                    report.missed += 1;
                    tracing::warn!(
                        installation_id = %installation.id,
                        workspace_id = %installation.workspace_id,
                        app_id = %installation.app_id,
                        bot_open_id = %info.open_id,
                        "lark backfill: union id absent in response"
                    );
                    continue;
                }
                let stamped = tokio::select! {
                    _ = cancel.cancelled() => {
                        report.cancelled = true;
                        return Ok(report);
                    }
                    result = self.store.stamp_union_id_if_missing(
                        installation.id,
                        &info.union_id,
                    ) => result,
                };
                match stamped {
                    Ok(true) => {
                        report.filled += 1;
                        tracing::info!(
                            installation_id = %installation.id,
                            workspace_id = %installation.workspace_id,
                            app_id = %installation.app_id,
                            bot_open_id = %info.open_id,
                            "lark backfill: stamped union id"
                        );
                    }
                    Ok(false) => report.raced += 1,
                    Err(error) => {
                        report.errored += 1;
                        tracing::warn!(
                            %error,
                            installation_id = %installation.id,
                            workspace_id = %installation.workspace_id,
                            "lark backfill: persist union id failed"
                        );
                    }
                }
            }
            cursor = last;
        }
    }
}

pub fn is_lark_international_host(raw: &str) -> bool {
    let raw = raw.trim();
    if url::Url::parse(raw).is_err() {
        return false;
    }
    let Some((_, rest)) = raw.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    host.eq_ignore_ascii_case("open.larksuite.com")
}
