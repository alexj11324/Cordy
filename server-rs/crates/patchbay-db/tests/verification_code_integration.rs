//! Integration tests against a live Postgres. Skipped when DATABASE_URL is
//! unset so `cargo test` stays green in environments without a database.

use chrono::{Duration, Utc};
use patchbay_db::queries::verification_code;
use sqlx::postgres::PgPoolOptions;

async fn test_pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .ok()
}

#[tokio::test]
async fn verification_code_can_only_be_consumed_once_concurrently() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let email = format!("otp-cas-{}@example.invalid", uuid::Uuid::now_v7());
    let code = verification_code::create_verification_code(
        &pool,
        &email,
        "123456",
        Some(Utc::now() + Duration::minutes(10)),
    )
    .await
    .expect("create verification code")
    .expect("insert returning row");

    let mut consumers = Vec::new();
    for _ in 0..8 {
        let pool = pool.clone();
        consumers.push(tokio::spawn(async move {
            verification_code::mark_verification_code_used(&pool, code.id)
                .await
                .expect("consume verification code")
        }));
    }

    let mut consumed = 0;
    for consumer in consumers {
        if consumer.await.expect("consumer task") {
            consumed += 1;
        }
    }
    assert_eq!(consumed, 1, "exactly one concurrent consumer must win");

    sqlx::query("DELETE FROM verification_code WHERE id = $1")
        .bind(code.id)
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn verification_code_consume_rejects_expired_or_exhausted_codes() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    for (suffix, expires_at, attempts) in [
        ("expired", Utc::now() - Duration::seconds(1), 0),
        ("exhausted", Utc::now() + Duration::minutes(10), 5),
    ] {
        let email = format!("otp-cas-{suffix}-{}@example.invalid", uuid::Uuid::now_v7());
        let code =
            verification_code::create_verification_code(&pool, &email, "123456", Some(expires_at))
                .await
                .expect("create verification code")
                .expect("insert returning row");
        if attempts > 0 {
            sqlx::query("UPDATE verification_code SET attempts = $2 WHERE id = $1")
                .bind(code.id)
                .bind(attempts)
                .execute(&pool)
                .await
                .expect("set attempts");
        }

        assert!(
            !verification_code::mark_verification_code_used(&pool, code.id)
                .await
                .expect("consume verification code"),
            "{suffix} code must not be consumed"
        );

        sqlx::query("DELETE FROM verification_code WHERE id = $1")
            .bind(code.id)
            .execute(&pool)
            .await
            .expect("cleanup");
    }
}
