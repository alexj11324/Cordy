//! Canonical Work Product identity and explicit Issue/Task/Run relations.
//!
//! Provider-specific tables remain snapshot stores. This module is the only
//! association path: webhook code may mirror a provider object and ensure its
//! Work Product identity, but only an authenticated explicit attach creates a
//! relation.

#![allow(clippy::too_many_arguments)]

use crate::models::{AgentTaskExecutionProvenance, WorkProduct, WorkProductRelation};
use sqlx::Row;
use uuid::Uuid;

const WORK_PRODUCT_COLUMNS: &str = "id, workspace_id, kind, provider, external_identity, external_url, provider_record_type, provider_record_id, created_at, updated_at";
const RELATION_COLUMNS: &str = "id, workspace_id, work_product_id, issue_id, task_id, run_id, relation_key, relation_source, attached_by_type, attached_by_id, attached_at, close_intent, detached_at, detached_by_type, detached_by_id, detached_task_id, detached_run_id";
const PROVENANCE_COLUMNS: &str = "task_id, workspace_id, run_id, repo_identity, execution_workspace, head_branch, head_sha, head_state, started_at, finished_at, discovery_status, discovery_lease_id, discovery_match_count, discovery_reason, discovery_work_product_id, discovery_at, updated_at";

pub const RELATION_SOURCE_MANUAL_EXPLICIT: &str = "manual_explicit";
pub const RELATION_SOURCE_TASK_EXPLICIT: &str = "task_explicit";
pub const RELATION_SOURCE_EXECUTION_BRANCH_DISCOVERY: &str = "execution_branch_discovery";

pub const DISCOVERY_NOT_ATTEMPTED: &str = "not_attempted";
pub const DISCOVERY_PENDING: &str = "pending";
pub const DISCOVERY_IN_PROGRESS: &str = "in_progress";
pub const DISCOVERY_UNASSOCIATED: &str = "unassociated";
pub const DISCOVERY_AMBIGUOUS: &str = "ambiguous";
pub const DISCOVERY_ASSOCIATED: &str = "associated";
pub const DISCOVERY_INELIGIBLE: &str = "ineligible";

