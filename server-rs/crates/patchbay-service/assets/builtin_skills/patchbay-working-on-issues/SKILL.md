---
name: patchbay-working-on-issues
description: "Use when acting on a Patchbay issue beyond what the brief covers: PR linking vs close intent, reading a linked PR's real state, metadata keys, status-change side effects, sub-issue todo vs backlog."
allowed-tools: Bash(patchbay *), Bash(git *), Bash(gh *)
---

# Working on Patchbay issues

Product contracts the runtime brief does not fully encode: PR linking vs close
intent, reading linked-PR state, metadata keys, status side effects, and
sub-issue enqueue behavior.

For building mention links, load `patchbay-mentioning` instead — not this skill.

Every contract below is traced to source in
`references/working-on-issues-source-map.md`.

## Work Products and explicit PR association

A PR is a Work Product. Its association with an Issue belongs to the canonical
Work Product relation created by an authenticated execution context or by an
authorized user explicitly attaching the PR. The PR title, body, branch, and
human-readable issue key are never scanned to create or recover a relation.

When an agent creates a PR, register it before completing the task through the
task-scoped Patchbay credentials. The server derives the task, run, Issue, and
workspace from that authenticated context; do not submit a different task or
Issue as a caller-controlled owner:

```bash
patchbay issue pull-request attach <issue-id> --url https://github.com/<owner>/<repo>/pull/<number>
```

For an execution that did not register its PR, Patchbay may perform one
deterministic post-run lookup using that execution's persisted workspace, repo
identity, and exact head branch. A unique PR in the same repository and
workspace becomes a relation with `execution_branch_discovery` provenance. Zero,
multiple, shared, detached, or default-branch matches stay unassociated or
ambiguous for explicit review. The branch is only an input to this one lookup;
later webhook and poller updates use the durable relation and do not search by
branch again.

Manual, old-agent, and external PRs without an explicit registration remain
unassociated until an authorized user attaches them. An issue key such as
`PB-2759` may help a person search GitHub or name a branch, but it does not prove
identity, authorization, or relation correctness. A PR containing that key must
not appear in the Issue's linked list until it is explicitly attached or safely
associated by the execution-branch discovery above.

Close intent is also explicit. If a PR should advance an Issue when it merges,
pass `--close-intent` during the attach operation; text such as `Closes PB-2759`
is ordinary PR content and has no association or close-intent effect.

### Default for code-changing issue work

When an issue run changes code in a checked-out GitHub repo, the default handoff
is to open or update a PR before posting the final Patchbay issue comment, unless
the user explicitly asked for a local-only change or no PR. This is a default, not
an unconditional command: if no code changed, say no PR is needed; if PR creation
is blocked by auth, failing tests, or missing remote state, report that blocker
instead of pretending the run is complete.

After `gh pr create` (or once you know the PR URL), **immediately attach it to the
Issue** — this is the explicit write-back path. A task-token attach must be
verified against provider metadata from the authorized GitHub App, including the
exact repository and execution branch. A workspace member may use the same URL
form without an App for a manual attach; caller-supplied metadata is never an
ownership proof for a task token:

```bash
patchbay issue pull-request attach <issue-id> --url https://github.com/<owner>/<repo>/pull/<number>
```

An issue key in the PR title, body, or branch is optional human context. It is not
required for association and does not change the attach result. If provider
metadata is unavailable to a task-token call, leave the PR unassociated for an
authorized member to confirm instead of bypassing the ownership check. If the PR should
close the Issue on merge, pass explicit close intent:

```text
PB-2759: fix login redirect        # optional human context
Closes PB-2759                     # ordinary text; no automatic effect
```

The explicit relation answers "which PR is this" at creation or attach time. CI
status, mergeability, and merge events still come from the GitHub App integration;
attaching does not fake any of them. Without an installation, the attached card
shows number + URL until a webhook supplies full metadata. Later webhooks preserve
the relation provenance and only refresh the Work Product snapshot.

