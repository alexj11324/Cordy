---
name: patchbay-teams
description: "Use when creating, inspecting, updating, assigning to, or debugging a Patchbay team, including how leader routing picks who runs."
user-invocable: false
allowed-tools: Bash(patchbay *)
---

# Patchbay Teams

## Quick start

If debugging why a team did or did not run, inspect first:

```bash
patchbay issue get <issue-id> --output json
patchbay team get <team-id> --output json
patchbay team member list <team-id> --output json
patchbay issue comment list <issue-id> --roots-only --summary --output json
patchbay issue comment list <issue-id> --thread <thread-id> --tail 30 --output json
```

The two comment reads are a sequence: scan the roots first, then open the threads that look relevant — mention triggers, failure reasons, and user instructions usually live in the replies, which the roots scan never returns.

If the command shape is unclear, check help instead of guessing:

```bash
patchbay team --help
patchbay team member --help
patchbay issue update --help
patchbay issue comment add --help
```

Do not assign, comment, mention, update, delete, or record team activity just
to test. These can mutate workspace state or trigger agent runs.

## Core model

A Patchbay team is a workspace routing and coordination object.

A team is not an agent. It does not run work by itself. Current behavior:
team-routed work runs through the team's `leader_id` agent.

Important consequences:

- assigning an issue to a team routes to the leader;
- mentioning a team routes to the leader;
- team-assigned automation resolves to the leader;
- team members are not automatically fanned out;
- team `instructions` are leader briefing content, not member prompts.

## CLI

Team commands:

```bash
patchbay team list --output json
patchbay team get <team-id> --output json
patchbay team create --name <name> --leader <agent-name-or-id> --output json
patchbay team update <team-id> --instructions "<leader coordination policy>" --output json
patchbay team delete <team-id>
```

Member commands:

```bash
patchbay team member list <team-id> --output json
patchbay team member add <team-id> --member-id <id> --type agent|member --role <role> --output json
patchbay team member remove <team-id> --member-id <id> --type agent|member
patchbay team member set-role <team-id> --member-id <id> --member-type agent|member --role <role> --output json
```

Team leader evaluation command:

```bash
patchbay team activity <issue-id> action|no_action|failed --reason "<why>" --output json
```

`activity` is a write: it records the leader's evaluation decision on an issue.
Use it only when acting as the team leader after evaluating a trigger.

Which issue it accepts: **the issue your current turn is running on**. The
target issue does NOT need to be assigned to your team — a `@team` mention on
an issue whose executor is an individual agent, or a leader task bound to a child issue,
all record fine. What the server checks is your task row (`is_leader_task` plus
a stamped `team_id`), not the issue's executor. A leader woken by a stage
barrier runs on the PARENT issue, so record against the parent, not the child
you just read; passing an unrelated issue id is rejected and the error names the
issue you should have used.

If the call fails, do not exit silently — the comment prohibition on `no_action`
only applies once the recording succeeded. Post one short comment with the
outcome instead, and only when this turn has not already commented: on the
`action` path your delegation comment is already that record.

Issue/comment commands often needed with teams:

```bash
patchbay issue get <issue-id> --output json
patchbay issue update <issue-id> --help
patchbay issue comment list <issue-id> --roots-only --summary --output json
patchbay issue comment add <issue-id> --help
```

Comment reads stay bounded — the scan-then-expand sequence from the quick
start above — never one unbounded `issue comment list` pull.

Prefer `--output json` for reads. Use `--help` before writes.

## Team fields

- `id` — team UUID.
- `workspace_id` — workspace the team belongs to.
- `name` — display name; unique per workspace.
- `description` — human-facing metadata/display text. Do not assume runtime
  prompt impact unless source proves a consumer.
- `instructions` — team-level instructions added to the team leader briefing.
  They are not directly injected into every team member.
- `avatar_url` — optional team avatar URL.
- `leader_id` — agent ID of the team leader; the runtime target for
  team-routed work.
- `creator_id` — creator of the team.
- `archived_at` / `archived_by` — archive metadata. Archived teams are rejected
  by assignment/automation routing paths.
- `member_count` — list response count of team members.
- `member_preview` — list response preview of team members.

Use `instructions` for leader-facing coordination policy: team responsibility,
delegation expectations, when to ask humans, and review/handoff rules. Do not
write it as if every member automatically receives it.

## Team member fields

- `member_type` — `agent` or `member`.
- `member_id` — ID of the agent or workspace member.
- `role` — roster role label. Current behavior: non-empty `role` appears in the
  leader briefing roster. Do not assume it creates scheduling, permissions, or
  routing behavior.

## Creation and leader membership

Creating a team requires `leader_id`. The leader must be a workspace agent.
Create/update does not reject an archived leader: the lookup only checks the
agent exists in the workspace. An archived leader fails closed later, at
routing/dispatch — assignment, automation admission, and the comment/mention
readiness gate all reject an archived leader before any task is enqueued.