fn qualified_columns(columns: &str, alias: &str) -> String {
    columns
        .split(", ")
        .map(|column| format!("{alias}.{column}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn work_product_from_row(row: &sqlx::postgres::PgRow) -> anyhow::Result<WorkProduct> {
    Ok(WorkProduct {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        kind: row.try_get(2)?,
        provider: row.try_get(3)?,
        external_identity: row.try_get(4)?,
        external_url: row.try_get(5)?,
        provider_record_type: row.try_get(6)?,
        provider_record_id: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
    })
}

fn relation_from_row(row: &sqlx::postgres::PgRow) -> anyhow::Result<WorkProductRelation> {
    Ok(WorkProductRelation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        work_product_id: row.try_get(2)?,
        issue_id: row.try_get(3)?,
        task_id: row.try_get(4)?,
        run_id: row.try_get(5)?,
        relation_key: row.try_get(6)?,
        relation_source: row.try_get(7)?,
        attached_by_type: row.try_get(8)?,
        attached_by_id: row.try_get(9)?,
        attached_at: row.try_get(10)?,
        close_intent: row.try_get(11)?,
        detached_at: row.try_get(12)?,
        detached_by_type: row.try_get(13)?,
        detached_by_id: row.try_get(14)?,
        detached_task_id: row.try_get(15)?,
        detached_run_id: row.try_get(16)?,
    })
}

fn work_product_relation_from_joined_row(
    row: &sqlx::postgres::PgRow,
) -> anyhow::Result<(WorkProduct, WorkProductRelation)> {
    Ok((
        WorkProduct {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            kind: row.try_get(2)?,
            provider: row.try_get(3)?,
            external_identity: row.try_get(4)?,
            external_url: row.try_get(5)?,
            provider_record_type: row.try_get(6)?,
            provider_record_id: row.try_get(7)?,
            created_at: row.try_get(8)?,
            updated_at: row.try_get(9)?,
        },
        WorkProductRelation {
            id: row.try_get(10)?,
            workspace_id: row.try_get(11)?,
            work_product_id: row.try_get(12)?,
            issue_id: row.try_get(13)?,
            task_id: row.try_get(14)?,
            run_id: row.try_get(15)?,
            relation_key: row.try_get(16)?,
            relation_source: row.try_get(17)?,
            attached_by_type: row.try_get(18)?,
            attached_by_id: row.try_get(19)?,
            attached_at: row.try_get(20)?,
            close_intent: row.try_get(21)?,
            detached_at: row.try_get(22)?,
            detached_by_type: row.try_get(23)?,
            detached_by_id: row.try_get(24)?,
            detached_task_id: row.try_get(25)?,
            detached_run_id: row.try_get(26)?,
        },
    ))
}

fn provenance_from_row(
    row: &sqlx::postgres::PgRow,
) -> anyhow::Result<AgentTaskExecutionProvenance> {
    let repo_identity: String = row.try_get(3)?;
    let execution_workspace: String = row.try_get(4)?;
    Ok(AgentTaskExecutionProvenance {
        task_id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        run_id: row.try_get(2)?,
        repo_identity: (!repo_identity.is_empty()).then_some(repo_identity),
        execution_workspace: (!execution_workspace.is_empty()).then_some(execution_workspace),
        head_branch: row.try_get(5)?,
        head_sha: row.try_get(6)?,
        head_state: row.try_get(7)?,
        started_at: row.try_get(8)?,
        finished_at: row.try_get(9)?,
        discovery_status: row.try_get(10)?,
        discovery_lease_id: row.try_get(11)?,
        discovery_match_count: row.try_get(12)?,
        discovery_reason: row.try_get(13)?,
        discovery_work_product_id: row.try_get(14)?,
        discovery_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
    })
}

/// Upsert an external identity without creating an Issue relation. Webhooks
/// use this for both linked and unlinked provider objects.
pub async fn upsert_work_product(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    kind: &str,
    provider: &str,
    external_identity: &str,
    external_url: Option<&str>,
    provider_record_type: Option<&str>,
    provider_record_id: Option<Uuid>,
) -> anyhow::Result<WorkProduct> {
    anyhow::ensure!(valid_kind(kind), "invalid work product kind");
    anyhow::ensure!(valid_provider(provider), "invalid work product provider");
    anyhow::ensure!(
        valid_external_identity(external_identity),
        "invalid work product external identity"
    );
    let query = format!(
        r#"INSERT INTO work_product (
    workspace_id, kind, provider, external_identity, external_url,
    provider_record_type, provider_record_id
) VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (workspace_id, provider, external_identity) DO UPDATE SET
    external_url = COALESCE(EXCLUDED.external_url, work_product.external_url),
    provider_record_type = COALESCE(EXCLUDED.provider_record_type, work_product.provider_record_type),
    provider_record_id = COALESCE(EXCLUDED.provider_record_id, work_product.provider_record_id),
    updated_at = now()
RETURNING {WORK_PRODUCT_COLUMNS}"#
    );
    let row = sqlx::query(&query)
        .bind(workspace_id)
        .bind(kind)
        .bind(provider)
        .bind(external_identity)
        .bind(external_url)
        .bind(provider_record_type)
        .bind(provider_record_id)
        .fetch_one(executor)
        .await?;
    work_product_from_row(&row)
}

pub async fn get_work_product_by_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    id: Uuid,
) -> anyhow::Result<Option<WorkProduct>> {
    let query = format!(
        "SELECT {WORK_PRODUCT_COLUMNS} FROM work_product WHERE workspace_id = $1 AND id = $2"
    );
    let row = sqlx::query(&query)
        .bind(workspace_id)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    row.as_ref().map(work_product_from_row).transpose()
}

/// Locks a Work Product row for the duration of the caller's transaction.
/// Relation insertion and product cleanup use this same row lock so a cleanup
/// cannot commit after observing the product but before observing a concurrent
/// relation insert.
pub async fn lock_work_product(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    work_product_id: Uuid,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        r#"SELECT id
FROM work_product
WHERE workspace_id = $1 AND id = $2
FOR UPDATE"#,
    )
    .bind(workspace_id)
    .bind(work_product_id)
    .fetch_optional(executor)
    .await?;
    Ok(row.is_some())
}