In the final issue comment, include the PR URL when a PR exists. If the task did
not produce a PR because no code changed or the user asked not to create one, say
that explicitly.

## Reading a linked PR's real state

When a step depends on PR state, query Patchbay's canonical Work Product relation
and provider snapshot — do not infer it from branch names, PR text, GitHub search,
memory, or task metadata.

```bash
patchbay issue pull-requests <issue-id> --output json
```

Returns `{"pull_requests": [...]}`. Each element exposes:

- `number`, `html_url`, `title`
- `state` — the PR lifecycle as a **single enum**, one of `merged`, `closed`,
  `draft`, `open`. There is no separate `draft` or `merged` boolean in the
  response; the server folds them into `state` (merged wins, then closed, then
  draft, else open).
- `merged_at` — non-null once merged; a second confirmation of `state: merged`.
- `provider` — `github`, `forgejo`, `gitea`, or `gitlab`.
- `mergeable_state` — mirrors GitHub (`clean` / `dirty` surfaced; other values
  round-trip as unknown; retained for compatibility).
- GitHub API snapshot fields: `snapshot_available`, `mergeable`,
  `merge_state_status`, `checks_rollup`, `checks_total`, `checks_passed`,
  `checks_failed`, `checks_running`, `failed_check_names`,
  `snapshot_fetched_at`, and `snapshot_stale`. `snapshot_available == true`
  means the feature is enabled and the snapshot matches the PR's current head.
  Only then does `checks_rollup == null` mean "no checks"; false means the
  snapshot feature is disabled, has not fetched yet, or only has an old head.
- `checks_conclusion` — coarse CI compatibility status: `passed`, `failed`,
  `pending`, or `null`. GitHub derives it from the current API snapshot;
  Forgejo/Gitea/GitLab derive it from webhook commit statuses. Backed by the
  provider-appropriate check counts.

So "is it merged?" is `state == "merged"` (or `merged_at != null`); "is it still
a draft?" is `state == "draft"`; coarse CI status is `checks_conclusion`.

If the command returns no linked PRs after a PR was opened, the PR has not been
explicitly attached and was not uniquely associated by the originating
execution's exact-branch discovery. Attach it yourself when authorized; the
attach operation records the relation and its provenance immediately.

## Metadata: durable custom state

Metadata is a free-form KV bag of durable issue state. Reading metadata is safe.
Writing a metadata key is a state mutation and should be tied to an explicit
task requirement to record that state for later readers or runs. Keys are
whatever your workflow needs — the platform curates no vocabulary; pick short
snake_case names and reuse them consistently within your workspace.

Never store secrets, tokens, or API keys in metadata.
Not metadata: logs or summaries; runtime bookkeeping such as timestamps,
attempt counts, or agent IDs; or other run metadata such as
files touched and investigation notes — those belong in the result comment.

```bash
patchbay issue metadata set <issue-id> --key <key> --value <value>
patchbay issue metadata delete <issue-id> --key <stale-key>
```

`--value` is JSON-parsed by default (bool/number are sniffed); pass `--type
string|number|bool` to force a type.

## Custom properties: typed workflow state

Workspaces may define custom issue properties (Severity, Environment, QA
Status, Reviewer, ...). Properties are the typed, user-visible sibling of
metadata: values are validated against the definition (select options, date
format, http(s) URL, member reference), visible in the issue sidebar, and
addressed by name.

- Read what exists before writing: `patchbay property list` shows the catalog;
  `patchbay issue property list <issue-id>` shows values set on the issue.
- Set values by property name and option name — the CLI translates to ids:

```bash
patchbay issue property set <issue-id> --name Environment --value staging
patchbay issue property set <issue-id> --name Platforms --value "iOS,Android"
patchbay issue property set <issue-id> --name Reviewer --value Bohan
patchbay issue property unset <issue-id> --name Environment
```

- A validation error lists the legal options — fix the value and retry.
- `actor` / `multi_actor` properties (Reviewer, Escalation contact, ...) hold
  workspace members only. `--value` takes a member name, email, UUID, short id,
  or an explicit `member:<uuid>`; `multi_actor` takes a comma-separated list
  (duplicates dropped, order kept, max 20).
