//! Typed SQL queries for issue records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChildIssueProgressRow {
    pub parent_issue_id: Option<Uuid>,
    pub total: i64,
    pub done: i64,
}

pub async fn child_issue_progress(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<ChildIssueProgressRow>> {
    let rows = sqlx::query(
        r#"SELECT parent_issue_id,
       COUNT(*)::bigint AS total,
       COUNT(*) FILTER (WHERE issue_effective_status(workspace_id, status) IN ('done', 'cancelled'))::bigint AS done
FROM issue
WHERE workspace_id = $1
  AND parent_issue_id IS NOT NULL
GROUP BY parent_issue_id"#
    )
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ChildIssueProgressRow {
            parent_issue_id: row.try_get(0)?,
            total: row.try_get(1)?,
            done: row.try_get(2)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CountCreatedIssueExecutorsRow {
    pub executor_type: Option<String>,
    pub executor_id: Option<Uuid>,
    pub frequency: i64,
}

pub async fn count_created_issue_executors(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    creator_id: Uuid,
) -> anyhow::Result<Vec<CountCreatedIssueExecutorsRow>> {
    let rows = sqlx::query(
        r#"SELECT
  executor_type,
  executor_id,
  COUNT(*)::bigint as frequency
FROM issue
WHERE workspace_id = $1
  AND creator_id = $2
  AND creator_type = 'member'
  AND executor_type IS NOT NULL
  AND executor_id IS NOT NULL
GROUP BY executor_type, executor_id"#,
    )
    .bind(workspace_id)
    .bind(creator_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(CountCreatedIssueExecutorsRow {
            executor_type: row.try_get(0)?,
            executor_id: row.try_get(1)?,
            frequency: row.try_get(2)?,
        });
    }
    Ok(out)
}

pub async fn count_issues(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    status: Option<&str>,
    priority: Option<&str>,
    executor_id: Option<Uuid>,
    executor_ids: Vec<Uuid>,
    creator_id: Uuid,
    project_id: Uuid,
    scheduled: Option<bool>,
    metadata_filter: &serde_json::Value,
    involves_user_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT count(*) FROM issue i
WHERE i.workspace_id = $1
  AND ($2::text IS NULL OR i.status = $2)
  AND ($3::text IS NULL OR i.priority = $3)
  AND ($4::uuid IS NULL OR i.executor_id = $4)
  AND ($5::uuid[] IS NULL OR i.executor_id = ANY($5::uuid[]))
  AND ($6::uuid IS NULL OR i.creator_id = $6)
  AND ($7::uuid IS NULL OR i.project_id = $7)
  AND ($8::bool IS NULL OR (i.start_date IS NOT NULL OR i.due_date IS NOT NULL))
  AND ($9::jsonb IS NULL OR i.metadata @> $9::jsonb)
  AND (
    $10::uuid IS NULL
    OR (i.executor_type = 'agent' AND i.executor_id IN (
          SELECT a.id FROM agent a
           WHERE a.workspace_id = $1
             AND a.owner_id     = $10::uuid
    ))
    OR (i.executor_type = 'team' AND i.executor_id IN (
          SELECT sm.team_id
            FROM team_member sm
            JOIN team s ON s.id = sm.team_id
           WHERE s.workspace_id = $1
             AND sm.member_type = 'member'
             AND sm.member_id   = $10::uuid
          UNION
          SELECT s.id
            FROM team s
            JOIN agent a ON a.id = s.leader_id
           WHERE s.workspace_id = $1
             AND a.workspace_id = $1
             AND a.owner_id     = $10::uuid
          UNION
          SELECT sm.team_id
            FROM team_member sm
            JOIN team s ON s.id = sm.team_id
            JOIN agent a ON a.id = sm.member_id
           WHERE s.workspace_id = $1
             AND sm.member_type = 'agent'
             AND a.workspace_id = $1
             AND a.owner_id     = $10::uuid
    ))
  )"#,
    )
    .bind(workspace_id)
    .bind(status)
    .bind(priority)
    .bind(executor_id)
    .bind(executor_ids)
    .bind(creator_id)
    .bind(project_id)
    .bind(scheduled)
    .bind(metadata_filter)
    .bind(involves_user_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn create_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    title: &str,
    description: Option<&str>,
    status: &str,
    priority: &str,
    owner_type: Option<&str>,
    owner_id: Option<Uuid>,
    executor_type: Option<&str>,
    executor_id: Option<Uuid>,
    reviewer_type: Option<&str>,
    reviewer_id: Option<Uuid>,
    creator_type: &str,
    creator_id: Uuid,
    parent_issue_id: Option<Uuid>,
    position: f64,
    start_date: Option<chrono::NaiveDate>,
    due_date: Option<chrono::NaiveDate>,
    number: i32,
    project_id: Option<Uuid>,
    stage: Option<i32>,
    id: Uuid,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"INSERT INTO issue (
    workspace_id, title, description, status, priority,
    executor_type, executor_id, reviewer_type, reviewer_id, creator_type, creator_id,
    parent_issue_id, position, start_date, due_date, number, project_id,
    stage, last_activity_at, id, owner_type, owner_id
) VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
    $16, $17, $18, now(), COALESCE($19::uuid, gen_random_uuid()), $20, $21
) RETURNING id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id"#
    )
        .bind(workspace_id)
        .bind(title)
        .bind(description)
        .bind(status)
        .bind(priority)
        .bind(executor_type)
        .bind(executor_id)
        .bind(reviewer_type)
        .bind(reviewer_id)
        .bind(creator_type)
        .bind(creator_id)
        .bind(parent_issue_id)
        .bind(position)
        .bind(start_date)
        .bind(due_date)
        .bind(number)
        .bind(project_id)
        .bind(stage)
        .bind(id)
        .bind(owner_type)
        .bind(owner_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        executor_type: row.try_get(6)?,
        executor_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
        reviewer_type: row.try_get("reviewer_type")?,
        reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
    }))
}