pub async fn attach_work_product_relation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    work_product_id: Uuid,
    issue_id: Option<Uuid>,
    task_id: Option<Uuid>,
    run_id: Option<Uuid>,
    relation_key: &str,
    relation_source: &str,
    attached_by_type: &str,
    attached_by_id: Uuid,
    close_intent: bool,
) -> anyhow::Result<WorkProductRelation> {
    anyhow::ensure!(
        matches!(
            relation_source,
            RELATION_SOURCE_MANUAL_EXPLICIT
                | RELATION_SOURCE_TASK_EXPLICIT
                | RELATION_SOURCE_EXECUTION_BRANCH_DISCOVERY
        ),
        "invalid work product relation source"
    );
    anyhow::ensure!(
        match relation_source {
            // A manual confirmation may converge an existing discovery row
            // without erasing the producing task/run provenance already on
            // that row. New manual rows still pass NULL task/run values from
            // the handler; the schema permits retained producer provenance on
            // an upgraded row.
            RELATION_SOURCE_MANUAL_EXPLICIT => attached_by_type == "user",
            RELATION_SOURCE_TASK_EXPLICIT | RELATION_SOURCE_EXECUTION_BRANCH_DISCOVERY => {
                attached_by_type == "agent" && task_id.is_some()
            }
            _ => false,
        },
        "relation source does not match its authenticated execution actor"
    );
    let query = format!(
        r#"WITH locked_issue AS MATERIALIZED (
    SELECT issue.id
    FROM issue
    WHERE issue.id = $3
      AND issue.workspace_id = $1
    FOR UPDATE
), issue_fence AS MATERIALIZED (
    -- Consume the lock CTE before the INSERT. MATERIALIZED alone does not
    -- guarantee evaluation order; this fence keeps the issue -> relation
    -- order consistent with issue deletion.
    SELECT count(*) AS locked_count FROM locked_issue
)
INSERT INTO work_product_relation (
    workspace_id, work_product_id, issue_id, task_id, run_id, relation_key,
    relation_source, attached_by_type, attached_by_id, close_intent
) SELECT $1, $2, $3, $4, $5,
    COALESCE(
        CASE WHEN $7 = 'manual_explicit' THEN (
            SELECT existing.relation_key
            FROM work_product_relation existing
            WHERE existing.workspace_id = $1
              AND existing.work_product_id = $2
              AND existing.issue_id IS NOT DISTINCT FROM $3::uuid
              AND existing.detached_at IS NULL
            ORDER BY existing.attached_at ASC, existing.id ASC
            LIMIT 1
        ) ELSE NULL END,
        $6
    ),
    $7, $8, $9, $10
FROM issue_fence
LEFT JOIN locked_issue ON TRUE
WHERE EXISTS (
    SELECT 1 FROM work_product
    WHERE id = $2 AND workspace_id = $1
)
  AND issue_fence.locked_count >= 0
  AND ($3::uuid IS NULL OR locked_issue.id IS NOT NULL)
  AND ($4::uuid IS NULL OR EXISTS (
      SELECT 1
      FROM agent_task_queue task
      JOIN agent ON agent.id = task.agent_id
      WHERE task.id = $4
        AND agent.workspace_id = $1
        AND task.issue_id IS NOT DISTINCT FROM $3::uuid
        AND ($8 <> 'agent' OR task.agent_id = $9)
  ))
  AND ($5::uuid IS NULL OR EXISTS (
      SELECT 1
      FROM agent_task_queue task
      JOIN agent ON agent.id = task.agent_id
      WHERE task.id = $4
        AND task.autopilot_run_id = $5
        AND agent.workspace_id = $1
  ))
ON CONFLICT (work_product_id, relation_key) WHERE detached_at IS NULL DO UPDATE SET
    relation_source = CASE
        WHEN work_product_relation.relation_source = 'execution_branch_discovery'
         AND EXCLUDED.relation_source IN ('task_explicit', 'manual_explicit')
        THEN EXCLUDED.relation_source
        ELSE work_product_relation.relation_source
    END,
    attached_by_type = CASE
        WHEN work_product_relation.relation_source = 'execution_branch_discovery'
         AND EXCLUDED.relation_source IN ('task_explicit', 'manual_explicit')
        THEN EXCLUDED.attached_by_type
        ELSE work_product_relation.attached_by_type
    END,
    attached_by_id = CASE
        WHEN work_product_relation.relation_source = 'execution_branch_discovery'
         AND EXCLUDED.relation_source IN ('task_explicit', 'manual_explicit')
        THEN EXCLUDED.attached_by_id
        ELSE work_product_relation.attached_by_id
    END,
    attached_at = CASE
        WHEN work_product_relation.relation_source = 'execution_branch_discovery'
         AND EXCLUDED.relation_source IN ('task_explicit', 'manual_explicit')
        THEN EXCLUDED.attached_at
        ELSE work_product_relation.attached_at
    END,
    close_intent = work_product_relation.close_intent OR EXCLUDED.close_intent
RETURNING {RELATION_COLUMNS}"#
    );
    let row = sqlx::query(&query)
        .bind(workspace_id)
        .bind(work_product_id)
        .bind(issue_id)
        .bind(task_id)
        .bind(run_id)
        .bind(relation_key)
        .bind(relation_source)
        .bind(attached_by_type)
        .bind(attached_by_id)
        .bind(close_intent)
        .fetch_optional(executor)
        .await?
        .ok_or_else(|| anyhow::anyhow!("work product is not in the requested workspace"))?;
    relation_from_row(&row)
}

