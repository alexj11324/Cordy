//! Integration tests for the transaction-scoped Work Product association lock.
//! Skipped when DATABASE_URL is unset so environments without Postgres remain
//! runnable.

use patchbay_db::queries::work_product;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::oneshot;
use tokio::time::{timeout, Duration};
use uuid::Uuid;

async fn test_pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .ok()
}

#[tokio::test]
async fn task_work_product_scope_serializes_attach_and_discovery_critical_sections() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let workspace_id = Uuid::now_v7();
    let task_id = Uuid::now_v7();
    let (first_locked_tx, first_locked_rx) = oneshot::channel();
    let (release_first_tx, release_first_rx) = oneshot::channel();
    let (second_started_tx, second_started_rx) = oneshot::channel();
    let (second_acquired_tx, mut second_acquired_rx) = oneshot::channel();

    let first_pool = pool.clone();
    let first = tokio::spawn(async move {
        let mut transaction = first_pool.begin().await.expect("begin first transaction");
        work_product::lock_task_work_product_scope(&mut *transaction, workspace_id, task_id)
            .await
            .expect("first task lock");
        first_locked_tx.send(()).expect("signal first lock");
        release_first_rx.await.expect("release first transaction");
        transaction.commit().await.expect("commit first transaction");
    });

    first_locked_rx.await.expect("first transaction acquired lock");

    let second_pool = pool.clone();
    let second = tokio::spawn(async move {
        let mut transaction = second_pool
            .begin()
            .await
            .expect("begin second transaction");
        second_started_tx.send(()).expect("signal second start");
        work_product::lock_task_work_product_scope(&mut *transaction, workspace_id, task_id)
            .await
            .expect("second task lock");
        second_acquired_tx.send(()).expect("signal second lock");
        transaction.commit().await.expect("commit second transaction");
    });

    second_started_rx.await.expect("second transaction started");
    let second_was_blocked = timeout(Duration::from_millis(100), &mut second_acquired_rx)
        .await
        .is_err();
    release_first_tx.send(()).expect("release first lock");

    first.await.expect("first transaction task");
    assert!(
        second_was_blocked,
        "a concurrent attach/discovery critical section must wait for the task lock"
    );
    timeout(Duration::from_secs(5), &mut second_acquired_rx)
        .await
        .expect("second transaction acquired lock after first commit")
        .expect("second lock signal");
    second.await.expect("second transaction task");
}
