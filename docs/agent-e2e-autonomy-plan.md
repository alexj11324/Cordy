# Agent end-to-end autonomy plan

## Audit result

The current implementation is not an end-to-end autonomous loop.

- Entering `in_review` is fail-closed: the issue must have an assignee and the
  assignee must change. This prevents an ownerless active issue, but it does
  not select a reviewer or guarantee that the selected owner is a runnable
  Agent.
- A status-only transition from `in_review` to `in_progress` does not enqueue a
  run. The previous implementer is not persisted, so a human reviewer can
  return work to an issue that no Agent owns or wakes.
- GitHub webhook and snapshot code mirrors PR and check state only. It does not
  enqueue a repair task on failed checks or requested changes, and it has no
  GitHub review/comment/merge write path. CI therefore runs when GitHub
  triggers it, but no supervisor keeps repairing a PR until merge.
- The current GitHub repository has no branch protection/ruleset, automatic
  merge is disabled, and the open PRs have no reviewer requests or assignees.
  These are deployment facts, not guarantees supplied by the application.

As audited on 2026-08-29, the repository had 613 remote branches and six
open PRs (#609, #612, #616, #617, #618, and #619); all six had zero GitHub
reviewer requests and zero PR assignees. PR #616 targets another feature
branch rather than `main`. PR #617's current head was receiving a normal CI
run, but no workflow or application path assigned a failed check to an Agent
or attempted a merge. The registered one-shot/remediation workflows from
earlier branches were not present in `main` and did not provide a durable
supervisor.

## Observable success and invariants

The implementation is complete only when a canary issue can demonstrate this
single event sequence with durable evidence:

1. An implementation Agent is assigned and receives a task.
2. The issue enters `in_review` with a distinct reviewer selected by policy.
3. A requested-change or failed-CI event moves the issue to `in_progress`,
   restores or selects an implementation owner, and creates exactly one
   pending repair task for the relevant PR head.
4. A new PR head causes a fresh CI/review evaluation; duplicate webhook
   delivery does not create duplicate work.
5. Only an explicitly enabled policy may merge, and the merge request is
   guarded by the current PR head SHA, passing checks, approval, and a clean
   merge state.
6. Every unavailable owner, exhausted retry budget, stale head, permission
   failure, and merge conflict is persisted as an actionable human handoff;
   no event is silently dropped.

## Delivery phases

### Phase 1 — review-return ownership (implemented in this branch)

Persist the owner that handed an issue to review. On a status-only return to
`in_progress`, restore that owner and use a dedicated `review_return` enqueue
source. Keep the existing different-reviewer guard and fail closed if the
stored owner is no longer valid.

### Phase 2 — durable coordinator handoff (implemented in this follow-up)

Write a task-completed or review-returned event and its pending assignment to
the PostgreSQL outbox in the same transaction as the business transition. A
leased Coordinator consumes the outbox, selects an eligible reviewer or
restores the implementation owner, records the decision and audit activity,
and dispatches exactly one follow-up task. Missed notifications and process
restarts recover through polling and lease expiry; the outbox remains the
source of truth. This phase does not merge PRs automatically.

### Phase 3 — reviewer policy and availability

Add an explicit workspace/issue reviewer policy. It must choose a different,
authorized Agent (or deliberately route to a human), record the decision, and
surface a blocked handoff when no eligible reviewer exists. Do not choose an
arbitrary Agent as an implicit fallback.

### Phase 4 — durable PR remediation supervisor

Consume PR, review, check-suite, check-run, and workflow-run events. Correlate
them to the linked issue and current head SHA, then transition/requeue through
the same issue workflow used by HTTP writes. Store a durable per-head attempt
and lease record so retries survive process restarts, are bounded, and are
idempotent.

### Phase 5 — provider write actions and guarded merge

Extend the provider boundary with least-privilege operations for requesting a
review, posting a remediation handoff, and merging. Require an explicit
auto-merge policy, current-head compare-and-swap, passing required checks,
approved review, and a clean merge state. Never merge an unrelated or stale
head.

### Phase 6 — canary and operational proof

Run a disposable repository canary covering review return, requested changes,
failed CI, duplicate delivery, agent disconnect/reconnect, stale head, merge
conflict, and successful merge. Then enable the policy for one workspace and
measure handoff latency, duplicate rate, repair success, retry exhaustion, and
human escalations before widening rollout.

## Current boundary

The earlier Phase 1 PR persists the review-return owner. This follow-up adds
the durable coordinator handoff and reviewer selection for local issue-task
transitions. Phases 3–5 still require an explicit reviewer-policy surface,
durable PR remediation events, provider write/merge authority, and a guarded
merge policy; they must not be inferred from the current read-only GitHub
snapshot integration. The audit evidence and acceptance gates above remain
the source of truth for the next implementation increments.
