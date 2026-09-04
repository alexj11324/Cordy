-- =====================
-- VCS Connection (Forgejo / Gitea / GitLab)
-- =====================

-- name: ListVCSConnectionsByWorkspace :many
SELECT * FROM vcs_connection
WHERE workspace_id = $1
ORDER BY created_at ASC;

-- name: GetVCSConnectionByID :one
SELECT * FROM vcs_connection
WHERE id = $1;

-- name: UpsertVCSConnection :one
-- Reconnecting the same instance rotates the stored token/secret, provider,
-- and identity in place rather than creating a duplicate row.
INSERT INTO vcs_connection (
    workspace_id, provider, instance_url, account_login,
    access_token_encrypted, webhook_secret_encrypted, connected_by_id
) VALUES (
    $1, $2, $3, $4, $5, $6, sqlc.narg('connected_by_id')
)
ON CONFLICT (workspace_id, instance_url) DO UPDATE SET
    provider                 = EXCLUDED.provider,
    account_login            = EXCLUDED.account_login,
    access_token_encrypted   = EXCLUDED.access_token_encrypted,
    webhook_secret_encrypted = EXCLUDED.webhook_secret_encrypted,
    connected_by_id          = EXCLUDED.connected_by_id,
    updated_at               = now()
RETURNING *;

-- name: DeleteVCSConnection :exec
-- These tables carry no FKs, so the cascade that once removed the connection's
-- mirrored PRs, their issue links, and CI statuses is gone (migration 213). Do
-- that cleanup explicitly here, in one statement so it commits or rolls back
-- atomically with the connection row. The target CTE also scopes every child
-- delete to a connection that actually belongs to the workspace, so a wrong
-- workspace_id is a no-op rather than deleting another tenant's child rows.
WITH target AS (
    SELECT vcs_connection.id FROM vcs_connection WHERE vcs_connection.id = $1 AND vcs_connection.workspace_id = $2
),
target_prs AS (
    SELECT vcs_pull_request.id FROM vcs_pull_request
    WHERE vcs_pull_request.connection_id IN (SELECT target.id FROM target)
),
target_products AS (
    SELECT work_product.id FROM work_product
    WHERE work_product.provider_record_type = 'vcs_pull_request'
      AND work_product.provider_record_id IN (SELECT target_prs.id FROM target_prs)
),
cleared_relations AS (
    DELETE FROM work_product_relation
    WHERE work_product_id IN (SELECT target_products.id FROM target_products)
),
cleared_products AS (
    DELETE FROM work_product
    WHERE id IN (SELECT target_products.id FROM target_products)
),
cleared_statuses AS (
    DELETE FROM vcs_commit_status WHERE connection_id IN (SELECT target.id FROM target)
),
cleared_prs AS (
    DELETE FROM vcs_pull_request WHERE connection_id IN (SELECT target.id FROM target)
)
DELETE FROM vcs_connection WHERE vcs_connection.id = $1 AND vcs_connection.workspace_id = $2;

-- name: RotateVCSConnectionWebhookSecret :one
UPDATE vcs_connection
SET webhook_secret_encrypted = $3,
    updated_at = now()
WHERE id = $1 AND workspace_id = $2
RETURNING *;

-- =====================
-- VCS Pull Request
-- =====================