pub async fn list_work_products_by_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<Vec<(WorkProduct, WorkProductRelation)>> {
    let query = format!(
        r#"WITH linked AS (
SELECT DISTINCT ON (wp.id) {}, {}
FROM work_product wp
JOIN work_product_relation wpr
  ON wpr.work_product_id = wp.id
 AND wpr.workspace_id = wp.workspace_id
 AND wpr.issue_id = $2
 AND wpr.detached_at IS NULL
WHERE wp.workspace_id = $1
ORDER BY wp.id,
    CASE wpr.relation_source
        WHEN 'manual_explicit' THEN 0
        WHEN 'task_explicit' THEN 1
        ELSE 2
    END,
    wpr.attached_at DESC,
    wpr.id DESC
)
SELECT * FROM linked
ORDER BY 21 DESC, 10 DESC, 1 DESC"#,
        qualified_columns(WORK_PRODUCT_COLUMNS, "wp"),
        qualified_columns(RELATION_COLUMNS, "wpr")
    );
    let rows = sqlx::query(&query)
        .bind(workspace_id)
        .bind(issue_id)
        .fetch_all(executor)
        .await?;
    rows.iter()
        .map(work_product_relation_from_joined_row)
        .collect()
}

pub async fn list_issue_ids_for_work_product(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    work_product_id: Uuid,
) -> anyhow::Result<Vec<Uuid>> {
    let rows = sqlx::query(
        r#"SELECT DISTINCT issue_id
FROM work_product_relation
WHERE workspace_id = $1 AND work_product_id = $2
  AND issue_id IS NOT NULL AND detached_at IS NULL
ORDER BY issue_id"#,
    )
    .bind(workspace_id)
    .bind(work_product_id)
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| row.try_get(0).map_err(Into::into))
        .collect()
}

pub async fn list_relations_for_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<Vec<WorkProductRelation>> {
    let query = format!(
        r#"SELECT {RELATION_COLUMNS}
FROM work_product_relation
WHERE workspace_id = $1 AND task_id = $2 AND detached_at IS NULL
ORDER BY attached_at DESC, id DESC"#
    );
    let rows = sqlx::query(&query)
        .bind(workspace_id)
        .bind(task_id)
        .fetch_all(executor)
        .await?;
    rows.iter().map(relation_from_row).collect()
}