pub async fn create_issue_with_origin(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    title: &str,
    description: Option<&str>,
    status: &str,
    priority: &str,
    owner_type: Option<&str>,
    owner_id: Option<Uuid>,
    executor_type: Option<&str>,
    executor_id: Option<Uuid>,
    reviewer_type: Option<&str>,
    reviewer_id: Option<Uuid>,
    creator_type: &str,
    creator_id: Uuid,
    parent_issue_id: Option<Uuid>,
    position: f64,
    start_date: Option<chrono::NaiveDate>,
    due_date: Option<chrono::NaiveDate>,
    number: i32,
    project_id: Option<Uuid>,
    origin_type: Option<&str>,
    origin_id: Option<Uuid>,
    stage: Option<i32>,
    id: Uuid,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"INSERT INTO issue (
    workspace_id, title, description, status, priority,
    executor_type, executor_id, reviewer_type, reviewer_id, creator_type, creator_id,
    parent_issue_id, position, start_date, due_date, number, project_id,
    origin_type, origin_id, stage, last_activity_at, id, owner_type, owner_id
) VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
    $16, $17, $18, $19, $20, now(), COALESCE($21::uuid, gen_random_uuid()), $22, $23
) RETURNING id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id"#
    )
        .bind(workspace_id)
        .bind(title)
        .bind(description)
        .bind(status)
        .bind(priority)
        .bind(executor_type)
        .bind(executor_id)
        .bind(reviewer_type)
        .bind(reviewer_id)
        .bind(creator_type)
        .bind(creator_id)
        .bind(parent_issue_id)
        .bind(position)
        .bind(start_date)
        .bind(due_date)
        .bind(number)
        .bind(project_id)
        .bind(origin_type)
        .bind(origin_id)
        .bind(stage)
        .bind(id)
        .bind(owner_type)
        .bind(owner_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        executor_type: row.try_get(6)?,
        executor_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
        reviewer_type: row.try_get("reviewer_type")?,
        reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
    }))
}

pub async fn delete_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH target AS (
    SELECT issue.id FROM issue WHERE issue.id = $1 AND issue.workspace_id = $2
),
cleared_work_product_relations AS (
    DELETE FROM work_product_relation
    WHERE issue_id IN (SELECT target.id FROM target)
),
cleared_coordination_assignments AS (
    DELETE FROM agent_coordination_assignment
    WHERE issue_id IN (SELECT target.id FROM target)
),
cleared_coordination_outbox AS (
    DELETE FROM agent_coordination_outbox
    WHERE issue_id IN (SELECT target.id FROM target)
),
affected_dependency_graph_plans AS (
    SELECT plan.id
    FROM dependency_graph_plan plan
    WHERE plan.workspace_id = $2
      AND plan.parent_issue_id IN (SELECT target.id FROM target)
    UNION
    SELECT node.plan_id
    FROM dependency_graph_node node
    WHERE node.workspace_id = $2
      AND node.issue_id IN (SELECT target.id FROM target)
    UNION
    SELECT edge.plan_id
    FROM dependency_graph_edge edge
    WHERE edge.workspace_id = $2
      AND (edge.from_issue_id IN (SELECT target.id FROM target)
           OR edge.to_issue_id IN (SELECT target.id FROM target))
),
cleared_dependency_graph_issue_created_outbox AS (
    DELETE FROM dependency_graph_issue_created_outbox
    WHERE workspace_id = $2
      AND plan_id IN (SELECT id FROM affected_dependency_graph_plans)
),
cleared_dependency_graph_edges AS (
    DELETE FROM dependency_graph_edge
    WHERE workspace_id = $2
      AND plan_id IN (SELECT id FROM affected_dependency_graph_plans)
),
cleared_dependency_graph_nodes AS (
    DELETE FROM dependency_graph_node
    WHERE workspace_id = $2
      AND plan_id IN (SELECT id FROM affected_dependency_graph_plans)
),
cleared_dependency_graph_plans AS (
    DELETE FROM dependency_graph_plan
    WHERE workspace_id = $2
      AND id IN (SELECT id FROM affected_dependency_graph_plans)
)
DELETE FROM issue WHERE issue.id IN (SELECT target.id FROM target)"#,
    )
    .bind(id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_issue_metadata_key(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    key: &str,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"UPDATE issue SET
    metadata = metadata - $1::text,
    revision = revision + CASE WHEN metadata ? $1::text THEN 1 ELSE 0 END,
    last_activity_at = CASE
        WHEN metadata ? $1::text
        THEN GREATEST(COALESCE(last_activity_at, updated_at), now())
        ELSE last_activity_at
    END,
    updated_at = now()
WHERE id = $2 AND workspace_id = $3
RETURNING id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id"#
    )
        .bind(key)
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        executor_type: row.try_get(6)?,
        executor_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
        reviewer_type: row.try_get("reviewer_type")?,
        reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
    }))
}

