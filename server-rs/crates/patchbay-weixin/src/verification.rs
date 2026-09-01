use std::sync::Weak;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Record the first real Weixin inbound -> outbound response for the exact
/// installation generation that sent it, then notify connected clients.
pub(crate) async fn record_round_trip(
    pool: &PgPool,
    bus: &Weak<patchbay_events::Bus>,
    installation_id: Uuid,
    installed_at: DateTime<Utc>,
) {
    match patchbay_db::queries::channel::mark_channel_installation_round_trip(
        pool,
        installation_id,
        crate::TYPE_WEIXIN,
        installed_at,
    )
    .await
    {
        Ok(Some(workspace_id)) => {
            if let Some(bus) = bus.upgrade() {
                bus.publish(&patchbay_events::Event {
                    event_type: patchbay_protocol::EVENT_WEIXIN_INSTALLATION_VERIFIED.into(),
                    workspace_id: workspace_id.to_string(),
                    actor_type: "system".into(),
                    payload: serde_json::json!({
                        "id": installation_id,
                        "verification": {"round_trip_status": "passed"},
                    }),
                    ..Default::default()
                });
            } else {
                tracing::debug!(
                    installation_id = %installation_id,
                    "weixin round-trip marker persisted but realtime bus is unavailable"
                );
            }
        }
        Ok(None) => tracing::debug!(
            installation_id = %installation_id,
            "weixin round-trip succeeded but installation verification marker was not updated"
        ),
        Err(error) => tracing::warn!(
            installation_id = %installation_id,
            %error,
            "weixin round-trip succeeded but installation verification marker failed"
        ),
    }
}