-- name: UpsertVCSPullRequest :one
-- pr_updated_at guards against an out-of-order webhook redelivery regressing
-- the PR state, mirroring the updated_at guard on UpsertVCSCommitStatus. Each
-- mutable column is applied only when the incoming event is at least as new as
-- the stored row; a stale event keeps the existing values. The row is still
-- touched (and thus RETURNED) either way, so callers always get the current PR
-- — the webhook needs pr.id to link the issue even on a stale redelivery.
INSERT INTO vcs_pull_request (
    workspace_id, connection_id, provider, repo_owner, repo_name, pr_number,
    title, state, html_url, branch, author_login, author_avatar_url,
    merged_at, closed_at, pr_created_at, pr_updated_at,
    additions, deletions, changed_files, head_sha
) VALUES (
    $1, $2, $3, $4, $5, $6,
    $7, $8, $9, sqlc.narg('branch'), sqlc.narg('author_login'), sqlc.narg('author_avatar_url'),
    sqlc.narg('merged_at'), sqlc.narg('closed_at'), $10, $11,
    $12, $13, $14, $15
)
ON CONFLICT (connection_id, repo_owner, repo_name, pr_number) DO UPDATE SET
    workspace_id      = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.workspace_id      ELSE vcs_pull_request.workspace_id      END,
    provider          = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.provider          ELSE vcs_pull_request.provider          END,
    title             = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.title             ELSE vcs_pull_request.title             END,
    state             = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.state             ELSE vcs_pull_request.state             END,
    html_url          = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.html_url          ELSE vcs_pull_request.html_url          END,
    branch            = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.branch            ELSE vcs_pull_request.branch            END,
    author_login      = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.author_login      ELSE vcs_pull_request.author_login      END,
    author_avatar_url = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.author_avatar_url ELSE vcs_pull_request.author_avatar_url END,
    merged_at         = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.merged_at         ELSE vcs_pull_request.merged_at         END,
    closed_at         = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.closed_at         ELSE vcs_pull_request.closed_at         END,
    pr_updated_at     = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.pr_updated_at     ELSE vcs_pull_request.pr_updated_at     END,
    additions         = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.additions         ELSE vcs_pull_request.additions         END,
    deletions         = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.deletions         ELSE vcs_pull_request.deletions         END,
    changed_files     = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.changed_files     ELSE vcs_pull_request.changed_files     END,
    head_sha          = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.head_sha          ELSE vcs_pull_request.head_sha          END,
    updated_at        = now()
RETURNING *;

-- name: GetVCSPullRequestForWorkProduct :one
WITH checks AS (
    SELECT
        pr.id AS pr_id,
        COUNT(*)::bigint AS total,
        SUM(CASE WHEN status.state = 'failed' THEN 1 ELSE 0 END)::bigint AS failed,
        SUM(CASE WHEN status.state = 'passed' THEN 1 ELSE 0 END)::bigint AS passed,
        SUM(CASE WHEN status.state = 'pending' THEN 1 ELSE 0 END)::bigint AS pending
    FROM vcs_pull_request pr
    JOIN vcs_commit_status status
      ON status.connection_id = pr.connection_id
     AND status.sha = pr.head_sha
     AND pr.head_sha <> ''
    WHERE pr.id = $1
    GROUP BY pr.id
)
SELECT
    pr.*,
    COALESCE(checks.total, 0)::bigint AS checks_total,
    COALESCE(checks.passed, 0)::bigint AS checks_passed,
    COALESCE(checks.failed, 0)::bigint AS checks_failed,
    COALESCE(checks.pending, 0)::bigint AS checks_pending
FROM vcs_pull_request pr
LEFT JOIN checks ON checks.pr_id = pr.id
WHERE pr.id = $1;

-- name: GetIssueCombinedPullRequestCloseAggregate :one
-- Cross-provider close gate. An issue can carry PRs from GitHub AND a
-- self-hosted VCS provider at the same time, so auto-advance has to see BOTH
-- table pairs. Reading only one (as the per-provider aggregates do) lets a
-- merged close-intent PR/MR on one provider advance an issue that still has an
-- open PR on the other — either webhook is blind to the other's in-flight work.
-- Sum the in-flight (open/draft) and merged-with-close-intent counts across
-- both provider mirrors, joined through the one Work Product relation table.
-- provider_reference relations are excluded on both sides, so a bare body
-- mention neither counts as in-flight nor gates advance.
WITH combined AS (
    SELECT pr.state AS state, relation.close_intent AS close_intent
    FROM github_pull_request pr
    JOIN work_product product
      ON product.provider_record_type = 'github_pull_request'
     AND product.provider_record_id = pr.id
    JOIN work_product_relation relation ON relation.work_product_id = product.id
    WHERE relation.issue_id = $1
      AND relation.detached_at IS NULL
      AND relation.relation_source IN ('manual_explicit', 'task_explicit', 'execution_branch_discovery', 'provider_discovery')
    UNION ALL
    SELECT pr.state AS state, relation.close_intent AS close_intent
    FROM vcs_pull_request pr
    JOIN work_product product
      ON product.provider_record_type = 'vcs_pull_request'
     AND product.provider_record_id = pr.id
    JOIN work_product_relation relation ON relation.work_product_id = product.id
    WHERE relation.issue_id = $1
      AND relation.detached_at IS NULL
      AND relation.relation_source IN ('manual_explicit', 'task_explicit', 'execution_branch_discovery', 'provider_discovery')
)
SELECT
    COALESCE(SUM(CASE WHEN state IN ('open', 'draft') THEN 1 ELSE 0 END), 0)::bigint AS open_count,
    COALESCE(SUM(CASE WHEN state = 'merged' AND close_intent THEN 1 ELSE 0 END), 0)::bigint AS merged_with_close_intent_count
