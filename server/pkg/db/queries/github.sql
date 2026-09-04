-- =====================
-- GitHub Installation
-- =====================

-- name: ListGitHubInstallationsByWorkspace :many
SELECT * FROM github_installation
WHERE workspace_id = $1
ORDER BY created_at ASC;

-- name: ListGitHubInstallationsByInstallationID :many
-- One installation_id can be bound to several workspaces; webhook routing lists
-- every binding and fans the event out to each bound workspace. Ordered oldest
-- first so processing is deterministic and replay-stable.
SELECT * FROM github_installation
WHERE installation_id = $1
ORDER BY created_at ASC, id ASC;

-- name: GetGitHubInstallationByID :one
SELECT * FROM github_installation
WHERE id = $1;

-- name: CreateGitHubInstallation :one
INSERT INTO github_installation (
    workspace_id, installation_id, account_login, account_type, account_avatar_url, connected_by_id
) VALUES (
    $1, $2, $3, $4, sqlc.narg('account_avatar_url'), sqlc.narg('connected_by_id')
)
ON CONFLICT (workspace_id, installation_id) DO UPDATE SET
    account_login = EXCLUDED.account_login,
    account_type = EXCLUDED.account_type,
    account_avatar_url = EXCLUDED.account_avatar_url,
    connected_by_id = EXCLUDED.connected_by_id,
    updated_at = now()
RETURNING *;

-- name: DeleteGitHubInstallation :exec
DELETE FROM github_installation WHERE id = $1 AND workspace_id = $2;

-- name: DeleteGitHubInstallationByInstallationID :many
-- GitHub-side uninstall/suspend removes trust in the installation entirely, so
-- drop every workspace binding. Returns one row per deleted binding so the
-- handler can broadcast to each affected workspace.
DELETE FROM github_installation WHERE installation_id = $1
RETURNING id, workspace_id;

-- name: UpdateGitHubInstallationAccountByInstallationID :many
-- Refresh the GitHub account display metadata across every workspace binding of
-- an installation (fired by installation.created/new_permissions_accepted/
-- unsuspend). Leaves workspace_id and connected_by_id untouched.
UPDATE github_installation
SET account_login = $2,
    account_type = $3,
    account_avatar_url = sqlc.narg('account_avatar_url'),
    updated_at = now()
WHERE installation_id = $1
RETURNING *;

-- name: UpsertPendingGitHubInstallation :one
INSERT INTO github_pending_installation (
    installation_id, account_login, account_type, account_avatar_url
) VALUES (
    $1, $2, $3, sqlc.narg('account_avatar_url')
)
ON CONFLICT (installation_id) DO UPDATE SET
    account_login = EXCLUDED.account_login,
    account_type = EXCLUDED.account_type,
    account_avatar_url = EXCLUDED.account_avatar_url,
    updated_at = now()
RETURNING *;

-- name: DeletePendingGitHubInstallation :exec
DELETE FROM github_pending_installation WHERE installation_id = $1;

-- name: GetPendingGitHubInstallation :one
SELECT * FROM github_pending_installation WHERE installation_id = $1
;

-- =====================
-- GitHub Pull Request
-- =====================

-- name: UpsertGitHubPullRequest :one
-- mergeable_state has three-state semantics on UPDATE:
--   1. clear_mergeable_state=true → write NULL (state-changing actions like
--      opened/synchronize/reopened/edited(base) invalidate the prior verdict).
--   2. clear_mergeable_state=false, mergeable_state non-null → write the value.
--   3. clear_mergeable_state=false, mergeable_state null → preserve existing
--      column. Metadata events (labeled/assigned/etc.) ship payloads without
--      mergeability, and silently clobbering a known clean/dirty would lose
--      information that GitHub only re-computes lazily.
-- INSERT path always writes the incoming value (NULL acceptable for a new row).
INSERT INTO github_pull_request (
    workspace_id, installation_id, repo_owner, repo_name, pr_number,
    title, state, html_url, branch, author_login, author_avatar_url,
    merged_at, closed_at, pr_created_at, pr_updated_at,
    head_sha, mergeable_state,
    additions, deletions, changed_files
) VALUES (
    $1, $2, $3, $4, $5,
    $6, $7, $8, sqlc.narg('branch'), sqlc.narg('author_login'), sqlc.narg('author_avatar_url'),
    sqlc.narg('merged_at'), sqlc.narg('closed_at'), $9, $10,
    $11, sqlc.narg('mergeable_state'),
    $12, $13, $14
)
ON CONFLICT (workspace_id, repo_owner, repo_name, pr_number) DO UPDATE SET
    installation_id = EXCLUDED.installation_id,
    title = EXCLUDED.title,
    state = EXCLUDED.state,
    html_url = EXCLUDED.html_url,
    branch = EXCLUDED.branch,
    author_login = EXCLUDED.author_login,
    author_avatar_url = EXCLUDED.author_avatar_url,
    merged_at = EXCLUDED.merged_at,
    closed_at = EXCLUDED.closed_at,
    pr_updated_at = EXCLUDED.pr_updated_at,
    head_sha = EXCLUDED.head_sha,
    mergeable_state = CASE
        WHEN COALESCE(sqlc.narg('clear_mergeable_state')::boolean, FALSE) THEN NULL
        WHEN EXCLUDED.mergeable_state IS NOT NULL THEN EXCLUDED.mergeable_state
        ELSE github_pull_request.mergeable_state
    END,
    additions     = EXCLUDED.additions,
    deletions     = EXCLUDED.deletions,
    changed_files = EXCLUDED.changed_files,
    updated_at = now()
