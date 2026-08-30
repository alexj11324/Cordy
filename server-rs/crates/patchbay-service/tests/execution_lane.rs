//! Live PostgreSQL contracts for the execution-lane claim boundary.
//!
//! These tests intentionally use the real task service and claim SQL. They are
//! skipped without DATABASE_URL, while CI runs them against the migrated
//! PostgreSQL service.

use std::sync::Arc;

use patchbay_db::models::AgentTaskQueue;
use patchbay_db::queries::agent::{claim_agent_task, create_retry_task, get_agent_task};
use patchbay_service::task_service::TaskService;
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

async fn test_pool() -> anyhow::Result<Option<PgPool>> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        return Ok(None);
    };
    Ok(Some(
        PgPoolOptions::new()
            .max_connections(16)
            .connect(&url)
            .await?,
    ))
}

struct Fixture {
    pool: PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    runtime_id: Uuid,
    agent_id: Uuid,
    issue_id: Uuid,
    second_issue_id: Uuid,
    first_chat_id: Uuid,
    second_chat_id: Uuid,
}

impl Fixture {
    async fn create(pool: PgPool) -> anyhow::Result<Self> {
        let workspace_id = Uuid::now_v7();
        let user_id = Uuid::now_v7();
        let runtime_id = Uuid::now_v7();
        let agent_id = Uuid::now_v7();
        let issue_id = Uuid::now_v7();
        let second_issue_id = Uuid::now_v7();
        let first_chat_id = Uuid::now_v7();
        let second_chat_id = Uuid::now_v7();

        sqlx::query(
            "INSERT INTO workspace (id, name, slug) VALUES ($1, 'execution lane contract', $2)",
        )
        .bind(workspace_id)
        .bind(format!("execution-lane-{workspace_id}"))
        .execute(&pool)
        .await?;
        sqlx::query("INSERT INTO \"user\" (id, name, email) VALUES ($1, 'lane contract user', $2)")
            .bind(user_id)
            .bind(format!("execution-lane-{user_id}@example.test"))
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO agent_runtime (id, workspace_id, daemon_id, name, runtime_mode, provider, status, last_seen_at) \
             VALUES ($1, $2, $3, 'execution lane runtime', 'local', 'lane-contract', 'online', now())",
        )
        .bind(runtime_id)
        .bind(workspace_id)
        .bind(format!("execution-lane-{runtime_id}"))
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO agent (id, workspace_id, name, runtime_mode, status, max_concurrent_tasks, owner_id, runtime_id) \
             VALUES ($1, $2, 'execution lane agent', 'local', 'idle', 4, $3, $4)",
        )
        .bind(agent_id)
        .bind(workspace_id)
        .bind(user_id)
        .bind(runtime_id)
        .execute(&pool)
        .await?;

        for (id, number, title) in [
            (issue_id, 1, "execution lane issue"),
            (second_issue_id, 2, "execution lane second issue"),
        ] {
            sqlx::query(
                "INSERT INTO issue (id, workspace_id, title, status, priority, creator_type, creator_id, assignee_type, assignee_id, number) \
                 VALUES ($1, $2, $3, 'todo', 'none', 'member', $4, 'agent', $5, $6)",
            )
            .bind(id)
            .bind(workspace_id)
            .bind(title)
            .bind(user_id)
            .bind(agent_id)
            .bind(number)
            .execute(&pool)
            .await?;
        }

        for (id, title) in [
            (first_chat_id, "execution lane first chat"),
            (second_chat_id, "execution lane second chat"),
        ] {
            sqlx::query(
                "INSERT INTO chat_session (id, workspace_id, agent_id, creator_id, title) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(workspace_id)
            .bind(agent_id)
            .bind(user_id)
            .bind(title)
            .execute(&pool)
            .await?;
        }

        Ok(Self {
            pool,
            workspace_id,
            user_id,
            runtime_id,
            agent_id,
            issue_id,
            second_issue_id,
            first_chat_id,
            second_chat_id,
        })
    }

    async fn task(
        &self,
        issue_id: Option<Uuid>,
        chat_session_id: Option<Uuid>,
        status: &str,
        priority: i32,
        context: Value,
    ) -> anyhow::Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO agent_task_queue \
             (id, agent_id, runtime_id, issue_id, chat_session_id, status, priority, context, attempt, max_attempts, delivered_comment_ids) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 1, 3, '{}'::uuid[])",
        )
        .bind(id)
        .bind(self.agent_id)
        .bind(self.runtime_id)
        .bind(issue_id)
        .bind(chat_session_id)
        .bind(status)
        .bind(priority)
        .bind(context)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    async fn clear_tasks(&self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM agent_task_queue WHERE agent_id = $1")
            .bind(self.agent_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_status(&self, task_id: Uuid, status: &str) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE agent_task_queue \
             SET status = $2, dispatched_at = CASE WHEN $2 IN ('dispatched', 'running') THEN now() ELSE dispatched_at END, \
                 started_at = CASE WHEN $2 = 'running' THEN now() ELSE started_at END, \
                 completed_at = CASE WHEN $2 IN ('completed', 'failed', 'cancelled') THEN now() ELSE completed_at END \
             WHERE id = $1",
        )
        .bind(task_id)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn finish(&self, task_id: Uuid) -> anyhow::Result<()> {
        self.set_status(task_id, "completed").await
    }

    async fn set_capacity(&self, max_concurrent_tasks: i32) -> anyhow::Result<()> {
        sqlx::query("UPDATE agent SET max_concurrent_tasks = $2 WHERE id = $1")
            .bind(self.agent_id)
            .bind(max_concurrent_tasks)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn cleanup(&self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM agent_task_queue WHERE agent_id = $1")
            .bind(self.agent_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM chat_session WHERE id IN ($1, $2)")
            .bind(self.first_chat_id)
            .bind(self.second_chat_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM issue WHERE id IN ($1, $2)")
            .bind(self.issue_id)
            .bind(self.second_issue_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM agent WHERE id = $1")
            .bind(self.agent_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM agent_runtime WHERE id = $1")
            .bind(self.runtime_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM \"user\" WHERE id = $1")
            .bind(self.user_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM workspace WHERE id = $1")
            .bind(self.workspace_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

async fn claim_service_task(
    service: &TaskService,
    agent_id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    service
        .claim_task(agent_id)
        .await
        .map_err(|error| anyhow::anyhow!("claim execution-lane fixture task: {error}"))
}

async fn run_lane_contracts(fixture: &Fixture) -> anyhow::Result<()> {
    let service = TaskService::new(fixture.pool.clone(), Arc::new(patchbay_events::Bus::new()));

    // A single chat session is one lane even when it has multiple queued
    // turns. Once the first turn is terminal, the next turn can claim.
    fixture.clear_tasks().await?;
    fixture.set_capacity(2).await?;
    let first = fixture
        .task(None, Some(fixture.first_chat_id), "queued", 20, Value::Null)
        .await?;
    let second = fixture
        .task(None, Some(fixture.first_chat_id), "queued", 10, Value::Null)
        .await?;
    let claimed_first = claim_service_task(&service, fixture.agent_id)
        .await?
        .expect("first chat claim");
    assert_eq!(claimed_first.id, first);
    assert_eq!(
        claimed_first.execution_lane_key.as_str(),
        &format!("chat:{}", fixture.first_chat_id)
    );
    assert!(claim_service_task(&service, fixture.agent_id)
        .await?
        .is_none());
    fixture.finish(first).await?;
    assert_eq!(
        claim_service_task(&service, fixture.agent_id)
            .await?
            .expect("second chat claim")
            .id,
        second
    );

    // Different chat sessions on the same Agent use different lanes and can
    // consume two capacity slots concurrently.
    fixture.clear_tasks().await?;
    let first_chat_task = fixture
        .task(None, Some(fixture.first_chat_id), "queued", 20, Value::Null)
        .await?;
    let second_chat_task = fixture
        .task(
            None,
            Some(fixture.second_chat_id),
            "queued",
            10,
            Value::Null,
        )
        .await?;
    let claimed_a = claim_service_task(&service, fixture.agent_id)
        .await?
        .expect("first chat lane");
    let claimed_b = claim_service_task(&service, fixture.agent_id)
        .await?
        .expect("second chat lane");
    assert_eq!(claimed_a.id, first_chat_task);
    assert_eq!(claimed_b.id, second_chat_task);
    assert_ne!(claimed_a.execution_lane_key, claimed_b.execution_lane_key);
    fixture.finish(first_chat_task).await?;
    fixture.finish(second_chat_task).await?;

    // The issue main lane is serialized, but a side chat lane for the same
    // issue is independent. Running main is moved to `running` so the legacy
    // pending index does not reject the second main row before claim tests it.
    fixture.clear_tasks().await?;
    let main_first = fixture
        .task(Some(fixture.issue_id), None, "queued", 30, Value::Null)
        .await?;
    let claimed_main = claim_service_task(&service, fixture.agent_id)
        .await?
        .expect("main claim");
    assert_eq!(claimed_main.id, main_first);
    fixture.set_status(main_first, "running").await?;
    let main_second = fixture
        .task(Some(fixture.issue_id), None, "queued", 20, Value::Null)
        .await?;
    let side = fixture
        .task(
            Some(fixture.issue_id),
            None,
            "queued",
            10,
            serde_json::json!({
                "side_chat_parent_task_id": main_first.to_string(),
                "side_chat_root_comment_id": "lane-side-root"
            }),
        )
        .await?;
    assert!(claim_service_task(&service, fixture.agent_id)
        .await?
        .is_some_and(|task| {
            task.id == side
                && task.execution_lane_key.to_string()
                    == format!(
                        "issue:{}:agent:{}:side:lane-side-root",
                        fixture.issue_id, fixture.agent_id
                    )
        }));
    fixture.finish(main_first).await?;
    fixture.finish(side).await?;
    assert_eq!(
        claim_service_task(&service, fixture.agent_id)
            .await?
            .expect("main successor")
            .id,
        main_second
    );

    // Unscoped work has one default lane per Agent.
    fixture.clear_tasks().await?;
    let default_first = fixture.task(None, None, "queued", 20, Value::Null).await?;
    let default_second = fixture.task(None, None, "queued", 10, Value::Null).await?;
    let claimed_default = claim_service_task(&service, fixture.agent_id)
        .await?
        .expect("default claim");
    assert_eq!(claimed_default.id, default_first);
    assert_eq!(
        claimed_default.execution_lane_key.as_str(),
        &format!("agent:{}:default", fixture.agent_id)
    );
    assert!(claim_service_task(&service, fixture.agent_id)
        .await?
        .is_none());
    fixture.finish(default_first).await?;
    assert_eq!(
        claim_service_task(&service, fixture.agent_id)
            .await?
            .expect("default successor")
            .id,
        default_second
    );

    // A local-directory waiter remains active for lane and capacity guards.
    fixture.clear_tasks().await?;
    let waiting = fixture
        .task(
            Some(fixture.second_issue_id),
            None,
            "waiting_local_directory",
            20,
            Value::Null,
        )
        .await?;
    let waiting_successor = fixture
        .task(
            Some(fixture.second_issue_id),
            None,
            "queued",
            10,
            Value::Null,
        )
        .await?;
    assert!(claim_service_task(&service, fixture.agent_id)
        .await?
        .is_none());
    fixture.finish(waiting).await?;
    assert_eq!(
        claim_service_task(&service, fixture.agent_id)
            .await?
            .expect("waiting successor")
            .id,
        waiting_successor
    );

    // Capacity still applies across different lanes.
    fixture.clear_tasks().await?;
    fixture.set_capacity(1).await?;
    let capacity_first = fixture
        .task(None, Some(fixture.first_chat_id), "queued", 20, Value::Null)
        .await?;
    let capacity_second = fixture
        .task(
            None,
            Some(fixture.second_chat_id),
            "queued",
            10,
            Value::Null,
        )
        .await?;
    assert_eq!(
        claim_service_task(&service, fixture.agent_id)
            .await?
            .expect("capacity first")
            .id,
        capacity_first
    );
    assert!(claim_service_task(&service, fixture.agent_id)
        .await?
        .is_none());
    fixture.finish(capacity_first).await?;
    assert_eq!(
        claim_service_task(&service, fixture.agent_id)
            .await?
            .expect("capacity successor")
            .id,
        capacity_second
    );
    fixture.finish(capacity_second).await?;
    fixture.set_capacity(4).await?;

    // Auto-retry copies the identity and provider continuity fields. The lane
    // is regenerated from the copied routing fields; no transcript is copied.
    fixture.clear_tasks().await?;
    let retry_parent = fixture
        .task(
            Some(fixture.issue_id),
            None,
            "failed",
            20,
            serde_json::json!({
                "side_chat_parent_task_id": "retry-parent",
                "side_chat_root_comment_id": "retry-side-root"
            }),
        )
        .await?;
    sqlx::query(
        "UPDATE agent_task_queue SET session_id = 'provider-session-1', work_dir = '/durable/project', failure_reason = 'agent_error' WHERE id = $1",
    )
    .bind(retry_parent)
    .execute(&fixture.pool)
    .await?;
    let parent = get_agent_task(&fixture.pool, retry_parent)
        .await?
        .expect("retry parent");
    let retry = create_retry_task(
        &fixture.pool,
        retry_parent,
        None,
        None,
        &serde_json::json!({}),
        &serde_json::json!({}),
        Uuid::now_v7(),
    )
    .await?
    .expect("retry row");
    assert_eq!(retry.execution_lane_key, parent.execution_lane_key);
    assert_eq!(retry.session_id, parent.session_id);
    assert_eq!(retry.work_dir, parent.work_dir);

    Ok(())
}

#[tokio::test]
async fn execution_lanes_serialize_isolate_and_preserve_retry_continuity() -> anyhow::Result<()> {
    let Some(pool) = test_pool().await? else {
        eprintln!("skipping: DATABASE_URL not set");
        return Ok(());
    };
    let fixture = Fixture::create(pool).await?;
    let result = run_lane_contracts(&fixture).await;
    fixture.cleanup().await?;
    result
}

#[tokio::test]
async fn concurrent_claims_have_one_lane_winner() -> anyhow::Result<()> {
    let Some(pool) = test_pool().await? else {
        eprintln!("skipping: DATABASE_URL not set");
        return Ok(());
    };
    let fixture = Fixture::create(pool).await?;
    fixture.clear_tasks().await?;
    fixture.set_capacity(4).await?;
    let first = fixture.task(None, None, "queued", 20, Value::Null).await?;
    let second = fixture.task(None, None, "queued", 10, Value::Null).await?;
    let agent_id = fixture.agent_id;
    let runtime_id = fixture.runtime_id;

    let first_claim = tokio::spawn({
        let pool = fixture.pool.clone();
        async move { claim_agent_task(&pool, 45.0, agent_id, runtime_id, 300.0).await }
    });
    let second_claim = tokio::spawn({
        let pool = fixture.pool.clone();
        async move { claim_agent_task(&pool, 45.0, agent_id, runtime_id, 300.0).await }
    });
    let results = [first_claim.await?, second_claim.await?];
    let winners = results
        .iter()
        .filter_map(|result| result.as_ref().ok().and_then(Option::as_ref))
        .count();
    assert_eq!(winners, 1, "same default lane must have one claim winner");
    assert!(results.iter().all(|result| match result {
        Ok(_) => true,
        Err(error) => error
            .downcast_ref::<sqlx::Error>()
            .and_then(|db_error| db_error.as_database_error())
            .and_then(|db_error| db_error.code().map(|code| code == "23505"))
            .unwrap_or(false),
    }));
    let active_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agent_task_queue WHERE id IN ($1, $2) AND status IN ('dispatched', 'running', 'waiting_local_directory')",
    )
    .bind(first)
    .bind(second)
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(active_count, 1);
    fixture.cleanup().await?;
    Ok(())
}