pub async fn list_work_products_by_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<Vec<(WorkProduct, WorkProductRelation)>> {
    let query = format!(
        r#"SELECT {}, {}
FROM work_product wp
JOIN work_product_relation wpr
  ON wpr.work_product_id = wp.id
 AND wpr.workspace_id = wp.workspace_id
WHERE wp.workspace_id = $1
  AND wpr.workspace_id = $1
  AND wpr.task_id = $2
  AND wpr.detached_at IS NULL
ORDER BY wpr.attached_at DESC, wpr.id DESC"#,
        qualified_columns(WORK_PRODUCT_COLUMNS, "wp"),
        qualified_columns(RELATION_COLUMNS, "wpr")
    );
    let rows = sqlx::query(&query)
        .bind(workspace_id)
        .bind(task_id)
        .fetch_all(executor)
        .await?;
    rows.iter()
        .map(work_product_relation_from_joined_row)
        .collect()
}

pub async fn has_active_relation_for_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        r#"SELECT EXISTS(
    SELECT 1 FROM work_product_relation
    WHERE workspace_id = $1
      AND task_id = $2
      AND relation_source = 'task_explicit'
      AND detached_at IS NULL
)"#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_one(executor)
    .await?;
    Ok(row.try_get(0)?)
}

pub async fn list_unassociated_work_products(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<WorkProduct>> {
    let query = format!(
        r#"SELECT {WORK_PRODUCT_COLUMNS}
FROM work_product wp
WHERE wp.workspace_id = $1
  AND NOT EXISTS (
      SELECT 1 FROM work_product_relation wpr
      WHERE wpr.workspace_id = wp.workspace_id
        AND wpr.work_product_id = wp.id
        AND wpr.detached_at IS NULL
        AND wpr.issue_id IS NOT NULL
  )
ORDER BY wp.updated_at DESC, wp.id DESC"#
    );
    let rows = sqlx::query(&query)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    rows.iter().map(work_product_from_row).collect()
}

pub async fn list_execution_provenances(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskExecutionProvenance>> {
    let query = format!(
        "SELECT {PROVENANCE_COLUMNS} FROM agent_task_execution_provenance WHERE workspace_id = $1 AND task_id = $2 ORDER BY updated_at DESC, repo_identity ASC, execution_workspace ASC"
    );
    let row = sqlx::query(&query)
        .bind(workspace_id)
        .bind(task_id)
        .fetch_all(executor)
        .await?;
    row.iter().map(provenance_from_row).collect()
}

/// Records one task-owned checkout before the agent starts and refreshes that
/// same checkout at terminal delivery. The natural key includes the exact
/// repository and execution workspace so one task can own several checkouts;
/// a terminal delivery replaces known execution head fields, while an adapter
/// that has no local checkout reports `unknown` and preserves facts already
/// recorded by the checkout endpoint.
pub async fn upsert_execution_provenance(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
    workspace_id: Uuid,
    run_id: Option<Uuid>,
    repo_identity: Option<&str>,
    execution_workspace: Option<&str>,
    head_branch: Option<&str>,
    head_sha: Option<&str>,
    head_state: &str,
    finished: bool,
) -> anyhow::Result<AgentTaskExecutionProvenance> {
    anyhow::ensure!(
        matches!(head_state, "attached" | "detached" | "default" | "unknown"),
        "invalid execution head state"
    );
    let repo_identity = repo_identity.unwrap_or_default();
    let execution_workspace = execution_workspace.unwrap_or_default();
    let query = format!(
        r#"INSERT INTO agent_task_execution_provenance (
    task_id, workspace_id, run_id, repo_identity, execution_workspace,
    head_branch, head_sha, head_state, started_at, finished_at
) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now(), CASE WHEN $9 THEN now() ELSE NULL END)
ON CONFLICT (workspace_id, task_id, repo_identity, execution_workspace) DO UPDATE SET
    run_id = COALESCE(agent_task_execution_provenance.run_id, EXCLUDED.run_id),
    head_branch = CASE
        WHEN $9 AND EXCLUDED.head_state <> 'unknown' THEN EXCLUDED.head_branch
        WHEN $9 THEN COALESCE(agent_task_execution_provenance.head_branch, EXCLUDED.head_branch)
        WHEN agent_task_execution_provenance.finished_at IS NOT NULL
        THEN agent_task_execution_provenance.head_branch
        ELSE COALESCE(agent_task_execution_provenance.head_branch, EXCLUDED.head_branch)
    END,
    head_sha = CASE
        WHEN $9 AND EXCLUDED.head_state <> 'unknown'
        THEN EXCLUDED.head_sha
        WHEN $9
        THEN COALESCE(agent_task_execution_provenance.head_sha, EXCLUDED.head_sha)
        WHEN agent_task_execution_provenance.finished_at IS NOT NULL
        THEN agent_task_execution_provenance.head_sha
        ELSE COALESCE(agent_task_execution_provenance.head_sha, EXCLUDED.head_sha)
    END,
    head_state = CASE
        WHEN $9 AND EXCLUDED.head_state <> 'unknown' THEN EXCLUDED.head_state
        WHEN $9 THEN agent_task_execution_provenance.head_state
        WHEN agent_task_execution_provenance.finished_at IS NOT NULL
        THEN agent_task_execution_provenance.head_state
        WHEN agent_task_execution_provenance.head_state <> 'unknown'
        THEN agent_task_execution_provenance.head_state
        ELSE EXCLUDED.head_state
    END,
    started_at = COALESCE(agent_task_execution_provenance.started_at, EXCLUDED.started_at),
    finished_at = CASE WHEN $9 THEN now() ELSE agent_task_execution_provenance.finished_at END,
    updated_at = now()
RETURNING {PROVENANCE_COLUMNS}"#
    );
    let row = sqlx::query(&query)
        .bind(task_id)
        .bind(workspace_id)
        .bind(run_id)
        .bind(repo_identity)
        .bind(execution_workspace)
        .bind(head_branch)
        .bind(head_sha)
        .bind(head_state)
        .bind(finished)
        .fetch_one(executor)
        .await?;
    provenance_from_row(&row)
}

