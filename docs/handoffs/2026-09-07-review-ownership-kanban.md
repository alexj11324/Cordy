# Review ownership and collapsed board columns

## Acceptance and invariants

Moving an issue into Review must preserve its executor and name a distinct
reviewer. An agent/team reviewer needs a durable dispatch obligation in the
same transaction as the status change. Returning work to implementation or
changing the reviewer retires the previous coordinated review task before the
replacement handoff. Metadata edits must preserve a concurrently changed role.

A successful drop on a collapsed status row expands that column and displays
the moved card. Failed moves restore the source and previous visibility.
Unknown/loading counts must not collapse a column. Exact custom-status filters
must remain effective during a drop. All column panels use the same neutral
gray in light and dark themes; semantic colors remain on status icons.

## Findings and fixes

| Finding | Change |
| --- | --- |
| Explicit transitions stored a reviewer but did not create review work. | Update, batch update, board move, and direct Review creation record a durable reviewer handoff. |
| A chosen agent was also required to hold the automatic team/default reviewer role. | Explicit user selection carries provenance through initial selection and persisted revalidation; automatic selection still requires the role. |
| Reviewer could be removed or made identical to executor while already in Review. | Enforce the role invariant throughout Review, including under the issue lock. |
| Metadata updates could overwrite a concurrent reviewer change. | Refresh untouched nullable fields under the same row lock for every single/batch write. |
| Reviewer-only batch requests were treated as empty mutations. | Include owner and reviewer fields in mutation detection. |
| Collapsed board drop behavior from PR #611 was absent after the mainline migration. | Restore eligible hidden drop targets, hover feedback, successful expansion, and failure rollback. |
| PR #751 removed column surfaces. | Restore rounded neutral gray surfaces without changing card width or adding colored status panels. |

## Workflow assessment and limits

The existing durable outbox, assignment ownership, transaction lock ordering,
and stale-generation fences are appropriate foundations. The fixes connect
explicit status changes to that machinery instead of creating another queue.

Human reviewers remain supported and are responsible through the issue. An
offline or busy agent can retain a pending assignment; assignment is not proof
that a model has started. `suppress_run` deliberately updates ownership without
dispatch, but does not bypass the reviewer invariant. Existing implementation
tasks are not cancelled solely by an issue status change, preserving the
product's documented task-lifecycle rule. A normal task completion without
coordination provenance does not by itself advance the issue through review.

Verification covers focused UI regression tests, TypeScript checks, real
PostgreSQL handler/service tests, and an actual Electron renderer. Electron
checks cover auto-collapsed and manually hidden drops, reload persistence,
failed-move rollback, and light/dark neutral surfaces. Live local HTTP checks
exercise the API and background coordinator with fixture agents; they do not
claim real provider/model execution or autonomous review quality.
