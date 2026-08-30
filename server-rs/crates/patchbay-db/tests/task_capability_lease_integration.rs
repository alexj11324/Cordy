//! Capability-lease persistence contracts against migrated PostgreSQL.

use chrono::{Duration, Utc};
use patchbay_db::queries::task_token::{
    create_task_token, get_task_token_by_hash, revoke_task_token, task_token_exists_for_claim,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

struct Rows {
    pool: PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    agent_id: Uuid,
    issue_id: Uuid,
    runtime_id: Uuid,
}

impl Rows {
    async fn optional() -> anyhow::Result<Option<Self>> {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            eprintln!("skipping task capability lease contract: DATABASE_URL not set");
            return Ok(None);
        };
        let pool = PgPool::connect(&url).await?;
        let workspace_id = Uuid::now_v7();
        let user_id = Uuid::now_v7();
        let agent_id = Uuid::now_v7();
        let issue_id = Uuid::now_v7();
        let runtime_id = Uuid::now_v7();
        sqlx::query("INSERT INTO workspace (id, name, slug) VALUES ($1, $2, $3)")
            .bind(workspace_id)
            .bind("Capability lease contract")
            .bind(format!("capability-lease-{workspace_id}"))
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO \"user\" (id, name, email) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind("Capability lease user")
            .bind(format!("capability-lease-{user_id}@example.test"))
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO member (workspace_id, user_id, role) VALUES ($1, $2, 'owner')")
            .bind(workspace_id)
            .bind(user_id)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO agent_runtime (id, workspace_id, name, runtime_mode, provider, status, owner_id) \
             VALUES ($1, $2, 'Capability lease runtime', 'local', 'test', 'online', $3)",
        )
        .bind(runtime_id)
        .bind(workspace_id)
        .bind(user_id)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO agent (id, workspace_id, name, runtime_mode, status, max_concurrent_tasks, owner_id, runtime_id) \
             VALUES ($1, $2, $3, 'local', 'idle', 1, $4, $5)",
        )
        .bind(agent_id)
        .bind(workspace_id)
        .bind("Capability lease agent")
        .bind(user_id)
        .bind(runtime_id)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO issue (id, workspace_id, title, status, priority, creator_type, creator_id, assignee_type, assignee_id, number, position) \
             VALUES ($1, $2, $3, 'in_progress', 'medium', 'member', $4, 'agent', $5, 1, 0)",
        )
        .bind(issue_id)
        .bind(workspace_id)
        .bind("Capability lease issue")
        .bind(user_id)
        .bind(agent_id)
        .execute(&pool)
        .await?;
        Ok(Some(Self {
            pool,
            workspace_id,
            user_id,
            agent_id,
            issue_id,
            runtime_id,
        }))
    }

    async fn task(&self, dispatched_at: chrono::DateTime<Utc>) -> anyhow::Result<Uuid> {
        let task_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO agent_task_queue (id, agent_id, issue_id, status, priority, dispatched_at, originator_user_id, runtime_id) \
             VALUES ($1, $2, $3, 'dispatched', 0, $4, $5, $6)",
        )
        .bind(task_id)
        .bind(self.agent_id)
        .bind(self.issue_id)
        .bind(dispatched_at)
        .bind(self.user_id)
        .bind(self.runtime_id)
        .execute(&self.pool)
        .await?;
        Ok(task_id)
    }

    async fn cleanup(self) {
        let _ = sqlx::query("DELETE FROM task_token WHERE workspace_id = $1")
            .bind(self.workspace_id)
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("DELETE FROM agent_task_queue WHERE agent_id = $1")
            .bind(self.agent_id)
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("DELETE FROM issue WHERE workspace_id = $1")
            .bind(self.workspace_id)
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("DELETE FROM agent WHERE workspace_id = $1")
            .bind(self.workspace_id)
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("DELETE FROM agent_runtime WHERE workspace_id = $1")
            .bind(self.workspace_id)
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("DELETE FROM member WHERE workspace_id = $1")
            .bind(self.workspace_id)
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("DELETE FROM workspace WHERE id = $1")
            .bind(self.workspace_id)
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("DELETE FROM \"user\" WHERE id = $1")
            .bind(self.user_id)
            .execute(&self.pool)
            .await;
    }
}

fn task_scope() -> serde_json::Value {
    json!([{
        "action": "task.read",
        "resource_type": "task_run",
        "resource_id": "$task"
    }])
}

