//! Installation-generation and verification-marker contracts against Postgres.

use chrono::{Duration, Utc};
use patchbay_db::queries::channel::{
    get_channel_installation, mark_channel_installation_round_trip,
    set_channel_installation_config,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

async fn test_pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .ok()
}

#[tokio::test]
async fn round_trip_marker_is_generation_scoped_and_idempotent() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping channel round-trip contract: DATABASE_URL not set");
        return;
    };

    let id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let installer_user_id = Uuid::new_v4();
    let generation_one = Utc::now();
    let generation_two = generation_one + Duration::seconds(1);
    let channel_type = "telegram-round-trip-contract";

    sqlx::query(
        r#"INSERT INTO channel_installation
           (id, workspace_id, agent_id, channel_type, config, status,
            installer_user_id, installed_at, created_at, updated_at)
           VALUES ($1, $2, NULL, $3, $4, 'active', $5, $6, $6, $6)"#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(channel_type)
    .bind(json!({"credential_generation": "one"}))
    .bind(installer_user_id)
    .bind(generation_one)
    .execute(&pool)
    .await
    .expect("insert channel installation fixture");

    // Simulate a reconnect that reuses the row id but replaces credentials and
    // advances the installation generation before the old send completes.
    sqlx::query(
        "UPDATE channel_installation SET config = $2, installed_at = $3, updated_at = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(json!({"credential_generation": "two"}))
    .bind(generation_two)
    .execute(&pool)
    .await
    .expect("replace installation generation");

    let stale = mark_channel_installation_round_trip(
        &pool,
        id,
        channel_type,
        generation_one,
    )
    .await
    .expect("stale generation marker");
    assert!(stale.is_none(), "stale generation must not verify the row");

    let fresh = mark_channel_installation_round_trip(
        &pool,
        id,
        channel_type,
        generation_two,
    )
    .await
    .expect("current generation marker");
    assert_eq!(fresh, Some(workspace_id));

    let duplicate = mark_channel_installation_round_trip(
        &pool,
        id,
        channel_type,
        generation_two,
    )
    .await
    .expect("duplicate generation marker");
    assert!(duplicate.is_none(), "a passed marker must be idempotent");

    let current = get_channel_installation(&pool, id, channel_type)
        .await
        .expect("read installation")
        .expect("installation exists");
    assert_eq!(current.config["credential_generation"], "two");
    assert_eq!(
        current.config["verification"]["round_trip_status"],
        "passed"
    );

    // Metadata/config patches are not a new install generation. This protects
    // legitimate in-flight messages from being rejected by the CAS fence.
    let before = current.installed_at.clone();
    let mut patched_config = current.config.clone();
    patched_config["bot_union_id"] = json!("u1");
    set_channel_installation_config(&pool, id, &patched_config)
        .await
        .expect("patch installation config");
    let after = get_channel_installation(&pool, id, channel_type)
        .await
        .expect("read patched installation")
        .expect("patched installation exists")
        .installed_at;
    assert_eq!(before, after, "metadata patch must preserve installation generation");

    sqlx::query("DELETE FROM channel_installation WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("cleanup channel installation fixture");
}
