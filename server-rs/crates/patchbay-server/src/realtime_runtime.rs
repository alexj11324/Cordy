use std::sync::Arc;
use std::time::Duration;

use patchbay_config::RedisConfig;
use patchbay_handler::realtime_forwarder::RealtimeForwarder;
use patchbay_realtime::hub::Hub;
use patchbay_realtime::{
    ManagedRelay, MirroredRelay, RedisRelay, ShardedStreamRelay, ShardedStreamRelayConfig,
    SwitchableRelayBroadcaster,
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const RELAY_RETRY_MIN: Duration = Duration::from_secs(1);
const RELAY_RETRY_MAX: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayMode {
    Sharded,
    Dual,
    Legacy,
}

impl RelayMode {
    fn from_config(value: Option<&str>) -> Self {
        match value
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "" | "sharded" => Self::Sharded,
            "dual" => Self::Dual,
            "legacy" => Self::Legacy,
            value => {
                tracing::warn!(
                    value,
                    default = "sharded",
                    "invalid REALTIME_RELAY_MODE; using default"
                );
                Self::Sharded
            }
        }
    }
}

#[derive(Clone)]
struct RelaySettings {
    redis_url: String,
    mode: RelayMode,
}

impl RelaySettings {
    fn from_config(config: &RedisConfig) -> Option<Self> {
        let redis_url = config
            .realtime_relay_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                config
                    .url
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
            })?
            .to_string();
        Some(Self {
            redis_url,
            mode: RelayMode::from_config(config.realtime_relay_mode.as_deref()),
        })
    }
}

/// Owns the fanout worker and every Redis relay task used by the HTTP server.
/// It intentionally outlives the router so shutdown can drain HTTP producers,
/// then queued events, then relay consumers in that order.
pub struct RealtimeRuntime {
    hub: Arc<Hub>,
    broadcaster: Arc<SwitchableRelayBroadcaster>,
    relay_settings: Option<RelaySettings>,
    shutdown: CancellationToken,
    retry_task: Option<JoinHandle<()>>,
    forwarder: Option<RealtimeForwarder>,
    daemon_notifier: Option<Arc<patchbay_daemon::notifier::RelayNotifier>>,
}

impl RealtimeRuntime {
    pub async fn from_config(hub: Arc<Hub>, config: &RedisConfig) -> Self {
        let shutdown = CancellationToken::new();
        let broadcaster = Arc::new(SwitchableRelayBroadcaster::new(hub.clone()));
        let relay_settings = RelaySettings::from_config(config);
        let Some(settings) = relay_settings.clone() else {
            tracing::info!("realtime: Redis URL unset; using in-memory hub");
            return Self {
                hub,
                broadcaster,
                relay_settings: None,
                shutdown,
                retry_task: None,
                forwarder: None,
                daemon_notifier: None,
            };
        };

        let relay = match tokio::time::timeout(
            RELAY_CONNECT_TIMEOUT,
            build_relay(&settings.redis_url, settings.mode, hub.clone()),
        )
        .await
        {
            Ok(Ok(relay)) => Some(relay),
            Ok(Err(error)) => {
                tracing::error!(%error, "realtime Redis relay unavailable; retrying in background");
                None
            }
            Err(_) => {
                tracing::error!(
                    "realtime Redis relay connection timed out; retrying in background"
                );
                None
            }
        };
        if let Some(relay) = relay {
            tracing::info!(node_id = %relay.node_id(), mode = ?settings.mode, "realtime Redis relay enabled");
            broadcaster.set_relay(Some(relay));
        }
        Self {
            hub,
            broadcaster,
            relay_settings: Some(settings),
            shutdown,
            retry_task: None,
            forwarder: None,
            daemon_notifier: None,
        }
    }

    pub fn attach(
        &mut self,
        bus: &patchbay_events::Bus,
        daemon_hub: Option<Arc<patchbay_daemon::hub::DaemonHub>>,
        daemon_notifier: Arc<patchbay_daemon::notifier::RelayNotifier>,
    ) {
        let relay_mode = self.relay_settings.as_ref().map(|settings| settings.mode);
        if relay_mode != Some(RelayMode::Legacy) && relay_mode.is_some() {
            self.daemon_notifier = Some(daemon_notifier.clone());
        }
        if let Some(relay) = self.broadcaster.relay() {
            activate_relay(
                relay,
                relay_mode.unwrap_or(RelayMode::Sharded),
                daemon_hub.clone(),
                daemon_notifier.clone(),
                self.shutdown.clone(),
            );
        } else if let Some(settings) = self.relay_settings.clone() {
            let hub = self.hub.clone();
            let broadcaster = self.broadcaster.clone();
            let shutdown = self.shutdown.clone();
            self.retry_task = Some(tokio::spawn(async move {
                let mut delay = RELAY_RETRY_MIN;
                loop {
                    tokio::select! {
                        () = shutdown.cancelled() => return,
                        () = tokio::time::sleep(delay) => {}
                    }
                    let attempt = tokio::time::timeout(
                        RELAY_CONNECT_TIMEOUT,
                        build_relay(&settings.redis_url, settings.mode, hub.clone()),
                    )
                    .await;
                    match attempt {
                        Ok(Ok(relay)) => {
                            activate_relay(
                                relay.clone(),
                                settings.mode,
                                daemon_hub.clone(),
                                daemon_notifier.clone(),
                                shutdown.clone(),
                            );
                            tracing::info!(
                                node_id = %relay.node_id(),
                                mode = ?settings.mode,
                                "realtime Redis relay recovered after startup outage"
                            );
                            broadcaster.set_relay(Some(relay));
                            return;
                        }
                        Ok(Err(error)) => {
                            tracing::warn!(%error, ?delay, "realtime Redis relay retry failed");
                        }
                        Err(_) => {
                            tracing::warn!(?delay, "realtime Redis relay retry timed out");
                        }
                    }
                    delay = std::cmp::min(delay.saturating_mul(2), RELAY_RETRY_MAX);
                }
            }));
        }
        self.forwarder = Some(RealtimeForwarder::start(bus, self.broadcaster.clone()));
    }