pub async fn find_active_duplicate_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    parent_issue_id: Option<Uuid>,
    normalized_title: &str,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id FROM issue
WHERE workspace_id = $1
  AND issue_effective_status(workspace_id, status) NOT IN ('done', 'cancelled')
  AND project_id IS NOT DISTINCT FROM $2::uuid
  AND parent_issue_id IS NOT DISTINCT FROM $3::uuid
  AND lower(btrim(regexp_replace(
        translate(
          title,
          U&'\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000',
          repeat(' ', 19)
        ),
        '[[:space:]]+', ' ', 'g'
      ))) = $4
ORDER BY created_at ASC
LIMIT 1"#
    )
        .bind(workspace_id)
        .bind(project_id)
        .bind(parent_issue_id)
        .bind(normalized_title)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        executor_type: row.try_get(6)?,
        executor_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
        reviewer_type: row.try_get("reviewer_type")?,
        reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
    }))
}

pub async fn find_recent_automation_duplicate_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    origin_id: Uuid,
    project_id: Option<Uuid>,
    normalized_title: &str,
    created_after: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"SELECT i.id, i.workspace_id, i.title, i.description, i.status, i.priority, i.executor_type, i.executor_id, i.creator_type, i.creator_id, i.parent_issue_id, i.acceptance_criteria, i.context_refs, i.position, i.due_date, i.created_at, i.updated_at, i.number, i.project_id, i.origin_type, i.origin_id, i.first_executed_at, i.start_date, i.metadata, i.stage, i.properties, i.revision, i.last_activity_at, i.reviewer_type, i.reviewer_id, i.owner_type, i.owner_id FROM issue i
WHERE i.workspace_id = $1
  AND issue_effective_status(i.workspace_id, i.status) NOT IN ('done', 'cancelled')
  AND i.origin_type = 'automation'
  AND i.origin_id = $2
  AND i.project_id IS NOT DISTINCT FROM $3::uuid
  AND lower(btrim(regexp_replace(
        translate(
          i.title,
          U&'\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000',
          repeat(' ', 19)
        ),
        '[[:space:]]+', ' ', 'g'
      ))) = $4
  AND i.created_at >= $5::timestamptz
  AND EXISTS (
    SELECT 1
    FROM automation_run r
    WHERE r.issue_id = i.id
      AND r.automation_id = i.origin_id
      AND r.status IN ('issue_created', 'running', 'completed')
  )
ORDER BY i.created_at ASC
LIMIT 1"#
    )
        .bind(workspace_id)
        .bind(origin_id)
        .bind(project_id)
        .bind(normalized_title)
        .bind(created_after)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        executor_type: row.try_get(6)?,
        executor_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
        reviewer_type: row.try_get("reviewer_type")?,
        reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
    }))
}

pub async fn get_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id FROM issue
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        executor_type: row.try_get(6)?,
        executor_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
        reviewer_type: row.try_get("reviewer_type")?,
        reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
    }))
}

pub async fn get_issue_by_number(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    number: i32,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id FROM issue
WHERE workspace_id = $1 AND number = $2"#
    )
        .bind(workspace_id)
        .bind(number)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        executor_type: row.try_get(6)?,
        executor_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
        reviewer_type: row.try_get("reviewer_type")?,
        reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
    }))
}

pub async fn get_issue_by_origin(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    origin_type: Option<&str>,
    origin_id: Uuid,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id FROM issue
WHERE workspace_id = $1
  AND origin_type = $2
  AND origin_id = $3
LIMIT 1"#
    )
        .bind(workspace_id)
        .bind(origin_type)
        .bind(origin_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        executor_type: row.try_get(6)?,
        executor_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
        reviewer_type: row.try_get("reviewer_type")?,
        reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetIssueGCStatusRow {
    pub workspace_id: Option<Uuid>,
    pub status: String,
    pub updated_at: Option<DateTime<Utc>>,
}

pub async fn get_issue_gc_status(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<GetIssueGCStatusRow>> {
    let row = sqlx::query(
        r#"SELECT workspace_id, status, updated_at
FROM issue
WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetIssueGCStatusRow {
        workspace_id: row.try_get(0)?,
        status: row.try_get(1)?,
        updated_at: row.try_get(2)?,
    }))
}

pub async fn get_issue_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id FROM issue
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        executor_type: row.try_get(6)?,
        executor_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
        reviewer_type: row.try_get("reviewer_type")?,
        reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
    }))
}