FROM combined;

-- =====================
-- VCS commit status (CI)
-- =====================

-- name: UpsertVCSCommitStatus :exec
-- One row per (connection, sha, context); a redelivery or state transition
-- overwrites in place. updated_at guards against an older event overwriting a
-- newer one for the same context. state is normalized (passed/failed/pending).
INSERT INTO vcs_commit_status (
    connection_id, sha, context, state, target_url, description, updated_at
) VALUES (
    $1, $2, $3, $4, sqlc.narg('target_url'), sqlc.narg('description'), $5
)
ON CONFLICT (connection_id, sha, context) DO UPDATE SET
    state       = EXCLUDED.state,
    target_url  = EXCLUDED.target_url,
    description = EXCLUDED.description,
    updated_at  = EXCLUDED.updated_at
WHERE EXCLUDED.updated_at >= vcs_commit_status.updated_at;

-- name: ListIssueIDsForVCSPRHead :many
-- Issues linked to any PR whose head sha matches the given status, so a
-- commit-status event can fan out a PR-card refresh to the right issues.
SELECT DISTINCT relation.issue_id
FROM vcs_pull_request pr
JOIN work_product product
  ON product.provider_record_type = 'vcs_pull_request'
 AND product.provider_record_id = pr.id
JOIN work_product_relation relation ON relation.work_product_id = product.id
WHERE pr.connection_id = $1 AND pr.head_sha = $2 AND pr.head_sha <> ''
  AND relation.issue_id IS NOT NULL
  AND relation.detached_at IS NULL
  AND relation.relation_source IN ('manual_explicit', 'task_explicit', 'execution_branch_discovery', 'provider_discovery');

-- =====================
-- Issue ↔ VCS PR link
-- =====================

-- name: LinkIssueToVCSPullRequest :exec
-- Keep the existing identifier-based self-hosted VCS auto-link contract while
-- storing its relation in the unified Work Product model.
WITH product AS (
    INSERT INTO work_product (
        workspace_id, kind, provider, external_identity, external_url,
        provider_record_type, provider_record_id
    )
    SELECT
        pr.workspace_id,
        'pull_request',
        pr.provider,
        pr.connection_id::text || ':' || pr.repo_owner || '/' || pr.repo_name || '#' || pr.pr_number::text,
        pr.html_url,
        'vcs_pull_request',
        pr.id
    FROM vcs_pull_request pr
    WHERE pr.id = sqlc.arg('pull_request_id')
    ON CONFLICT (workspace_id, provider, external_identity) DO UPDATE SET
        external_url = EXCLUDED.external_url,
        provider_record_type = EXCLUDED.provider_record_type,
        provider_record_id = EXCLUDED.provider_record_id,
        updated_at = now()
    RETURNING id, workspace_id
)
INSERT INTO work_product_relation (
    workspace_id, work_product_id, issue_id, relation_key, relation_source,
    attached_by_type, attached_by_id, close_intent
)
SELECT
    product.workspace_id,
    product.id,
    sqlc.arg('issue_id'),
    'provider:vcs_pull_request:' || sqlc.arg('pull_request_id')::uuid::text
        || ':issue:' || sqlc.arg('issue_id')::uuid::text,
    CASE WHEN sqlc.arg('mention_only')::boolean
        THEN 'provider_reference' ELSE 'provider_discovery' END,
    'system',
    NULL,
    sqlc.arg('close_intent')
FROM product
ON CONFLICT (work_product_id, relation_key) WHERE detached_at IS NULL DO UPDATE SET
    close_intent = CASE
        WHEN sqlc.arg('preserve_close_intent') THEN work_product_relation.close_intent
        ELSE EXCLUDED.close_intent
    END,
    relation_source = CASE
        WHEN sqlc.arg('preserve_close_intent') THEN work_product_relation.relation_source
        ELSE EXCLUDED.relation_source
    END;
