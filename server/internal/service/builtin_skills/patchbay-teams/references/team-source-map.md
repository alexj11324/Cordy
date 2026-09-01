# Team Source Map

This file records source evidence for `patchbay-teams/SKILL.md`.

Use this when the task requires exact source paths, edge-case behavior, tests, or contract verification.

## Object Model

### DB shape

Source:

```text
server/migrations/@@HIST_084_TEAM@@.up.sql                # base table: name, description, leader_id, creator_id
server/migrations/@@HIST_085_TEAM@@_archive.up.sql        # archived_at, archived_by columns
server/migrations/@@HIST_088_TEAM@@_instructions.up.sql   # instructions column
server/pkg/db/queries/team.sql
packages/core/types/team.ts
```

Key facts:

- `team` stores `name`, `description`, `leader_id`, `creator_id` (084), archive
  metadata `archived_at`/`archived_by` (085), and `instructions` (088).
- `team_member` stores `member_type`, `member_id`, and `role`.
- `member_type` is constrained to `agent` or `member`.
- issue `assignee_type` supports `team`.

## CLI

Source:

```text
server/cmd/patchbay/cmd_team.go
```

Commands:

```bash
patchbay team list
patchbay team get <team-id>
patchbay team create
patchbay team update <team-id>
patchbay team delete <team-id>
patchbay team activity <issue-id> <outcome>

patchbay team member list <team-id>
patchbay team member add <team-id>
patchbay team member remove <team-id>
patchbay team member set-role <team-id>
```

Use `--help` for exact flags before writes.

## Create / Update

Source:

```text
server/internal/handler/team.go                  # CreateTeam ~200-272, UpdateTeam ~287-364
server/pkg/db/queries/agent.sql                   # GetAgentInWorkspace ~15-17
server/pkg/db/generated/agent.sql.go              # getAgentInWorkspace ~1261
```

Contracts:

- create requires `leader_id` (team.go:215-218);
- leader must be a workspace agent — both create (team.go:230-237) and update
  (team.go:333-338) validate via `GetAgentInWorkspace`;
- archived leader is NOT rejected at create/update: `GetAgentInWorkspace` is
  `WHERE id = $1 AND workspace_id = $2` (agent.sql:15-17) with no archived
  filter, so an archived agent can be set as leader here. Archived-leader fails
  closed later, at routing/dispatch — see the readiness gate (team.go:945,
  isTeamLeaderReady → service.AgentReadiness at team.go:1017), assignment
  validation (issue.go:2625-2627), and automation admission (automation.go:885-891);
- leader is auto-added as member with role `leader` (team.go:258-263);
- updating `leader_id` auto-adds new leader as member if missing (team.go:340-347).

## Leader Briefing

Source:

```text
server/internal/handler/team_briefing.go         # buildTeamLeaderBriefing ~104, buildTeamRoster ~121, renderMemberRow ~169, agentSkillsRosterSegment, formatRosterRow
server/internal/handler/daemon.go                  # briefing injection ~1187, ~1530
```

Contracts:

- team leader tasks append briefing to leader agent instructions
  (daemon.go:1187, 1530);
- briefing includes operating protocol, roster, and optional instructions
  (team_briefing.go:104-117);
- `buildTeamLeaderBriefing` takes an `ownsIssueStatus` argument selecting
  responsibility 6 via `teamOperatingProtocolFor`: the status grant
  (`teamParentStatusOwned`) only when `issue.assignee_type == "team"` and
  `issue.assignee_id == team.id`, otherwise an explicit prohibition
  (`teamParentStatusNotOwned`). Quick-create passes `false` — no issue exists
  yet. Injection is broader than authority on purpose: it is keyed off
  `is_leader_task`, which also fires for `@team` mentions on issues owned by
  someone else (MUL-3724);
