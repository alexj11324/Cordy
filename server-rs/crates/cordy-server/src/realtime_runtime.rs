use std::sync::Arc;
use std::time::Duration;

use cordy_handler::realtime_forwarder::RealtimeForwarder;
use cordy_realtime::hub::Hub;
use cordy_realtime::{
    Broadcaster, DualWriteBroadcaster, ManagedRelay, MirroredRelay, RedisRelay, ShardedStreamRelay,
    ShardedStreamRelayConfig,
};
use tokio_util::sync::CancellationToken;

const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayMode {
    Sharded,
    Dual,
    Legacy,
}

impl RelayMode {
    fn from_env() -> Self {
        match std::env::var("REALTIME_RELAY_MODE")
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

/// Owns the fanout worker and every Redis relay task used by the HTTP server.
/// It intentionally outlives the router so shutdown can drain HTTP producers,
/// then queued events, then relay consumers in that order.
pub struct RealtimeRuntime {
    broadcaster: Arc<dyn Broadcaster>,
    relay: Option<Arc<dyn ManagedRelay>>,
    relay_mode: Option<RelayMode>,
    shutdown: CancellationToken,
    forwarder: Option<RealtimeForwarder>,
}

impl RealtimeRuntime {
    pub async fn from_env(hub: Arc<Hub>) -> Self {
        let shutdown = CancellationToken::new();
        let local: Arc<dyn Broadcaster> = hub.clone();
        let redis_url = std::env::var("REALTIME_RELAY_REDIS_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                std::env::var("REDIS_URL")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            });
        let Some(redis_url) = redis_url else {
            tracing::info!("realtime: Redis URL unset; using in-memory hub");
            return Self {
                broadcaster: local,
                relay: None,
                relay_mode: None,
                shutdown,
                forwarder: None,
            };
        };

        let mode = RelayMode::from_env();
        let relay = match tokio::time::timeout(
            RELAY_CONNECT_TIMEOUT,
            build_relay(&redis_url, mode, hub.clone()),
        )
        .await
        {
            Ok(Ok(relay)) => relay,
            Ok(Err(error)) => {
                tracing::error!(%error, "realtime Redis relay unavailable; using in-memory hub");
                return Self {
                    broadcaster: local,
                    relay: None,
                    relay_mode: None,
                    shutdown,
                    forwarder: None,
                };
            }
            Err(_) => {
                tracing::error!("realtime Redis relay connection timed out; using in-memory hub");
                return Self {
                    broadcaster: local,
                    relay: None,
                    relay_mode: None,
                    shutdown,
                    forwarder: None,
                };
            }
        };

        let broadcaster: Arc<dyn Broadcaster> =
            Arc::new(DualWriteBroadcaster::new(hub, relay.clone()));
        tracing::info!(node_id = %relay.node_id(), ?mode, "realtime Redis relay enabled");
        Self {
            broadcaster,
            relay: Some(relay),
            relay_mode: Some(mode),
            shutdown,
            forwarder: None,
        }
    }

    pub fn attach(
        &mut self,
        bus: &cordy_events::Bus,
        daemon_hub: Option<Arc<cordy_daemon::hub::DaemonHub>>,
        daemon_notifier: Arc<cordy_daemon::notifier::RelayNotifier>,
    ) {
        if let Some(relay) = &self.relay {
            if self.relay_mode != Some(RelayMode::Legacy) {
                if let Some(daemon_hub) = daemon_hub {
                    relay.set_daemon_runtime_deliverer(daemon_hub);
                }
                daemon_notifier.set_relay(Some(relay.clone()));
            }
            relay.clone().start(self.shutdown.clone());
        }
        self.forwarder = Some(RealtimeForwarder::start(bus, self.broadcaster.clone()));
    }

    pub async fn shutdown(mut self) {
        if let Some(forwarder) = self.forwarder.take() {
            forwarder.shutdown().await;
        }
        self.shutdown.cancel();
        if let Some(relay) = self.relay {
            relay.stop();
            relay.wait().await;
        }
    }
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
        std::env::set_var("REALTIME_RELAY_MODE", "unknown");
        assert_eq!(RelayMode::from_env(), RelayMode::Sharded);
        std::env::remove_var("REALTIME_RELAY_MODE");
    }
}