- Definitions may include an optional catalog icon for visual identification;
  it does not change the property's type or value validation.
- Agents cannot create or edit property definitions (owner/admin humans only).
  If a needed property does not exist, propose it in a comment instead.
- Property vs metadata: if the value is workflow state a human should see and
  filter by, and a definition exists, prefer the property. Metadata stays the
  free-form bag for durable custom issue state.

## Status changes have server side effects

A status change is not cosmetic — the server enqueues or skips agent work based
on it. These are the contracts, not advice:

- **`backlog` and `todo` are non-running queues.** A graph root is persisted as
  `todo`; a dependent is persisted as `blocked`. Neither state starts an agent.
  The coordinator admits a ready `todo` issue to `in_progress` only after all
  hard dependencies are `done`, the executor has capacity, and its ACP/model is
  available. When the last hard dependency completes, `blocked` becomes `todo`
  and waits for that admission pass.
- **`in_progress` / `in_review`** are agent-managed CLI mutations, not
  `StartTask` / `CompleteTask` side effects. The runtime brief asks agents to
  write the state the issue is in whenever their work changes it — not from
  the trigger type or the run's lifecycle, and not gated on being the
  executor. Writes happen whenever the state changes, mid-turn included: a
  turn that advances the issue's own ask sets `in_progress` as soon as that
  is known, so the board shows the work while it runs; a blocker is recorded
  when it is hit; and the turn must not exit with a stale value — delivered
  the issue's own ask → `in_review`; work continues beyond the turn
  (dispatched sub-issues, partial delivery) → `in_progress`; stuck →
  `blocked`. A turn that produces none of the issue's own deliverable —
  answering a question, consulting on work owned elsewhere — writes nothing
  at any point. The kind of activity never decides this: research, design,
  planning, and review all count as the work exactly when they are what the
  issue asks for (a review-the-PR issue is being worked the moment reviewing
  starts). Questions, discussion, or acknowledgements never move the status.
  Team leaders: dispatching members is not delivery — a dispatch turn
  leaves the parent `in_progress`, and it moves to `in_review` only when a
  later re-trigger confirms the overall goal is met.
- **Every active status needs roles.** `in_progress` and `in_review` require an
  executor; `in_review` additionally requires a reviewer different from it.
  `owner` is the accountable human and is independent from `executor` and
  `reviewer`. `backlog`, `todo`, `done`, and `cancelled` may be unassigned.
  Entering `in_review` is a handoff, not a status-only update: the executor is
  retained, the independent reviewer is persisted, and the server creates a
  reviewer task. If no suitable reviewer is available, keep the issue in its
  current active state and ask a human to choose one.
- **`done`** on a child issue posts a system comment on its parent. If an
  explicitly attached Work Product relation carries close intent, it advances
  the Issue to `done` on merge — PR text alone never sets that intent.
- **`cancelled`** is a terminal, user-driven decision to close the issue. Like
  `done` it enqueues no new agent work, but it does **not** stop tasks already in
  flight — a run in progress keeps going (PB-4465). To stop a running task,
  cancel the task itself.
- **Failed issue-triggered tasks** may roll an issue from `in_progress` back to
  `todo` when no active task / retry remains — that is the main server-owned
  status write on the agent-run path.

## Claim ownership without duplicating a run

Assigning an active issue to an agent normally starts a run. When the work is
already underway and the write only records ownership or progress, pass
`--no-start` on every command in that flow — suppressing the assignment alone
does not suppress a later status update:

```bash
patchbay issue update <issue-id> --executor-id <agent-id> --no-start
patchbay issue status <issue-id> in_progress --no-start
```

Before self-assigning, check the target issue's comment history for an existing
claim and any `## Active sibling runs` block (its `run-messages` commands show
work in flight). The server also suppresses a trusted self-assignment when the
exact target `(issue, agent)` pair already has a non-terminal task, but it
deliberately keeps same-agent handoffs to a fresh issue starting runs: cross-issue
serial chains and triage batches rely on that.