/// Loads a bounded set of issues in one workspace-scoped query. Dependency
/// graph snapshots use this instead of issuing one detail query per node.
pub async fn list_issues_in_workspace_by_ids(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    issue_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<Issue>> {
    Ok(sqlx::query_as::<_, Issue>(
        "SELECT id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id FROM issue WHERE workspace_id = $1 AND id = ANY($2::uuid[])",
    )
    .bind(workspace_id)
    .bind(issue_ids)
    .fetch_all(executor)
    .await?)
}

/// Locks the non-terminal issues owned by a dependency-graph plan before the
/// plan or its queue work is reconciled. The effective-status predicate keeps
/// custom Done/Cancelled statuses out of graph lifecycle cancellation.
pub async fn lock_nonterminal_dependency_graph_children(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    issue_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<Issue>> {
    Ok(sqlx::query_as::<_, Issue>(
        "SELECT id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id FROM issue WHERE workspace_id = $1 AND id = ANY($2::uuid[]) AND issue_effective_status(workspace_id, status) NOT IN ('done', 'cancelled') ORDER BY id FOR UPDATE",
    )
    .bind(workspace_id)
    .bind(issue_ids)
    .fetch_all(executor)
    .await?)
}

/// Moves only non-terminal dependency-graph children to the standard
/// Cancelled status while preserving revision/activity audit semantics.
pub async fn cancel_dependency_graph_children(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    issue_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<Issue>> {
    Ok(sqlx::query_as::<_, Issue>(
        r#"UPDATE issue SET
    status = 'cancelled',
    revision = revision + 1,
    last_activity_at = GREATEST(COALESCE(last_activity_at, updated_at), now()),
    updated_at = now()
WHERE workspace_id = $1
  AND id = ANY($2::uuid[])
  AND issue_effective_status(workspace_id, status) NOT IN ('done', 'cancelled')
RETURNING *"#,
    )
    .bind(workspace_id)
    .bind(issue_ids)
    .fetch_all(executor)
    .await?)
}

pub async fn list_child_issues(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    parent_issue_id: Uuid,
) -> anyhow::Result<Vec<Issue>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id FROM issue
WHERE parent_issue_id = $1
ORDER BY number ASC"#
    )
        .bind(parent_issue_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Issue {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            title: row.try_get(2)?,
            description: row.try_get(3)?,
            status: row.try_get(4)?,
            priority: row.try_get(5)?,
            executor_type: row.try_get(6)?,
            executor_id: row.try_get(7)?,
            creator_type: row.try_get(8)?,
            creator_id: row.try_get(9)?,
            parent_issue_id: row.try_get(10)?,
            acceptance_criteria: row.try_get(11)?,
            context_refs: row.try_get(12)?,
            position: row.try_get(13)?,
            due_date: row.try_get(14)?,
            created_at: row.try_get(15)?,
            updated_at: row.try_get(16)?,
            number: row.try_get(17)?,
            project_id: row.try_get(18)?,
            origin_type: row.try_get(19)?,
            origin_id: row.try_get(20)?,
            first_executed_at: row.try_get(21)?,
            start_date: row.try_get(22)?,
            metadata: row.try_get(23)?,
            stage: row.try_get(24)?,
            properties: row.try_get(25)?,
            revision: row.try_get(26)?,
            last_activity_at: row.try_get(27)?,
            reviewer_type: row.try_get("reviewer_type")?,
            reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
        });
    }
    Ok(out)
}

pub async fn list_children_by_parents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    parent_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<Issue>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id FROM issue
WHERE workspace_id = $1
  AND parent_issue_id = ANY($2::uuid[])