#[tokio::test]
async fn replay_terminal_expiry_revocation_and_child_narrowing_are_enforced() -> anyhow::Result<()>
{
    let Some(rows) = Rows::optional().await? else {
        return Ok(());
    };
    let dispatched_at = Utc::now();
    let task_id = rows.task(dispatched_at).await?;
    let expires_at = Some(Utc::now() + Duration::hours(1));
    let first_hash = format!("lease-first-{task_id}");
    let second_hash = format!("lease-second-{task_id}");
    let concurrent_scope = task_scope();
    let first = create_task_token(
        &rows.pool,
        &first_hash,
        task_id,
        rows.agent_id,
        rows.workspace_id,
        rows.user_id,
        expires_at,
        &concurrent_scope,
        None,
        Some(dispatched_at),
        1,
        Some(rows.user_id),
        Some(rows.runtime_id),
        Uuid::now_v7(),
    );
    let second = create_task_token(
        &rows.pool,
        &second_hash,
        task_id,
        rows.agent_id,
        rows.workspace_id,
        rows.user_id,
        expires_at,
        &concurrent_scope,
        None,
        Some(dispatched_at),
        1,
        Some(rows.user_id),
        Some(rows.runtime_id),
        Uuid::now_v7(),
    );
    let (first, second) = tokio::join!(first, second);
    let first = first?;
    let second = second?;
    assert_ne!(
        first.is_some(),
        second.is_some(),
        "one claim consumes one lease slot"
    );
    let (winner_hash, winner) = if let Some(lease) = first {
        (&first_hash, lease)
    } else {
        (&second_hash, second.expect("one insert must win"))
    };
    assert!(get_task_token_by_hash(&rows.pool, winner_hash)
        .await?
        .is_some());

    sqlx::query("UPDATE agent_task_queue SET runtime_id = NULL WHERE id = $1")
        .bind(task_id)
        .execute(&rows.pool)
        .await?;
    assert!(
        get_task_token_by_hash(&rows.pool, winner_hash)
            .await?
            .is_none(),
        "runtime reassignment invalidates the old device-bound lease"
    );
    sqlx::query("UPDATE agent_task_queue SET runtime_id = $2 WHERE id = $1")
        .bind(task_id)
        .bind(rows.runtime_id)
        .execute(&rows.pool)
        .await?;
    assert!(
        get_task_token_by_hash(&rows.pool, winner_hash)
            .await?
            .is_none(),
        "restoring task identity cannot revive a revoked lease"
    );

    sqlx::query("UPDATE agent_task_queue SET status = 'completed' WHERE id = $1")
        .bind(task_id)
        .execute(&rows.pool)
        .await?;
    assert!(get_task_token_by_hash(&rows.pool, winner_hash)
        .await?
        .is_none());
    let persisted_revocation: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT revoked_at FROM task_token WHERE id = $1")
            .bind(winner.id)
            .fetch_one(&rows.pool)
            .await?;
    assert!(
        persisted_revocation.is_some(),
        "terminal leases remain queryable as revoked audit evidence"
    );
    assert!(
        sqlx::query("UPDATE task_token SET revoked_at = NULL WHERE id = $1")
            .bind(winner.id)
            .execute(&rows.pool)
            .await
            .is_err()
    );
    assert!(create_task_token(
        &rows.pool,
        &format!("lease-terminal-replay-{task_id}"),
        task_id,
        rows.agent_id,
        rows.workspace_id,
        rows.user_id,
        Some(Utc::now() + Duration::hours(1)),
        &task_scope(),
        None,
        Some(dispatched_at),
        9,
        Some(rows.user_id),
        Some(rows.runtime_id),
        Uuid::now_v7(),
    )
    .await?
    .is_none());

    let expired_dispatched_at = Utc::now();
    let expired_task = rows.task(expired_dispatched_at).await?;
    let expired_hash = format!("lease-expired-{expired_task}");
    create_task_token(
        &rows.pool,
        &expired_hash,
        expired_task,
        rows.agent_id,
        rows.workspace_id,
        rows.user_id,
        Some(Utc::now() - Duration::seconds(1)),
        &task_scope(),
        None,
        Some(expired_dispatched_at),
        2,
        Some(rows.user_id),
        Some(rows.runtime_id),
        Uuid::now_v7(),
    )
    .await?;
    assert!(get_task_token_by_hash(&rows.pool, &expired_hash)
        .await?
        .is_none());

    let revoked_dispatched_at = Utc::now();
    let revoked_task = rows.task(revoked_dispatched_at).await?;
    let revoked_hash = format!("lease-revoked-{revoked_task}");
    let revoked = create_task_token(
        &rows.pool,
        &revoked_hash,
        revoked_task,
        rows.agent_id,
        rows.workspace_id,
        rows.user_id,
        Some(Utc::now() + Duration::hours(1)),
        &task_scope(),
        None,
        Some(revoked_dispatched_at),
        3,
        Some(rows.user_id),
        Some(rows.runtime_id),
        Uuid::now_v7(),
    )
    .await?
    .expect("active lease");
    assert_eq!(
        revoke_task_token(&rows.pool, revoked.id, "contract").await?,
        1
    );
    assert_eq!(
        revoke_task_token(&rows.pool, revoked.id, "replay").await?,
        0
    );
    assert!(get_task_token_by_hash(&rows.pool, &revoked_hash)
        .await?
        .is_none());
    assert!(create_task_token(
        &rows.pool,
        &format!("lease-revoked-replay-{revoked_task}"),
        revoked_task,
        rows.agent_id,
        rows.workspace_id,
        rows.user_id,
        Some(Utc::now() + Duration::hours(1)),
        &task_scope(),
        None,
        Some(revoked_dispatched_at),
        30,
        Some(rows.user_id),
        Some(rows.runtime_id),
        Uuid::now_v7(),
    )
    .await?
    .is_none());

    let parent_dispatched_at = Utc::now();
    let parent_task = rows.task(parent_dispatched_at).await?;
    create_task_token(
        &rows.pool,
        &format!("lease-parent-{parent_task}"),
        parent_task,
        rows.agent_id,
        rows.workspace_id,
        rows.user_id,
        Some(Utc::now() + Duration::hours(1)),
        &task_scope(),
        None,
        Some(parent_dispatched_at),
        4,
        Some(rows.user_id),
        Some(rows.runtime_id),
        Uuid::now_v7(),
    )
    .await?
    .expect("parent lease");
    let child_dispatched_at = Utc::now();
    let child_task = rows.task(child_dispatched_at).await?;
    sqlx::query("UPDATE agent_task_queue SET delegated_from_task_id = $2 WHERE id = $1")
        .bind(child_task)
        .bind(parent_task)
        .execute(&rows.pool)
        .await?;
    let requested = json!([
        {"action":"task.read","resource_type":"task_run","resource_id":"$task"},
        {"action":"credential.use","resource_type":"credential","resource_id":"*"}
    ]);
    let child = create_task_token(
        &rows.pool,
        &format!("lease-child-{child_task}"),
        child_task,
        rows.agent_id,
        rows.workspace_id,
        rows.user_id,
        Some(Utc::now() + Duration::hours(1)),
        &requested,
        Some(parent_task),
        Some(child_dispatched_at),
        5,
        Some(rows.user_id),
        Some(rows.runtime_id),
        Uuid::now_v7(),
    )
    .await?
    .expect("child lease");
    assert_eq!(child.delegation_depth, 1);
    assert_eq!(child.scope, task_scope());

    let rejected_dispatched_at = Utc::now();
    let rejected_task = rows.task(rejected_dispatched_at).await?;
    sqlx::query("UPDATE agent_task_queue SET delegated_from_task_id = $2 WHERE id = $1")
        .bind(rejected_task)
        .bind(parent_task)
        .execute(&rows.pool)
        .await?;
    assert!(create_task_token(
        &rows.pool,
        &format!("lease-widened-identity-{rejected_task}"),
        rejected_task,
        rows.agent_id,
        rows.workspace_id,
        rows.user_id,
        Some(Utc::now() + Duration::hours(1)),
        &task_scope(),
        Some(parent_task),
        Some(rejected_dispatched_at),
        6,
        Some(Uuid::now_v7()),
        Some(rows.runtime_id),
        Uuid::now_v7(),
    )
    .await?
    .is_none());
    assert!(
        !task_token_exists_for_claim(&rows.pool, rejected_task, Some(rejected_dispatched_at),)
            .await?
    );

    rows.cleanup().await;
    Ok(())
}