- when the claim's defensive gate withholds the briefing (NULL `team_id`,
  team hard-deleted, leader swapped after enqueue), the handler also clears
  `is_leader_task` on the claim response, so the wire flag means "briefing
  injected" and the run degrades to an ordinary agent turn. The daemon derives
  the leader role from that flag (plus `team_id` for quick-create), never from
  the briefing text (MUL-5811);
- every claim response carries `leader_role_resolved: true`, the capability
  that tells the daemon those fields are authoritative. Servers predating it
  omit it, and a daemon seeing it absent falls back to the legacy
  "`## Team Operating Protocol` appears in instructions" inference. That is
  the only correct read of either older shape: before #4951 no `is_leader_task`
  was sent at all, and after it the flag was sent without any guarantee that a
  briefing came with it. The field is claim-only and never rendered into a
  prompt;
- `instructions` section appears only when non-empty (team_briefing.go:110-112);
- archived agent members are skipped from roster (team_briefing.go:178-179);
- agent member roster rows list assigned workspace skills via
  `loadTeamMemberSkillNames` (ListAgentSkillNamesByAgentIDs) and
  `agentSkillsRosterSegment` — "skills: a, b" or
  "no skills assigned"; builtin patchbay-* skills are excluded and human
  members carry no skills segment (team_briefing.go renderMemberRow);
- no traced behavior injects `instructions` into every team member.

## Leader Evaluation Recording

Source:

```text
server/internal/handler/team.go                  # RecordTeamLeaderEvaluation ~949
server/cmd/patchbay/cmd_team.go                   # runTeamActivity ~459
server/internal/service/team_no_action.go        # HasTeamLeaderNoActionEvaluationForTask
server/internal/handler/comment.go                # no_action comment rejection ~1851
```

Contracts:

- authority is the TASK row from `X-Task-ID`, not the issue's assignee:
  `task.issue_id == issue.id`, `task.is_leader_task`, `task.team_id` valid.
  The team is loaded from `task.team_id`; the target issue may be assigned to
  anyone (MUL-6622 / GH #7487). The pre-fix `issue.assignee_type == "team"`
  gate diverged from the claim-side `is_leader_task` gate and made the call
  unsatisfiable on `@team`-on-agent-issue and leader-task-on-child paths;
- the task is loaded with `GetAgentTaskInWorkspace`, not `GetAgentTask`: the
  latter is a global lookup by id, and the rejection path quotes the task's
  issue id. Check order is load-bearing — tenant scope, then "caller owns this
  task", and only then any message naming a task-derived id;
- two authorization gates: `task.agent_id == caller`, then
  `team.leader_id == caller`. The second is required because
  `is_leader_task` on the row is enqueue-time INTENT: when the leader was
  swapped before the claim, `handler/daemon.go` clears `resp.IsLeaderTask` and
  runs the task as an ordinary agent turn while the row keeps
  `is_leader_task = true`. Without the live check, such a downgraded run could
  write a leader verdict and suppress its own comment. Removing this gate
  requires persisting the role the claim actually delivered;
- `activity_log.actor_id` is `task.agent_id`, not `team.leader_id`: the
  `no_action` comment suppression lookup matches on `task.agent_id`, so the
  live leader id there would silently break suppression;
- a leader agent running a NON-leader task is rejected — it is not running as
  the leader, and the runtime only mandates the call for `is_leader_task`;
- the `no_action` comment prohibition is conditional on this write succeeding
  (comment.go:1851 checks the activity exists), so the injected instructions
  tell leaders to fall back to a comment when the call errors — capped at one
  comment, and only when the turn has not already commented.

## Issue Assignment

Source:

```text
server/internal/handler/issue.go                  # assignee validation ~2614-2632
server/internal/handler/team.go                   # shouldEnqueueTeamLeaderOnAssign ~990, enqueueTeamLeaderTask ~1027
server/internal/service/task.go
```

Contracts:

- `assignee_type="team"` routes to `team.leader_id` (team.go:1028-1050);
- backlog assignment does not immediately enqueue (team.go:991-993);
- moving out of backlog can enqueue leader (team.go:990-994 → isTeamLeaderReady);
- assignee change cancels existing issue tasks first;
- private leader access is checked at assign-time (issue.go:2629-2632) and at
  enqueue-time via `canEnqueueTeamLeader` (team.go:1037);
- archived team / archived leader rejected at assign-time (issue.go:2622-2627);
- pending task dedup is applied (team.go:1042-1048);
- parent status is agent-managed: since MUL-6417 the brief's status rule is a
  fact judgment written when the work changes it (`writeWorkflowIssue`), and the
  leader variant adds one bullet — dispatching members is not delivery, so a dispatch
  turn leaves the parent `in_progress` and `in_review` waits for the re-trigger
  that confirms the overall goal is met. Team Operating Protocol
  (`team_briefing.go`) still states the ongoing `in_progress` → later
  `in_review` responsibility for owning leaders. `StartTask` / `CompleteTask`
  do not write issue status. There is no assignee gate anymore: a guest leader
  writes nothing not because it lacks a grant but because a turn that did not
  move the issue's state has nothing to record.
- status names are category rules: custom statuses inherit their category's
  behavior in full (MUL-6243, `server/internal/issuestatus/issuestatus.go`
  `Effective`/`Resolve`); the brief lists the workspace catalog when any custom
  statuses exist (MUL-6460, `writeIssueStatusCommand` in
  `server/internal/daemon/execenv/runtime_config_sections.go`).

## Comment / Mention

Source:

```text
server/internal/handler/comment.go                # comment triggers ~1057-1199, team mention branch ~1352
server/internal/handler/team.go                   # enqueueTeamLeaderTask ~986 (assign/backlog paths), lastTaskWasLeader ~915
server/internal/service/task.go                   # EnqueueTaskForTeamLeader
```

Contracts:

- commenting on a team-assigned issue can wake the leader — the comment path
  computes triggers via `computeCommentAgentTriggers` (comment.go:1124), whose
  assigned-team branch is `computeAssignedTeamLeaderCommentTrigger`
  (comment.go:1162-1199); the same computation backs the trigger-preview
  endpoint;
- explicit `mention://team/<id>` resolves team and adds the leader trigger
  (comment.go:1352-1391);
- team mention does not fan out to members — enqueue targets `team.LeaderID`
  only (comment.go:1104-1112, and team.go:1007 on the assign/backlog paths);
- leader task uses `is_leader_task=true` (via `EnqueueTaskForTeamLeader`);
- leader self-trigger loops are guarded — same-leader / last-task-was-leader
  guards (comment.go:1173-1176, lastTaskWasLeader at team.go:915) and member
  explicit-mention skip (comment.go:1177-1179).

## Automation

Source:

```text
server/internal/service/automation.go              # resolveAutomationLeader ~617-655, dispatch ~88-111
server/internal/handler/automation.go              # save-time validateAutomationAssignee ~845-893
```

Contracts:

- team automation resolves executable agent from `team.leader_id` —
  `resolveAutomationLeader` team branch (automation.go:639-651);
- readiness/admission checks target the leader: save-time validation rejects an
  archived team/leader (handler/automation.go:881-891), and dispatch re-runs
  `resolveAutomationLeader` + `AgentReadiness`;
- archived team fails closed / skips dispatch — `errTeamArchived`
  (automation.go:644-645);
- `create_issue` keeps the issue assigned to the team (automation.go:88-97);
- `run_only` creates task directly for leader (automation.go:99-106, dispatch via
  `resolveAutomationLeader` at automation.go:284).

## Child-done Parent Trigger

Source:

```text
server/internal/handler/issue_child_done.go       # dispatchParentAssigneeTrigger ~246, triggerChildDoneTeam ~304
```

Contracts:

- when a child issue closes a stage barrier and the parent is assigned to a
  team, the parent team leader is triggered (triggerChildDoneTeam in
  issue_child_done.go);
- routing is leader-only — one `EnqueueTaskForTeamLeader` on the leader, no
  member fan-out (triggerChildDoneTeam / dispatchParentAssigneeTrigger);
- no self-trigger guard: a same-team or shared-leader child still wakes the
  parent team leader — the wake is a serial handoff onto the PARENT and is the
  only carrier of the stage-barrier "advance / wrap up" instruction (MUL-3969,
  mirrors the agent path from MUL-2808). Re-triggering is bounded only by
  `HasPendingTaskForIssueAndAgent` (idempotent per parent issue + agent).
- no leader-invocation gate: child-done does NOT re-check whether the child's
  completer can invoke the leader. The parent was already permission-checked at
  team-assign time (`validateAssigneePair`), so waking its own leader is a
  coordination handoff, not a fresh invocation. Re-checking it here failed
  closed for the DEFAULT private leader (the child's completer is an
  agent/system actor with no resolvable human originator), stranding every
  process-team pipeline after stage 1 while direct-to-leader-agent parents
  advanced fine (MUL-4063 / GH #4928). Agent and team child-done now share one
  ungated path; any future invocation gate must be added to BOTH together.
- parent status is not auto-advanced by the barrier: the system comment asks the
  leader to continue or — when the overall goal is met — run
  `patchbay issue status <parent-id> in_review`. The Team Operating Protocol's
  standing "Own the parent issue status" responsibility (present exactly when
  the issue is assigned to this team) states the same expectation; the system
  comment marks the wrap-up moment. Since MUL-6417 the write itself needs no
  grant — the brief's fact judgment covers it — but `done` remains
  human / integration owned.

## Private Leader Access

Source:

```text
server/internal/handler/agent_access.go           # canInvokeAgent ~48-108, canEnqueueTeamLeader ~261-267
server/internal/handler/team.go                   # enqueueTeamLeaderTask gate ~955-974
```

Contracts (invocation gate, MUL-3963 — this is the *trigger* gate, distinct from
the view gate `canAccessPrivateAgent`):

- `canEnqueueTeamLeader` loads the leader and delegates to `canInvokeAgent`
  (agent_access.go:261-267);
- `canInvokeAgent` judges by the *effective invoking user*: a member actor is
  itself; an agent/system actor is the top-of-chain human originator
  (`originatorUserID`), which is `""` when none resolved (agent_access.go:48-54);
- the agent owner may always invoke their own agent (agent_access.go:57-59);
- `permission_mode != "public_to"` (i.e. private) is deny-by-default — no admin
  bypass, no A2A bypass; only the owner branch passes (agent_access.go:61-65);
- `public_to` consults the invocation-target allow-list: a `workspace` target
  admits any workspace member AND workspace-internal agent/system principals even
  with no resolved human (`workspaceBroad`); `member` targets require the
  resolved human to match; `team` targets are inert in V1 (agent_access.go:82-106);
- wired into `enqueueTeamLeaderTask` (team.go:955-974): the team
  assign/promote path denies the enqueue when the actor cannot invoke the leader
  (member authors are their own originator; agent-authored triggers pass `""`).
- NOTE: the child-done wake does NOT use this gate anymore — see "Child-done
  Parent Trigger" above (MUL-4063).

## Tests

Relevant test groups:

```text
server/internal/handler/team_assign_trigger_test.go
server/internal/handler/team_comment_trigger_test.go
server/internal/handler/team_briefing_test.go
server/internal/handler/team_private_leader_test.go
server/internal/handler/automation_private_leader_test.go
server/internal/handler/team_no_action_test.go
```

Verification command:

```bash
go test ./internal/handler -run 'Test.*Team|Test.*team|Test.*Automation.*Team|Test.*ChildDone.*Team'
```