    pub async fn shutdown(mut self) {
        if let Some(forwarder) = self.forwarder.take() {
            forwarder.shutdown().await;
        }
        if let Some(notifier) = self.daemon_notifier.take() {
            notifier.set_relay(None);
        }
        self.shutdown.cancel();
        if let Some(retry_task) = self.retry_task.take() {
            let _ = retry_task.await;
        }
        if let Some(relay) = self.broadcaster.relay() {
            self.broadcaster.set_relay(None);
            relay.stop();
            relay.wait().await;
        }
    }
}

fn activate_relay(
    relay: Arc<dyn ManagedRelay>,
    mode: RelayMode,
    daemon_hub: Option<Arc<patchbay_daemon::hub::DaemonHub>>,
    daemon_notifier: Arc<patchbay_daemon::notifier::RelayNotifier>,
    shutdown: CancellationToken,
) {
    if mode != RelayMode::Legacy {
        if let Some(daemon_hub) = daemon_hub {
            relay.set_daemon_runtime_deliverer(daemon_hub);
        }
        daemon_notifier.set_relay(Some(relay.clone()));
    }
    relay.start(shutdown);
}

async fn build_relay(
    redis_url: &str,
    mode: RelayMode,
    hub: Arc<Hub>,
) -> anyhow::Result<Arc<dyn ManagedRelay>> {
    let write_client = redis::Client::open(redis_url)?;
    let config = relay_config_from_env();
    match mode {
        RelayMode::Sharded => {
            let relay = ShardedStreamRelay::new(
                hub,
                write_client,
                Some(redis::Client::open(redis_url)?),
                config,
            )
            .await?;
            Ok(Arc::new(relay))
        }
        RelayMode::Legacy => {
            let relay = RedisRelay::new_with_clients(
                hub.clone(),
                hub,
                write_client,
                Some(redis::Client::open(redis_url)?),
                config.retention_config(),
            )
            .await?;
            Ok(Arc::new(relay))
        }
        RelayMode::Dual => {
            let sharded: Arc<dyn ManagedRelay> = Arc::new(
                ShardedStreamRelay::new(
                    hub.clone(),
                    write_client.clone(),
                    Some(redis::Client::open(redis_url)?),
                    config.clone(),
                )
                .await?,
            );
            let legacy: Arc<dyn ManagedRelay> = Arc::new(
                RedisRelay::new_with_clients(
                    hub.clone(),
                    hub,
                    write_client,
                    Some(redis::Client::open(redis_url)?),
                    config.retention_config(),
                )
                .await?,
            );
            Ok(Arc::new(MirroredRelay::new(sharded, legacy)))
        }
    }
}

fn relay_config_from_env() -> ShardedStreamRelayConfig {
    let mut config = ShardedStreamRelayConfig::default();
    config.shards = positive_env_usize("REALTIME_RELAY_SHARDS", config.shards);
    config.stream_max_len = positive_env_i64("REALTIME_RELAY_STREAM_MAXLEN", config.stream_max_len);
    config.read_count = positive_env_i64("REALTIME_RELAY_XREAD_COUNT", config.read_count);
    config.read_block = super::duration_env("REALTIME_RELAY_XREAD_BLOCK", config.read_block, false);
    config.replay_grace =
        super::duration_env("REALTIME_RELAY_REPLAY_GRACE", config.replay_grace, false);
    config.trim_horizon = super::duration_env(
        "REALTIME_RELAY_TRIM_HORIZON",
        config.replay_grace * 2,
        false,
    );
    config.stream_ttl = super::duration_env(
        "REALTIME_RELAY_STREAM_TTL",
        config.trim_horizon + config.replay_grace,
        false,
    );
    config.ttl_refresh_interval = super::duration_env(
        "REALTIME_RELAY_TTL_REFRESH_INTERVAL",
        config.ttl_refresh_interval,
        false,
    );
    config.maintenance_interval = super::duration_env(
        "REALTIME_RELAY_MAINTENANCE_INTERVAL",
        config.maintenance_interval,
        false,
    );
    config.stream_ttl_enabled = super::parse_go_bool(
        std::env::var("REALTIME_RELAY_STREAM_TTL_ENABLED")
            .ok()
            .as_deref(),
        false,
    );
    if let Err(error) = config.validate() {
        tracing::warn!(%error, "invalid realtime relay retention config; normalizing");
    }
    config.normalized()
}

fn positive_env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn positive_env_i64(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_mode_falls_back_to_sharded() {
        assert_eq!(RelayMode::from_config(Some("unknown")), RelayMode::Sharded);
    }

    #[test]
    fn loaded_relay_config_prefers_dedicated_url() {
        let settings = RelaySettings::from_config(&RedisConfig {
            url: Some("redis://general".into()),
            realtime_relay_url: Some("redis://relay".into()),
            realtime_relay_mode: Some("dual".into()),
            ..RedisConfig::default()
        })
        .expect("configured relay");
        assert_eq!(settings.redis_url, "redis://relay");
        assert_eq!(settings.mode, RelayMode::Dual);
    }

    #[test]
    fn loaded_relay_config_falls_back_to_general_redis() {
        let settings = RelaySettings::from_config(&RedisConfig {
            url: Some("redis://general".into()),
            ..RedisConfig::default()
        })
        .expect("configured relay");
        assert_eq!(settings.redis_url, "redis://general");
        assert_eq!(settings.mode, RelayMode::Sharded);
    }
}