ORDER BY parent_issue_id, number ASC"#
    )
        .bind(workspace_id)
        .bind(parent_ids)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Issue {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            title: row.try_get(2)?,
            description: row.try_get(3)?,
            status: row.try_get(4)?,
            priority: row.try_get(5)?,
            executor_type: row.try_get(6)?,
            executor_id: row.try_get(7)?,
            creator_type: row.try_get(8)?,
            creator_id: row.try_get(9)?,
            parent_issue_id: row.try_get(10)?,
            acceptance_criteria: row.try_get(11)?,
            context_refs: row.try_get(12)?,
            position: row.try_get(13)?,
            due_date: row.try_get(14)?,
            created_at: row.try_get(15)?,
            updated_at: row.try_get(16)?,
            number: row.try_get(17)?,
            project_id: row.try_get(18)?,
            origin_type: row.try_get(19)?,
            origin_id: row.try_get(20)?,
            first_executed_at: row.try_get(21)?,
            start_date: row.try_get(22)?,
            metadata: row.try_get(23)?,
            stage: row.try_get(24)?,
            properties: row.try_get(25)?,
            revision: row.try_get(26)?,
            last_activity_at: row.try_get(27)?,
            reviewer_type: row.try_get("reviewer_type")?,
            reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListIssueGCStatusesRow {
    pub id: Option<Uuid>,
    pub status: String,
    pub updated_at: Option<DateTime<Utc>>,
}

pub async fn list_issue_gc_statuses(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    issue_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<ListIssueGCStatusesRow>> {
    let rows = sqlx::query(
        r#"SELECT id, status, updated_at
FROM issue
WHERE workspace_id = $1
  AND id = ANY($2::uuid[])"#,
    )
    .bind(workspace_id)
    .bind(issue_ids)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListIssueGCStatusesRow {
            id: row.try_get(0)?,
            status: row.try_get(1)?,
            updated_at: row.try_get(2)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListIssuesRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub executor_type: Option<String>,
    pub executor_id: Option<Uuid>,
    pub creator_type: String,
    pub creator_id: Option<Uuid>,
    pub parent_issue_id: Option<Uuid>,
    pub position: f64,
    pub start_date: Option<chrono::NaiveDate>,
    pub due_date: Option<chrono::NaiveDate>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub number: i32,
    pub project_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
    pub stage: Option<i32>,
    pub properties: Option<serde_json::Value>,
    pub revision: i64,
}

pub async fn list_issues(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    limit: i32,
    offset: i32,
    status: Option<&str>,
    priority: Option<&str>,
    executor_id: Uuid,
    executor_ids: Vec<Uuid>,
    creator_id: Uuid,
    project_id: Uuid,
    scheduled: Option<bool>,
    metadata_filter: &serde_json::Value,
    involves_user_id: Uuid,
) -> anyhow::Result<Vec<ListIssuesRow>> {
    let rows = sqlx::query(
        r#"SELECT i.id, i.workspace_id, i.title, i.description, i.status, i.priority,
       i.executor_type, i.executor_id, i.creator_type, i.creator_id,
       i.parent_issue_id, i.position, i.start_date, i.due_date, i.created_at, i.updated_at, i.last_activity_at, i.number, i.project_id, i.metadata, i.stage, i.properties,
       i.revision
FROM issue i
WHERE i.workspace_id = $1
  AND ($4::text IS NULL OR i.status = $4)
  AND ($5::text IS NULL OR i.priority = $5)
  AND ($6::uuid IS NULL OR i.executor_id = $6)
  AND ($7::uuid[] IS NULL OR i.executor_id = ANY($7::uuid[]))
  AND ($8::uuid IS NULL OR i.creator_id = $8)
  AND ($9::uuid IS NULL OR i.project_id = $9)
  AND ($10::bool IS NULL OR (i.start_date IS NOT NULL OR i.due_date IS NOT NULL))
  AND ($11::jsonb IS NULL OR i.metadata @> $11::jsonb)
  AND (
    $12::uuid IS NULL
    -- (1) executor is an agent owned by the user
    OR (i.executor_type = 'agent' AND i.executor_id IN (
          SELECT a.id FROM agent a
           WHERE a.workspace_id = $1
             AND a.owner_id     = $12::uuid
    ))
    -- (2)(3)(4) executor is a team related to the user — three relations
    OR (i.executor_type = 'team' AND i.executor_id IN (
          -- (2) the user is a human member of the team
          SELECT sm.team_id
            FROM team_member sm
            JOIN team s ON s.id = sm.team_id
           WHERE s.workspace_id = $1
             AND sm.member_type = 'member'
             AND sm.member_id   = $12::uuid
          UNION
          -- (3) the team's canonical leader is an agent owned by the user.
          -- We read team.leader_id directly rather than relying on a
          -- team_member row, because the leader copy in team_member is
          -- best-effort (see team.go AddTeamMember error handling).
          SELECT s.id
            FROM team s
            JOIN agent a ON a.id = s.leader_id
           WHERE s.workspace_id = $1
             AND a.workspace_id = $1
             AND a.owner_id     = $12::uuid
          UNION
          -- (4) the team has an agent member owned by the user
          SELECT sm.team_id
            FROM team_member sm
            JOIN team s ON s.id = sm.team_id
            JOIN agent a ON a.id = sm.member_id
           WHERE s.workspace_id = $1
             AND sm.member_type = 'agent'
             AND a.workspace_id = $1
             AND a.owner_id     = $12::uuid
    ))
  )
ORDER BY i.position ASC, i.created_at DESC
LIMIT $2 OFFSET $3"#
    )
        .bind(workspace_id)
        .bind(limit)
        .bind(offset)
        .bind(status)
        .bind(priority)
        .bind(executor_id)
        .bind(executor_ids)
        .bind(creator_id)
        .bind(project_id)
        .bind(scheduled)
        .bind(metadata_filter)
        .bind(involves_user_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListIssuesRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            title: row.try_get(2)?,
            description: row.try_get(3)?,
            status: row.try_get(4)?,
            priority: row.try_get(5)?,
            executor_type: row.try_get(6)?,
            executor_id: row.try_get(7)?,
            creator_type: row.try_get(8)?,
            creator_id: row.try_get(9)?,
            parent_issue_id: row.try_get(10)?,
            position: row.try_get(11)?,
            start_date: row.try_get(12)?,
            due_date: row.try_get(13)?,
            created_at: row.try_get(14)?,
            updated_at: row.try_get(15)?,
            last_activity_at: row.try_get(16)?,
            number: row.try_get(17)?,
            project_id: row.try_get(18)?,
            metadata: row.try_get(19)?,
            stage: row.try_get(20)?,
            properties: row.try_get(21)?,
            revision: row.try_get(22)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListOpenIssuesRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub executor_type: Option<String>,
    pub executor_id: Option<Uuid>,
    pub creator_type: String,
    pub creator_id: Option<Uuid>,
    pub parent_issue_id: Option<Uuid>,
    pub position: f64,
    pub start_date: Option<chrono::NaiveDate>,
    pub due_date: Option<chrono::NaiveDate>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub number: i32,
    pub project_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
    pub stage: Option<i32>,
    pub properties: Option<serde_json::Value>,
    pub revision: i64,
}

pub async fn list_open_issues(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    priority: Option<&str>,
    executor_id: Uuid,
    executor_ids: Vec<Uuid>,
    creator_id: Uuid,
    project_id: Uuid,
    metadata_filter: &serde_json::Value,
    properties_filter: &serde_json::Value,
    involves_user_id: Uuid,
) -> anyhow::Result<Vec<ListOpenIssuesRow>> {
    let rows = sqlx::query(
        r#"SELECT i.id, i.workspace_id, i.title, i.description, i.status, i.priority,
       i.executor_type, i.executor_id, i.creator_type, i.creator_id,
       i.parent_issue_id, i.position, i.start_date, i.due_date, i.created_at, i.updated_at, i.last_activity_at, i.number, i.project_id, i.metadata, i.stage, i.properties,
       i.revision
FROM issue i
WHERE i.workspace_id = $1
  AND issue_effective_status(i.workspace_id, i.status) NOT IN ('done', 'cancelled')
  AND ($2::text IS NULL OR i.priority = $2)
  AND ($3::uuid IS NULL OR i.executor_id = $3)
  AND ($4::uuid[] IS NULL OR i.executor_id = ANY($4::uuid[]))
  AND ($5::uuid IS NULL OR i.creator_id = $5)
  AND ($6::uuid IS NULL OR i.project_id = $6)
  AND ($7::jsonb IS NULL OR i.metadata @> $7::jsonb)
  -- properties_filter is a jsonb array of groups, each group an array of
  -- containment patterns (built by parsePropertiesFilterParam): the issue
  -- must match at least one pattern from EVERY group (AND of ORs). A pattern
  -- of the shape {"__none__": "<definitionId>"} is the "no value" marker and
  -- matches when the issue's properties are missing that key. The correlated
  -- form skips the GIN index, which is fine here: open_only is an
  -- unpaginated workspace scan already narrowed by status.
  AND (
    $8::jsonb IS NULL
    OR NOT EXISTS (
      SELECT 1
      FROM jsonb_array_elements($8::jsonb) AS pf(alternatives)
      WHERE NOT EXISTS (
        SELECT 1
        FROM jsonb_array_elements(pf.alternatives) AS alt(pattern)
        WHERE (alt.pattern ? '__none__' AND NOT (i.properties ? (alt.pattern ->> '__none__')))
           OR (NOT (alt.pattern ? '__none__') AND i.properties @> alt.pattern)
      )
    )
  )
  AND (
    $9::uuid IS NULL
    OR (i.executor_type = 'agent' AND i.executor_id IN (
          SELECT a.id FROM agent a
           WHERE a.workspace_id = $1
             AND a.owner_id     = $9::uuid
    ))
    OR (i.executor_type = 'team' AND i.executor_id IN (
          SELECT sm.team_id
            FROM team_member sm
            JOIN team s ON s.id = sm.team_id
           WHERE s.workspace_id = $1
             AND sm.member_type = 'member'
             AND sm.member_id   = $9::uuid
          UNION
          SELECT s.id
            FROM team s
            JOIN agent a ON a.id = s.leader_id
           WHERE s.workspace_id = $1
             AND a.workspace_id = $1
             AND a.owner_id     = $9::uuid
          UNION
          SELECT sm.team_id
            FROM team_member sm
            JOIN team s ON s.id = sm.team_id
            JOIN agent a ON a.id = sm.member_id
           WHERE s.workspace_id = $1
             AND sm.member_type = 'agent'
             AND a.workspace_id = $1
             AND a.owner_id     = $9::uuid
    ))
  )
ORDER BY i.position ASC, i.created_at DESC"#
    )
        .bind(workspace_id)
        .bind(priority)
        .bind(executor_id)
        .bind(executor_ids)
        .bind(creator_id)
        .bind(project_id)
        .bind(metadata_filter)
        .bind(properties_filter)
        .bind(involves_user_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListOpenIssuesRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            title: row.try_get(2)?,
            description: row.try_get(3)?,
            status: row.try_get(4)?,
            priority: row.try_get(5)?,
            executor_type: row.try_get(6)?,
            executor_id: row.try_get(7)?,
            creator_type: row.try_get(8)?,
            creator_id: row.try_get(9)?,
            parent_issue_id: row.try_get(10)?,
            position: row.try_get(11)?,
            start_date: row.try_get(12)?,
            due_date: row.try_get(13)?,
            created_at: row.try_get(14)?,
            updated_at: row.try_get(15)?,
            last_activity_at: row.try_get(16)?,
            number: row.try_get(17)?,
            project_id: row.try_get(18)?,
            metadata: row.try_get(19)?,
            stage: row.try_get(20)?,
            properties: row.try_get(21)?,
            revision: row.try_get(22)?,
        });
    }
    Ok(out)
}

pub async fn lock_issue_duplicate_key(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    dollar_1: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))"#)
        .bind(dollar_1)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn lock_issue_for_channel_media_bind(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"SELECT id FROM issue
WHERE id = $1 AND workspace_id = $2
FOR KEY SHARE"#,
    )
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn lock_issue_for_delete(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"SELECT id FROM issue
WHERE id = $1 AND workspace_id = $2
FOR UPDATE"#,
    )
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn lock_issue_for_description_update(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id FROM issue
WHERE id = $1 AND workspace_id = $2
FOR UPDATE"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        executor_type: row.try_get(6)?,
        executor_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
        reviewer_type: row.try_get("reviewer_type")?,
        reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MarkIssueFirstExecutedRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub creator_type: String,
    pub creator_id: Option<Uuid>,
    pub first_executed_at: Option<DateTime<Utc>>,
}

pub async fn mark_issue_first_executed(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<MarkIssueFirstExecutedRow>> {
    let row = sqlx::query(
        r#"UPDATE issue
SET first_executed_at = now()
WHERE id = $1 AND first_executed_at IS NULL
RETURNING id, workspace_id, creator_type, creator_id, first_executed_at"#,
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(MarkIssueFirstExecutedRow {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        creator_type: row.try_get(2)?,
        creator_id: row.try_get(3)?,
        first_executed_at: row.try_get(4)?,
    }))
}

pub async fn materialize_issue_channel_media_markdown(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    base_description: Option<&str>,
    description: &str,
    markdown: Option<&str>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"UPDATE issue
SET description = CASE
        WHEN $1::text IS NOT NULL
             AND COALESCE(description, '') = $1::text
            THEN $2::text
        WHEN description IS NULL OR description = '' THEN $3
        ELSE description || E'\n\n' || $3
    END,
    revision = revision + 1,
    updated_at = now()
WHERE id = $4
  AND workspace_id = $5
RETURNING id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id"#
    )
        .bind(base_description)
        .bind(description)
        .bind(markdown)
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        executor_type: row.try_get(6)?,
        executor_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
        reviewer_type: row.try_get("reviewer_type")?,
        reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
    }))
}

