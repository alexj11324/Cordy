# Active issue executor invariant

Migration 585 rejects direct INSERT or UPDATE when an issue in `in_progress`,
`in_review`, or `blocked` lacks an `agent`/`team` executor type and UUID. Custom
statuses inherit the same rule from their workspace catalog. Clearing an active
issue's executor fails; moving it to `todo` and clearing the executor in the same
statement succeeds. PostgreSQL reports SQLSTATE `23514` and constraint name
`issue_active_executor_required`.

This is a database backstop for the existing Go workflow gate. Actor existence,
workspace membership, permissions, runtime eligibility and distinct review
handoff still belong to the application. This migration does not add foreign keys,
start an Agent, or synthesize execution history. An issue's `in_progress` label
is not evidence of a currently running execution; the UI's live border beam is
based on `agent_task_queue.status = 'running'`.

Custom status identity (workspace/key/category) is immutable, matching the
existing update API. Display names, colors and archive state remain editable.
The issue trigger locks a custom catalog row until commit, and refuses unknown
custom keys. New catalog entries cannot make legacy unassigned issues active.

## Existing data and rollout

Run the following read-only preflight before deploying the migration:

```sql
SELECT i.id, i.workspace_id, i.number, i.status
FROM issue i
WHERE issue_effective_status(i.workspace_id, i.status)
      IN ('in_progress', 'in_review', 'blocked')
  AND (i.executor_type IS NULL OR i.executor_id IS NULL
       OR i.executor_type NOT IN ('agent', 'team'));
```

The migration deliberately fails if this query finds rows. Review them and
explicitly assign eligible executors or move them out of active states. Do not
silently assign a real Agent merely to make a migration pass. The down migration
removes the guards without deleting or rewriting tasks.

Test with PostgreSQL via `DATABASE_URL`:

```sh
cd server
go test ./internal/migrations -run '^TestIssueActiveExecutorDatabaseGuard$' -count=1 -v
go test ./internal/issueroles -count=1
```

The migration test creates a uniquely named isolated schema, executes the actual
up/down SQL, checks built-in/custom statuses and workspace boundaries, and drops
only its own schema. Without `DATABASE_URL`, it skips; a skipped run is not proof.

The graph apply endpoint validates blocked-node executors before creating any
plan rows. Missing explicit/default execution returns HTTP 422 with code
`active_executor_required`, instead of leaking the database rejection as HTTP 500.
Independent-root admission remains unchanged; automatic admission and the new
Todo waiting semantics require a separate scheduler change.