## Dependency-graph sub-issues: `todo` is a queue, `blocked` is gated

Do not create graph children one at a time. Use the planning skill's typed
`dependency-graph apply`, which atomically creates all child issues, parent and
hard-dependency relations, role assignments, and readiness state. Roots start
as `todo`; every non-root starts as `blocked`. The server, not this prompt,
validates cycles, duplicate edges, workspace scope, and output references.

```bash
patchbay issue dependency-graph apply <parent-id> \
  --idempotency-key "<stable-plan-key>" --plan-stdin --output json < plan.json
```

When a prerequisite reaches `done`, the coordinator promotes only dependents
whose complete hard-prerequisite set is done to `todo`. It then performs the
capacity/ACP admission to `in_progress` and creates exactly one executor task.
Never use a manual status update to bypass this gate or to simulate a graph
edge.

### Stages: order sub-issues into barrier groups

`--stage <N>` (N ≥ 1) groups sub-issues under the same parent into ordered
stages. The parent executor is woken **once, when a whole stage finishes** —
i.e. every sub-issue in the lowest unfinished stage has reached a terminal
status (`done`/`cancelled`). A completion that does not close a stage is silent
(no comment, no wake). A sibling set with **no** stages is one implicit stage,
so the parent is woken once when the *last* sub-issue finishes — not on every
child.

Advancement is agent-driven: the server only detects the closed barrier and
wakes the parent executor, who then decides whether to promote the next stage's
`backlog` sub-issues to `todo`.

```bash
# Stage 1 runs now; later stages parked until promoted
patchbay issue create --title "Research A" --parent <id> --executor <agent> --stage 1 --status todo
patchbay issue create --title "Research B" --parent <id> --executor <agent> --stage 1 --status todo
patchbay issue create --title "Build"      --parent <id> --executor <agent> --stage 2 --status backlog
patchbay issue create --title "Ship"       --parent <id> --executor <agent> --stage 3 --status backlog
```

When both Stage 1 sub-issues finish you (the parent executor) are woken with a
"Stage 1 complete" comment. Inspect the layout, then promote the next stage:

```bash
patchbay issue children <parent-id>             # sub-issues grouped by stage
patchbay issue status <stage-2-child-id> todo   # promote when its deps are met
```

`issue children --output json` reports per-stage `done` counts. A workspace may
define custom statuses beyond the 7 built-ins; a custom status counts as done
here when its category is `done` or `cancelled`, which is what `status_category`
on each child carries. Read `status_category` rather than matching `status`
against the built-in names.

Read each sub-issue's description before promoting and only promote items whose
stated dependencies are met; if a description conflicts with the parent's
breakdown, leave it `backlog` and comment to confirm first.

## Incorrect → correct

PR association:

```text
PB-2759: fix login redirect        # human context only; never an association
patchbay issue pull-request attach <issue-id> --url <pr-url>
                                      # explicit, authorized association
```

Serial / phased sub-issues (don't start the whole chain at once):

```bash
# incorrect — all fire immediately, no ordering
patchbay issue create --title "Step 2" --parent <issue-id> --executor <agent> --status todo
patchbay issue create --title "Step 3" --parent <issue-id> --executor <agent> --status todo

# correct — stage them; Stage 1 runs, later stages park and are promoted as
# each stage's barrier closes
patchbay issue create --title "Step 1" --parent <issue-id> --executor <agent> --stage 1 --status todo
patchbay issue create --title "Step 2" --parent <issue-id> --executor <agent> --stage 2 --status backlog
patchbay issue create --title "Step 3" --parent <issue-id> --executor <agent> --stage 3 --status backlog
```

## References

`references/working-on-issues-source-map.md` — source anchors for the canonical
Work Product attach/discovery routes, provider snapshot fields, task admission
and terminal hooks, the backlog enqueue lines, child-done notify, the stage
barrier and its CLI, and the metadata CLI. Re-derive before depending on an exact
line.