pub async fn mark_execution_discovery(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    provenance: &AgentTaskExecutionProvenance,
    status: &str,
    match_count: i32,
    reason: Option<&str>,
    work_product_id: Option<Uuid>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        matches!(
            status,
            DISCOVERY_UNASSOCIATED
                | DISCOVERY_AMBIGUOUS
                | DISCOVERY_ASSOCIATED
                | DISCOVERY_INELIGIBLE
        ),
        "invalid execution discovery status"
    );
    let result = sqlx::query(
        r#"UPDATE agent_task_execution_provenance
SET discovery_status = $3,
    discovery_lease_id = NULL,
    discovery_match_count = $4,
    discovery_reason = $5,
    discovery_work_product_id = $6,
    discovery_at = now(),
    updated_at = now()
WHERE workspace_id = $1
  AND task_id = $2
  AND repo_identity = $7
  AND execution_workspace = $8
  AND discovery_lease_id = $9
  AND discovery_status = 'in_progress'"#,
    )
    .bind(provenance.workspace_id)
    .bind(provenance.task_id)
    .bind(status)
    .bind(match_count)
    .bind(reason)
    .bind(work_product_id)
    .bind(provenance.repo_identity.as_deref().unwrap_or_default())
    .bind(
        provenance
            .execution_workspace
            .as_deref()
            .unwrap_or_default(),
    )
    .bind(provenance.discovery_lease_id)
    .execute(executor)
    .await?;
    anyhow::ensure!(result.rows_affected() == 1, "execution discovery lease lost");
    Ok(())
}