pub async fn set_issue_metadata_key(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    key: &str,
    value: &serde_json::Value,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"UPDATE issue SET
    metadata = jsonb_set(metadata, ARRAY[$1::text], $2::jsonb),
    revision = revision + CASE WHEN metadata -> $1::text IS DISTINCT FROM $2::jsonb THEN 1 ELSE 0 END,
    last_activity_at = CASE
        WHEN metadata -> $1::text IS DISTINCT FROM $2::jsonb
        THEN GREATEST(COALESCE(last_activity_at, updated_at), now())
        ELSE last_activity_at
    END,
    updated_at = now()
WHERE id = $3 AND workspace_id = $4
RETURNING id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id"#
    )
        .bind(key)
        .bind(value)
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        executor_type: row.try_get(6)?,
        executor_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
        reviewer_type: row.try_get("reviewer_type")?,
        reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
    }))
}

pub async fn update_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    title: Option<&str>,
    description: Option<&str>,
    status: Option<&str>,
    priority: Option<&str>,
    executor_type: Option<&str>,
    executor_id: Uuid,
    position: Option<f64>,
    start_date: Option<chrono::NaiveDate>,
    due_date: Option<chrono::NaiveDate>,
    parent_issue_id: Uuid,
    project_id: Uuid,
    stage: Option<i32>,
    expected_revision: Option<i64>,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"WITH candidate AS (
    SELECT
        i.id, i.workspace_id, i.title, i.description, i.status, i.priority, i.executor_type, i.executor_id, i.creator_type, i.creator_id, i.parent_issue_id, i.acceptance_criteria, i.context_refs, i.position, i.due_date, i.created_at, i.updated_at, i.number, i.project_id, i.origin_type, i.origin_id, i.first_executed_at, i.start_date, i.metadata, i.stage, i.properties, i.revision, i.last_activity_at, i.reviewer_type, i.reviewer_id, i.owner_type, i.owner_id,
        COALESCE($2::text, i.title) AS next_title,
        COALESCE($3::text, i.description) AS next_description,
        COALESCE($4::text, i.status) AS next_status,
        COALESCE($5::text, i.priority) AS next_priority,
        $6::text AS next_executor_type,
        $7::uuid AS next_executor_id,
        COALESCE($8::double precision, i.position) AS next_position,
        $9::date AS next_start_date,
        $10::date AS next_due_date,
        $11::uuid AS next_parent_issue_id,
        $12::uuid AS next_project_id,
        $13::integer AS next_stage
    FROM issue AS i
    WHERE i.id = $1
      AND ($14::bigint IS NULL OR i.revision = $14::bigint)
), changed AS (
    SELECT
        candidate.id, candidate.workspace_id, candidate.title, candidate.description, candidate.status, candidate.priority, candidate.executor_type, candidate.executor_id, candidate.creator_type, candidate.creator_id, candidate.parent_issue_id, candidate.acceptance_criteria, candidate.context_refs, candidate.position, candidate.due_date, candidate.created_at, candidate.updated_at, candidate.number, candidate.project_id, candidate.origin_type, candidate.origin_id, candidate.first_executed_at, candidate.start_date, candidate.metadata, candidate.stage, candidate.properties, candidate.revision, candidate.last_activity_at, candidate.reviewer_type, candidate.reviewer_id, candidate.owner_type, candidate.owner_id, candidate.next_title, candidate.next_description, candidate.next_status, candidate.next_priority, candidate.next_executor_type, candidate.next_executor_id, candidate.next_position, candidate.next_start_date, candidate.next_due_date, candidate.next_parent_issue_id, candidate.next_project_id, candidate.next_stage,
        ROW(
            title, description, status, priority, executor_type, executor_id,
            position, start_date, due_date, parent_issue_id, project_id, stage
        ) IS DISTINCT FROM ROW(
            next_title, next_description, next_status, next_priority,
            next_executor_type, next_executor_id, next_position, next_start_date,
            next_due_date, next_parent_issue_id, next_project_id, next_stage
        ) AS did_change,
        ROW(
            title, description, status, priority, executor_type, executor_id,
            start_date, due_date, parent_issue_id, project_id, stage
        ) IS DISTINCT FROM ROW(
            next_title, next_description, next_status, next_priority,
            next_executor_type, next_executor_id, next_start_date, next_due_date,
            next_parent_issue_id, next_project_id, next_stage
        ) AS did_activity
    FROM candidate
)
UPDATE issue AS i SET
    title = changed.next_title,
    description = changed.next_description,
    status = changed.next_status,
    priority = changed.next_priority,
    executor_type = changed.next_executor_type,
    executor_id = changed.next_executor_id,
    position = changed.next_position,
    start_date = changed.next_start_date,
    due_date = changed.next_due_date,
    parent_issue_id = changed.next_parent_issue_id,
    project_id = changed.next_project_id,
    stage = changed.next_stage,
    revision = i.revision + changed.did_change::integer,
    last_activity_at = CASE WHEN changed.did_activity
        THEN GREATEST(COALESCE(i.last_activity_at, i.updated_at), now())
        ELSE i.last_activity_at
    END,
    updated_at = CASE WHEN changed.did_change THEN now() ELSE i.updated_at END