RETURNING *;

-- name: GetGitHubPullRequest :one
SELECT * FROM github_pull_request
WHERE workspace_id = $1 AND repo_owner = $2 AND repo_name = $3 AND pr_number = $4;

-- name: GetGitHubPullRequestForWorkProduct :one
WITH checks AS (
    SELECT
        cr.pr_id,
        COUNT(*)::bigint AS total,
        SUM(CASE WHEN cr.status = 'completed' AND cr.conclusion IN
                ('failure','cancelled','timed_out','action_required','startup_failure','stale','error')
            THEN 1 ELSE 0 END)::bigint AS failed,
        SUM(CASE WHEN cr.status = 'completed' AND cr.conclusion IN
                ('success','neutral','skipped')
            THEN 1 ELSE 0 END)::bigint AS passed,
        SUM(CASE WHEN cr.status <> 'completed' OR cr.conclusion IS NULL
            THEN 1 ELSE 0 END)::bigint AS running,
        COALESCE(
            array_agg(cr.name) FILTER (WHERE cr.status = 'completed' AND cr.conclusion IN
                ('failure','cancelled','timed_out','action_required','startup_failure','stale','error')),
            '{}'
        )::text[] AS failed_names
    FROM github_pull_request_check_run cr
    JOIN github_pull_request pr ON pr.id = cr.pr_id
    WHERE pr.id = $1
      AND cr.head_sha = pr.snapshot_head_sha
      AND pr.snapshot_head_sha <> ''
    GROUP BY cr.pr_id
)
SELECT
    pr.id, pr.workspace_id, pr.installation_id, pr.repo_owner, pr.repo_name,
    pr.pr_number, pr.title, pr.state, pr.html_url, pr.branch, pr.author_login,
    pr.author_avatar_url, pr.merged_at, pr.closed_at, pr.pr_created_at,
    pr.pr_updated_at, pr.head_sha, pr.mergeable_state,
    pr.additions, pr.deletions, pr.changed_files,
    pr.api_mergeable, pr.api_merge_state_status, pr.checks_rollup_state,
    pr.snapshot_head_sha, pr.snapshot_fetched_at,
    pr.created_at, pr.updated_at,
    COALESCE(c.total, 0)::bigint AS checks_total,
    COALESCE(c.passed, 0)::bigint AS checks_passed,
    COALESCE(c.failed, 0)::bigint AS checks_failed,
    COALESCE(c.running, 0)::bigint AS checks_running,
    COALESCE(c.failed_names, '{}')::text[] AS failed_check_names
FROM github_pull_request pr
LEFT JOIN checks c ON c.pr_id = pr.id
WHERE pr.id = $1;