/// Makes terminal discovery durable before its asynchronous worker is
/// spawned. A process restart can therefore drain the pending rows instead of
/// losing a `not_attempted` discovery with no audit trail.
pub async fn mark_task_discovery_pending(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"UPDATE agent_task_execution_provenance
SET discovery_status = 'pending',
    discovery_lease_id = NULL,
    discovery_match_count = 0,
    discovery_reason = NULL,
    discovery_work_product_id = NULL,
    discovery_at = NULL,
    finished_at = COALESCE(finished_at, now()),
    updated_at = now()
WHERE workspace_id = $1
  AND task_id = $2
  AND discovery_status = 'not_attempted'"#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Closes the durable discovery queue when a task already has an explicit
/// relation. The terminal provenance remains auditable, but no branch lookup
/// is scheduled or retried for that task.
pub async fn mark_task_discovery_skipped_for_explicit_relation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"UPDATE agent_task_execution_provenance
SET discovery_status = 'associated',
    discovery_lease_id = NULL,
    discovery_match_count = 0,
    discovery_reason = 'explicit_relation_exists',
    discovery_work_product_id = (
        SELECT relation.work_product_id
        FROM work_product_relation relation
        WHERE relation.workspace_id = $1
          AND relation.task_id = $2
          AND relation.relation_source = 'task_explicit'
          AND relation.detached_at IS NULL
        ORDER BY relation.attached_at ASC, relation.id ASC
        LIMIT 1
    ),
    discovery_at = now(),
    updated_at = now()
WHERE workspace_id = $1
  AND task_id = $2
  AND discovery_status IN ('not_attempted', 'pending', 'in_progress')"#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Claims a pending row for one discovery worker. Stale in-progress rows are
/// reclaimable after a process crash; final states are never reopened.
pub async fn claim_execution_discovery(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    provenance: &AgentTaskExecutionProvenance,
) -> anyhow::Result<Option<AgentTaskExecutionProvenance>> {
    let query = format!(
        r#"UPDATE agent_task_execution_provenance
SET discovery_status = 'in_progress',
    discovery_lease_id = gen_random_uuid(),
    discovery_at = now(),
    updated_at = now()
WHERE workspace_id = $1
  AND task_id = $2
  AND repo_identity = $3
  AND execution_workspace = $4
  AND (
      discovery_status = 'pending'
      OR (discovery_status = 'in_progress' AND updated_at < now() - interval '5 minutes')
  )
RETURNING {PROVENANCE_COLUMNS}"#
    );
    let row = sqlx::query(&query)
        .bind(provenance.workspace_id)
        .bind(provenance.task_id)
        .bind(provenance.repo_identity.as_deref().unwrap_or_default())
        .bind(
            provenance
                .execution_workspace
                .as_deref()
                .unwrap_or_default(),
        )
        .fetch_optional(executor)
        .await?;
    row.as_ref().map(provenance_from_row).transpose()
}

/// Lists tasks with work-product discovery still waiting for a worker. This
/// is the durable handoff used by the production maintenance loop.
pub async fn list_pending_execution_discovery_tasks(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    limit: i32,
) -> anyhow::Result<Vec<(Uuid, Uuid)>> {
    let rows = sqlx::query(
        r#"SELECT DISTINCT workspace_id, task_id
FROM agent_task_execution_provenance
WHERE discovery_status = 'pending'
   OR (discovery_status = 'in_progress' AND updated_at < now() - interval '5 minutes')
ORDER BY workspace_id, task_id
LIMIT $1"#,
    )
    .bind(limit)
    .fetch_all(executor)
    .await?;
    rows.iter()
        .map(|row| Ok((row.try_get(0)?, row.try_get(1)?)))
        .collect()
}

/// Serializes exact-head discovery for one workspace/repository/branch. The
/// caller holds this transaction-scoped lock through the provider lookup and
/// the provenance/relation writes.
pub async fn lock_branch_discovery(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    repo_identity: &str,
    head_branch: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"SELECT pg_advisory_xact_lock(
    hashtextextended($1::text || ':' || $2 || ':' || $3, 0)
)"#,
    )
    .bind(workspace_id)
    .bind(repo_identity)
    .bind(head_branch)
    .execute(executor)
    .await?;
    Ok(())
}