On create, the backend attempts to add the leader as a team member with role
`leader`. When updating `leader_id`, if the new leader is not already a member,
the backend adds the new leader as a team member with role `leader`.

## Leader briefing

For team leader tasks, Patchbay appends a team leader briefing to the leader
agent instructions. The briefing includes:

- Team Operating Protocol;
- Team Roster;
- Team Instructions, only when `instructions` is non-empty.

Roster entries include member name, member type, mention markdown, and non-empty
role. For agent members the roster also lists their assigned skills
(`skills: a, b`, or `no skills assigned` when the agent has none) so the leader
can delegate by capability instead of guessing from the role label; human
members carry no skills segment. Builtin `patchbay-*` skills are not listed —
only the workspace skills explicitly attached to the agent. Archived agent
members are skipped from the briefing roster.

## Issue assignment behavior

Issues can be assigned to teams with:

```text
executor_type = "team"
executor_id = <team-id>
```

Current behavior:

- assignment routes work to `team.leader_id`;
- it does not enqueue every team member;
- setting a team executor while status is `backlog` does not immediately start work;
- moving an issue with a team executor out of `backlog` can trigger the leader;
- changing executor cancels existing tasks for the issue before enqueueing the
  new executor path;
- parent issue status is agent-managed (same model as direct agent assignment):
  the leader's first assignment turn should move the parent to `in_progress`
  and keep it there while members work; the leader moves the parent to
  `in_review` only when a later re-trigger confirms the overall goal is met.
  Completing a leader `task` (including the first dispatch) does not itself
  change issue status;
- that status authority is granted only when the issue's `executor_type` /
  `executor_id` point at THIS team. The leader briefing is injected on every
  leader path, including an `@team` mention on an issue owned by a plain agent
  — on those paths the protocol instead carries an explicit "do not change this
  issue's status".

The status names above are category rules: a workspace may define custom
statuses beyond the built-ins, and each one inherits its category's behavior in
full (the runtime brief lists the workspace catalog when any exist).

Assignment validation rejects a missing type/id pair, non-existent team,
archived team, archived leader, and private leader when the actor cannot access
it.

## Comment and mention behavior

If an issue is assigned to a team, a new comment can wake the team leader. This
is leader routing, not member fan-out.

Team mention format:

```md
[@Team Name](mention://team/<team-id>)
```

Current behavior: resolve the team, read `leader_id`, enqueue a leader task,
and use the current comment as the trigger comment. It does not enqueue every
team member.

## Automation behavior

Automations can be assigned to teams. For `executor_type = "team"`:

- executable agent resolves from `team.leader_id`;
- admission/readiness checks run against the leader;
- archived teams fail closed / skip dispatch;
- run attribution records team id where applicable.

For `create_issue` automations, the created issue keeps `executor_type = "team"`
and `executor_id = <team-id>`, while the actual executing agent is the resolved
leader. For `run_only` automations, no issue is created; the task is created
directly for the resolved leader agent.

## Handling complaints or product gaps

When the user says team behavior is wrong, confusing, or disappointing, do not
immediately assume code is broken and do not defend current behavior just because
it exists. Classify first:

- expected current behavior;
- configuration issue;
- product limitation;
- actual bug.

Explain the current source-backed behavior. If the behavior is technically
correct but product-wise bad, say so and propose a scoped product/code change.

Do not silently change team routing, member fan-out, leader briefing, automation
behavior, or comment-trigger behavior without confirmation. These are product
contract changes with side effects.

## Side effects

These actions can trigger agent work or mutate durable state:

- creating a team;
- updating team fields;
- changing `leader_id`;
- adding/removing members;
- changing member roles;
- assigning an issue to a team;
- moving a team-assigned issue out of backlog;
- commenting on a team-assigned issue;
- mentioning a team;
- creating or triggering team-assigned automations;
- recording team activity with `patchbay team activity`;
- deleting/archive team.

Do not perform side-effecting actions as tests unless the user explicitly
authorizes them.

## Common wrong assumptions

- A team is not an agent.
- Team work routes to `leader_id`, not every member.
- Team mention routes to the leader, not every member.
- Team assignment routes to the leader, not every member.
- Team automation resolves to the leader as executable agent.
- `instructions` are leader briefing content, not automatic member prompts.
- `description` is not proven runtime prompt content.
- `role` is roster context, not automatic scheduling.
- Backlog assignment does not immediately start work.
- First leader dispatch is not parent completion — parent stays `in_progress`
  until the leader later confirms the overall goal and moves it to `in_review`.
- The server does not auto-flip parent status when child issues finish; it only
  wakes the leader with an explicit ask (including `in_review` when wrapping up).
- Getting the leader briefing does NOT imply status authority. A team
  `@`-mentioned into an issue assigned to someone else is a guest: roster and
  delegation rules yes, `patchbay issue status` no.

## References

For source paths, tests, edge cases, and exact routing details, see:

```text
references/team-source-map.md
```
