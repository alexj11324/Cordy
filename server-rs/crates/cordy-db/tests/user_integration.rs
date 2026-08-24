//! Integration tests against a live Postgres. Skipped when DATABASE_URL is
//! unset so `cargo test` stays green in environments without a database.

use cordy_db::user;
use sqlx::postgres::PgPoolOptions;

async fn test_pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .ok()
}

#[tokio::test]
async fn user_crud_roundtrip() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let email = format!(
        "rust-migration-test-{}@example.invalid",
        uuid::Uuid::now_v7()
    );
    let created = crate::user::create_user(&pool, "Rust Migration", &email, None)
        .await
        .expect("create_user");

    assert_eq!(created.email, email);
    assert_eq!(created.name, "Rust Migration");
    assert!(created.onboarded_at.is_none());

    let fetched = crate::user::get_user_by_email(&pool, &email)
        .await
        .expect("get_user_by_email")
        .expect("user should exist");
    assert_eq!(fetched.id, created.id);

    let onboarded = crate::user::mark_user_onboarded(&pool, created.id)
        .await
        .expect("mark_user_onboarded");
    assert!(onboarded.onboarded_at.is_some());

    let batch = crate::user::get_users_by_ids(&pool, &[created.id])
        .await
        .expect("get_users_by_ids");
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].1, "Rust Migration");

    // Cleanup — the test DB must not accumulate fixture rows.
    sqlx::query(r#"DELETE FROM "user" WHERE id = $1"#)
        .bind(created.id)
        .execute(&pool)
        .await
        .expect("cleanup");
}

/// Validates the GENERATOR-produced query module end-to-end: SQL text,
/// bind order, positional extraction, and nullability mapping.
#[tokio::test]
async fn generated_user_queries_roundtrip() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let email = format!("gen-test-{}@example.invalid", uuid::Uuid::now_v7());

    let created = cordy_db::queries::user::create_user(&pool, "Gen Test", &email, None)
        .await
        .expect("generated create_user")
        .expect("insert returning row");
    assert_eq!(created.email, email);
    assert_eq!(created.name, "Gen Test");

    let fetched = cordy_db::queries::user::get_user_by_email(&pool, &email)
        .await
        .expect("generated get_user_by_email")
        .expect("user exists");
    assert_eq!(fetched.id, created.id);

    let onboarded = cordy_db::queries::user::mark_user_onboarded(&pool, created.id)
        .await
        .expect("generated mark_user_onboarded")
        .expect("update returning row");
    assert!(onboarded.onboarded_at.is_some());

    let questionnaire = serde_json::json!({
        "role": "founder",
        "use_case": ["coding"],
        "version": 2
    });
    let patched =
        cordy_db::queries::user::patch_user_onboarding(&pool, Some(&questionnaire), created.id)
            .await
            .expect("patch_user_onboarding")
            .expect("update returning row");
    assert_eq!(patched.onboarding_questionnaire, questionnaire);

    let preserved = cordy_db::queries::user::patch_user_onboarding(&pool, None, created.id)
        .await
        .expect("preserve onboarding questionnaire")
        .expect("update returning row");
    assert_eq!(preserved.onboarding_questionnaire, questionnaire);

    let waitlisted = cordy_db::queries::user::join_cloud_waitlist(
        &pool,
        created.id,
        Some("waitlist@example.com"),
        Some("evaluating for our team"),
    )
    .await
    .expect("join cloud waitlist")
    .expect("update returning row");
    assert_eq!(
        waitlisted.cloud_waitlist_email.as_deref(),
        Some("waitlist@example.com")
    );
    assert_eq!(
        waitlisted.cloud_waitlist_reason.as_deref(),
        Some("evaluating for our team")
    );
    assert_eq!(waitlisted.onboarded_at, onboarded.onboarded_at);

    let waitlisted_without_reason = cordy_db::queries::user::join_cloud_waitlist(
        &pool,
        created.id,
        Some("second@example.com"),
        None,
    )
    .await
    .expect("overwrite cloud waitlist")
    .expect("update returning row");
    assert_eq!(
        waitlisted_without_reason.cloud_waitlist_email.as_deref(),
        Some("second@example.com")
    );
    assert!(waitlisted_without_reason.cloud_waitlist_reason.is_none());

    let onboarded_again = cordy_db::queries::user::mark_user_onboarded(&pool, created.id)
        .await
        .expect("mark_user_onboarded idempotently")
        .expect("update returning row");
    assert_eq!(onboarded_again.onboarded_at, onboarded.onboarded_at);

    let batch = cordy_db::queries::user::get_users_by_i_ds(&pool, vec![created.id])
        .await
        .expect("generated get_users_by_i_ds");
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].name, "Gen Test");

    sqlx::query(r#"DELETE FROM "user" WHERE id = $1"#)
        .bind(created.id)
        .execute(&pool)
        .await
        .expect("cleanup");
}