/// Returns every other potentially overlapping execution using the same
/// workspace-scoped repository identity and exact branch. An unfinished row
/// represents a currently active/shared checkout; a finished row matters only
/// when its terminal SHA is identical to the current execution. A later reuse
/// of the branch with a different terminal head is therefore not blocked by
/// unrelated historical provenance.
pub async fn list_other_branch_executions(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    repo_identity: &str,
    head_branch: &str,
    head_sha: &str,
    exclude_task_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskExecutionProvenance>> {
    let query = format!(
        r#"SELECT {PROVENANCE_COLUMNS}
FROM agent_task_execution_provenance p
WHERE p.workspace_id = $1
  AND p.repo_identity = $2
  AND p.head_branch = $3
  AND p.task_id <> $4
  AND (p.finished_at IS NULL OR p.head_sha = $5)
ORDER BY p.updated_at DESC, p.task_id DESC"#
    );
    let rows = sqlx::query(&query)
        .bind(workspace_id)
        .bind(repo_identity)
        .bind(head_branch)
        .bind(exclude_task_id)
        .bind(head_sha)
        .fetch_all(executor)
        .await?;
    rows.iter().map(provenance_from_row).collect()
}

/// Holds the claimed provenance row while a worker performs the provider
/// lookup and relation write. A stale worker cannot be reclaimed between this
/// lock and its final lease-checked update.
pub async fn lock_execution_discovery_lease(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    provenance: &AgentTaskExecutionProvenance,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        r#"SELECT task_id
FROM agent_task_execution_provenance
WHERE workspace_id = $1
  AND task_id = $2
  AND repo_identity = $3
  AND execution_workspace = $4
  AND discovery_status = 'in_progress'
  AND discovery_lease_id = $5
FOR UPDATE"#,
    )
    .bind(provenance.workspace_id)
    .bind(provenance.task_id)
    .bind(provenance.repo_identity.as_deref().unwrap_or_default())
    .bind(
        provenance
            .execution_workspace
            .as_deref()
            .unwrap_or_default(),
    )
    .bind(provenance.discovery_lease_id)
    .fetch_optional(executor)
    .await?;
    Ok(row.is_some())
}

pub async fn detach_work_product_relations(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    work_product_id: Uuid,
    issue_id: Uuid,
    detached_by_type: &str,
    detached_by_id: Uuid,
    detached_task_id: Option<Uuid>,
    detached_run_id: Option<Uuid>,
    only_task_id: Option<Uuid>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE work_product_relation
SET detached_at = now(),
    detached_by_type = $4,
    detached_by_id = $5,
    detached_task_id = $6,
    detached_run_id = $7
WHERE workspace_id = $1
  AND work_product_id = $2
  AND issue_id = $3
  AND detached_at IS NULL
  AND ($8::uuid IS NULL OR (
      task_id = $8
      AND attached_by_type = 'agent'
      AND relation_source IN ('task_explicit', 'execution_branch_discovery')
  ))"#,
    )
    .bind(workspace_id)
    .bind(work_product_id)
    .bind(issue_id)
    .bind(detached_by_type)
    .bind(detached_by_id)
    .bind(detached_task_id)
    .bind(detached_run_id)
    .bind(only_task_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_work_products_for_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query("DELETE FROM work_product WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_work_product_relations_for_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query("DELETE FROM work_product_relation WHERE issue_id = $1")
        .bind(issue_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_work_product_relations_for_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query("DELETE FROM work_product_relation WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

/// Stable idempotency key for one explicit attach execution. The task/run
/// values are server-derived; the request cannot choose them.
pub fn relation_key(issue_id: Option<Uuid>, task_id: Option<Uuid>, run_id: Option<Uuid>) -> String {
    format!(
        "issue:{}:task:{}:run:{}",
        issue_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "none".into()),
        task_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "manual".into()),
        run_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "none".into()),
    )
}

pub fn external_identity_for_github(repo_owner: &str, repo_name: &str, number: i32) -> String {
    format!(
        "{}/{}#{number}",
        repo_owner.trim().to_ascii_lowercase(),
        repo_name.trim().to_ascii_lowercase()
    )
}

pub fn external_identity_for_vcs(
    connection_id: Uuid,
    repo_owner: &str,
    repo_name: &str,
    number: i32,
) -> String {
    format!("connection:{connection_id}/{repo_owner}/{repo_name}#{number}")
}

/// Kept as a small pure validator so the handler/API and future non-PR
/// clients share the exact allow-list without parsing human-readable issue
/// identifiers.
pub fn valid_kind(kind: &str) -> bool {
    matches!(
        kind,
        "pull_request" | "branch" | "commit" | "preview" | "artifact" | "document"
    )
}

pub fn valid_provider(provider: &str) -> bool {
    !provider.trim().is_empty()
        && provider.len() <= 64
        && provider
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b' ')
}

pub fn valid_external_identity(identity: &str) -> bool {
    let value = identity.trim();
    !value.is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control)
}