-- name: GetIssueReviewHeadSha :one
-- Returns the head SHA of the commit currently "under review" for an issue:
-- the most-recently-updated linked PR that still has an open/draft state and a
-- non-empty head_sha. Used by the reviewer-loop dedup (TEN-356) so a pending
-- review task pinned to an old head does not satisfy a request after HEAD
-- advanced. Prefers in-flight PRs (open/draft) over merged/closed ones so a
-- stale merged sibling can't shadow the live review target; falls back to the
-- newest linked PR with a head_sha when none are open. Returns no rows (empty
-- string) when the issue has no linked PR — callers treat that as "no SHA key"
-- and dedup on (issue_id, agent_id) alone, preserving pre-TEN-356 behavior.
--
-- Spans both GitHub and self-hosted VCS PRs: a self-hosted PR pushing a new
-- commit must move the dedup head SHA the same way a GitHub PR does, otherwise
-- a fresh review round could be merged away against a stale key.
-- provider_reference relations are excluded on both arms, matching the
-- Work Product list and close-aggregate queries: a body-only mention is hidden
-- from the list and the close gate, so it must not win this ORDER BY and become
-- the review dedup head SHA either, masking the real working PR's SHA.
SELECT head_sha FROM (
    SELECT pr.head_sha AS head_sha, pr.state AS state, pr.pr_updated_at AS pr_updated_at
    FROM github_pull_request pr
    JOIN work_product product
      ON product.provider_record_type = 'github_pull_request'
     AND product.provider_record_id = pr.id
    JOIN work_product_relation relation ON relation.work_product_id = product.id
    WHERE relation.issue_id = $1 AND pr.head_sha <> ''
      AND relation.detached_at IS NULL
      AND relation.relation_source IN ('manual_explicit', 'task_explicit', 'execution_branch_discovery', 'provider_discovery')
    UNION ALL
    SELECT pr.head_sha AS head_sha, pr.state AS state, pr.pr_updated_at AS pr_updated_at
    FROM vcs_pull_request pr
    JOIN work_product product
      ON product.provider_record_type = 'vcs_pull_request'
     AND product.provider_record_id = pr.id
    JOIN work_product_relation relation ON relation.work_product_id = product.id
    WHERE relation.issue_id = $1 AND pr.head_sha <> ''
      AND relation.detached_at IS NULL
      AND relation.relation_source IN ('manual_explicit', 'task_explicit', 'execution_branch_discovery', 'provider_discovery')
) combined
ORDER BY (state IN ('open', 'draft')) DESC, pr_updated_at DESC
LIMIT 1;

-- name: ListIssueIDsForPullRequest :many
SELECT DISTINCT relation.issue_id
FROM work_product product
JOIN work_product_relation relation ON relation.work_product_id = product.id
WHERE product.provider_record_type = 'github_pull_request'
  AND product.provider_record_id = $1
  AND relation.issue_id IS NOT NULL
  AND relation.detached_at IS NULL
  AND relation.relation_source IN ('manual_explicit', 'task_explicit', 'execution_branch_discovery', 'provider_discovery');

-- =====================
-- Issue ↔ Pull Request link
-- =====================

-- name: LinkIssueToPullRequest :exec
-- Preserve the existing identifier-based GitHub auto-link contract. The
-- relation is still written to the unified Work Product tables, so the
-- current provider discovery behavior and the new explicit Work Product
-- surface share one durable representation.
WITH product AS (
    INSERT INTO work_product (
        workspace_id, kind, provider, external_identity, external_url,
        provider_record_type, provider_record_id
    )
    SELECT
        pr.workspace_id,
        'pull_request',
        'github',
        pr.repo_owner || '/' || pr.repo_name || '#' || pr.pr_number::text,
        pr.html_url,
        'github_pull_request',
        pr.id
    FROM github_pull_request pr
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
    'provider:github_pull_request:' || sqlc.arg('pull_request_id')::uuid::text
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

-- name: GetIssuePullRequestCloseAggregate :one
-- Aggregates the issue's linked PRs into the two counts that gate
-- auto-advance: how many are still in flight (`open` or `draft`) and how
-- many merged PRs declared explicit closing intent on the link row. The
-- webhook auto-advances the issue when open_count = 0 AND
-- merged_with_close_intent_count > 0. Both the PR state and the link row
-- (with close_intent) are persisted before this query runs, so the result
-- is event-agnostic — a link-only sibling closing after a closing-keyword
-- PR has already merged still resolves the issue.
--
-- provider_reference relations (a PR that merely mentions the issue identifier
-- in its body) are excluded: they are hidden from the issue's Work Product
-- list, so they must not silently gate auto-advance either. An open body-only
-- mention would otherwise keep open_count > 0 and block the issue from
-- advancing while being invisible in the UI. (These relations never carry
-- close_intent, so excluding them does not change
-- merged_with_close_intent_count.)
SELECT
    COALESCE(SUM(CASE WHEN pr.state IN ('open', 'draft') THEN 1 ELSE 0 END), 0)::bigint AS open_count,
    COALESCE(SUM(CASE WHEN pr.state = 'merged' AND relation.close_intent THEN 1 ELSE 0 END), 0)::bigint AS merged_with_close_intent_count
FROM github_pull_request pr
JOIN work_product product
  ON product.provider_record_type = 'github_pull_request'
 AND product.provider_record_id = pr.id
JOIN work_product_relation relation ON relation.work_product_id = product.id
WHERE relation.issue_id = $1
  AND relation.detached_at IS NULL
  AND relation.relation_source IN ('manual_explicit', 'task_explicit', 'execution_branch_discovery', 'provider_discovery');