FROM changed
WHERE i.id = changed.id
RETURNING i.id, i.workspace_id, i.title, i.description, i.status, i.priority, i.executor_type, i.executor_id, i.creator_type, i.creator_id, i.parent_issue_id, i.acceptance_criteria, i.context_refs, i.position, i.due_date, i.created_at, i.updated_at, i.number, i.project_id, i.origin_type, i.origin_id, i.first_executed_at, i.start_date, i.metadata, i.stage, i.properties, i.revision, i.last_activity_at, i.reviewer_type, i.reviewer_id, i.owner_type, i.owner_id"#
    )
        .bind(id)
        .bind(title)
        .bind(description)
        .bind(status)
        .bind(priority)
        .bind(executor_type)
        .bind(executor_id)
        .bind(position)
        .bind(start_date)
        .bind(due_date)
        .bind(parent_issue_id)
        .bind(project_id)
        .bind(stage)
        .bind(expected_revision)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        executor_type: row.try_get(6)?,
        executor_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
        reviewer_type: row.try_get("reviewer_type")?,
        reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
    }))
}

pub async fn update_issue_status(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    status: &str,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"UPDATE issue SET
    status = $2,
    revision = revision + CASE WHEN status IS DISTINCT FROM $2 THEN 1 ELSE 0 END,
    last_activity_at = CASE WHEN status IS DISTINCT FROM $2
        THEN GREATEST(COALESCE(last_activity_at, updated_at), now())
        ELSE last_activity_at
    END,
    updated_at = now()
WHERE id = $1 AND workspace_id = $3
RETURNING id, workspace_id, title, description, status, priority, executor_type, executor_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id, owner_type, owner_id"#
    )
        .bind(id)
        .bind(status)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        executor_type: row.try_get(6)?,
        executor_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
        reviewer_type: row.try_get("reviewer_type")?,
        reviewer_id: row.try_get("reviewer_id")?,
        owner_type: row.try_get("owner_type")?,
        owner_id: row.try_get("owner_id")?,
    }))
}
