---
name: patchbay-task-planning
description: "Use only for a complex, genuinely splittable goal when a team leader or an explicitly designated planner must propose a minimal dependency DAG."
user-invocable: false
allowed-tools: Bash(patchbay *), Bash(git *), Bash(gh *)
---

# Patchbay dependency-aware task planning

Use this skill only for complex, genuinely splittable goals that can be split
into two or more independently verifiable deliverables. It is a
planning capability for a team leader or explicitly designated planner. It is
not a replacement for the team leader base prompt, `patchbay-working-on-issues`,
or ordinary delegation. A simple, coherent goal remains one task. If it is
simple, do not split it merely because it mentions several files, commands,
functions, people, or steps.

The result of this skill is a complete typed plan submitted to the server's
atomic dependency-graph apply operation. Do not create child issues one at a
time and add edges later. A child created as an assigned `todo` before its
prerequisites are persisted can start in the gap, so that workflow is
forbidden.

The governing rule is: **LLM proposes; runtime validates.** This prompt helps
construct a useful proposal, but it cannot replace server-side workspace
scope, issue-reference, authorization, cycle, duplicate-edge, transaction,
idempotency, admission, or readiness-scheduler invariants. If the atomic
apply operation is unavailable, fail closed and report that planning cannot be
activated safely; do not emulate it with `issue create`, `issue update`, or
`issue assign` calls.

## 1. Decide whether to split

First understand the requested outcome, then decide whether decomposition is
actually useful.

Do not split when the work is one coherent change, one investigation with one
answer, a short sequence that cannot be independently accepted, or when the
proposed children would only divide files, commands, implementation steps,
or review chores. Avoid micro-tasks such as “edit file X”, “run command Y”, or
“write one function” unless that item is itself a separately owned,
independently verifiable deliverable required by the parent goal.

Split only when each child has a coherent responsibility, a concrete
deliverable, an observable completion condition, and a useful boundary for
parallel execution. Preserve the parent's goal, constraints, repository
scope, acceptance criteria, risk constraints, and required final integration.
If the work can be completed correctly as one issue, do not manufacture a
graph.

Before proposing tasks, read the available source of truth:

1. Read the parent issue and its acceptance criteria. Use the real issue
   identifier, not a guessed title:

   ```bash
   patchbay issue get <parent> --output json
   patchbay issue comment list <parent> --output json
   ```

2. Read the repository architecture, applicable `AGENTS.md`/rules, existing
   interfaces, API/schema contracts, migrations, tests, and the downstream
   consumer expectations relevant to the parent issue. Use `git` and `gh`
   read-only commands as needed. Do not infer a dependency from file layout
   or from an imagined merge order.

3. Identify the exact artifacts each possible child produces and what a
   downstream child would have to consume. If an output is not concrete enough
   to name and verify, the task boundary is not ready.

Ask for every candidate pair A and B:

> Can B correctly start and finish without knowing or consuming A's result?

If the answer is yes, do **not** create an A -> B dependency. Shared files,
the same module, likely merge conflicts, common ownership, or a desire to
review in order are not semantic dependencies. If the answer is no, create a
hard edge only when B consumes a specific successful output of A. Name that
output exactly in `consumed_output` and explain it for the user in `reason`.

Maximize safe parallelism. For every proposed blocked task, ask whether it
could start earlier with a narrower input, a stable interface, or a different
task boundary. Remove an edge that is only a transitive consequence of other
edges. The final graph should be a minimal dependency DAG, not a serialized
project plan.

## 2. Define independently verifiable tasks

Every task must stand on its own as an issue-sized deliverable. Give it:

- a stable temporary identifier unique within this plan;
- a precise title;
- a description that states responsibility, scope, context, and deliverable;
- acceptance criteria that another engineer can observe and verify;
- observable completion conditions, not an intention to “make progress”;
- concrete `outputs` that can be consumed or inspected;
- an `assignee` when ownership is known, or explicit candidate assignees;
- enough context for an unfamiliar engineer to execute it without reconstructing
  the parent conversation.

Do not hide an API, schema, artifact, migration, design, integration, or
verification dependency in prose. Either make the producing work an explicit
task and connect the real consumer, or keep the work in one coherent task.
Do not use a dependency to express a stage barrier, a preferred order, a
shared file, or a possible merge conflict.

Use only V1 hard dependencies. A hard prerequisite is satisfied only when its
issue has successfully reached `Done`. `todo`, `in_progress`, `in_review`,
`blocked`, `cancelled`, and `failed` do not satisfy a hard edge. A failed or
cancelled prerequisite must keep its dependents fail-closed and require
observable replanning or attention; never reinterpret failure as completion.

## 3. Construct the typed plan

Build the complete plan in memory before submitting it. The minimum JSON shape
is:

```json
{
  "goal": "The parent outcome this graph delivers",
  "parent_issue_id": "<parent UUID>",
  "tasks": [
    {
      "temp_id": "api-contract",
      "title": "Define the stable graph API contract",
      "description": "Responsibility, context, deliverable, and observable completion.",
      "acceptance_criteria": [
        "The contract is represented by a focused test or other observable evidence."
      ],
      "context": {
        "parent_constraint": "Relevant repository/API context"
      },
      "outputs": [
        "dependency graph API response contract"
      ],
      "assignee": {
        "type": "agent",
        "id": "<agent UUID>"
      },
      "candidate_assignees": []
    }
  ],
  "edges": [
    {
      "from": "api-contract",
      "to": "integration",
      "type": "hard",
      "reason": "Integration consumes the persisted response contract produced by api-contract.",
      "consumed_output": "dependency graph API response contract"
    }
  ],
  "waves": [["api-contract"], ["integration"]]
}
```

`waves` is a topology projection for explanation only. The server derives and
validates it; persisted edges are the execution source of truth. `from` is
always the prerequisite and `to` is always the dependent. Never reverse those
names to match a UI phrase such as “blocked by”. Use `type: "hard"` only in
V1. Every edge must have a non-empty, user-readable `reason` and the exact
`consumed_output` from the source task's `outputs`.

Before applying, verify all of the following yourself:

- every task contributes directly to the parent goal;
- task granularity is coherent and independently verifiable;
- no safe parallel work is hidden behind an unnecessary edge;
- all real API, schema, artifact, migration, design, integration, and
  verification prerequisites are represented;
- every edge names an actual consumed output, not just a topic or file;
- there are no self-edges, nonexistent endpoints, duplicate pairs, or cycles;
- transitive-only edges have been removed;
- every root task can run immediately;
- every non-root task waits for all hard prerequisites to reach successful Done;
- cancellation and failure remain fail-closed and visible for replanning;
- an unfamiliar engineer can understand the goal, responsibility, output,
  acceptance, owner, and dependency reason from the plan alone.

Check each blocked task one more time: could it begin with a stable contract,
an independently available fixture, or a narrower responsibility? If yes,
remove the edge and preserve parallelism.

## 4. Apply atomically

Use a stable idempotency key for this exact proposal. Reusing the key is safe
for replaying the same request; changing the plan requires a new key and a
new, intentional proposal. Submit the entire typed plan in one request:

```bash
patchbay issue dependency-graph apply <parent> \
  --idempotency-key "<stable-key-for-this-plan>" \
  --plan-stdin --output json < plan.json
```

Or use a plan file inside the current task worktree:

```bash
patchbay issue dependency-graph apply <parent> \
  --idempotency-key "<stable-key-for-this-plan>" \
  --plan-file ./plan.json --output json
```

The command sends the full proposal to the server, which validates workspace
scope, references, assignees, edge direction, output references, duplicates,
cycles, and idempotency before committing child issues, nodes, and edges in
one transaction. A rejected plan must leave no partially created assigned
children. Do not retry a changed body with the old idempotency key. Do not
fall back to these unsafe sequences:

```text
issue create -> issue assign -> add dependency edges
issue create children -> wait -> repair graph
issue update/status to simulate a dependency gate
```

Those operations cannot provide atomic plan application and can let a
dependent run before its graph is durable. If the endpoint returns a validation
or conflict error, inspect the response, correct the typed proposal, and use a
new key only for a deliberately changed plan. If the endpoint is missing or
the runtime cannot prove that apply is atomic, stop and report the blocker.

## 5. Explain and monitor the real graph

After a successful apply, read the persisted graph rather than trusting the
proposal's waves:

```bash
patchbay issue dependency-graph get <parent> --output json
```

Report the plan id, task identifiers, persisted edges and reasons, derived
waves, initial root readiness, blocked prerequisites, and any attention state.
The scheduler—not this prompt—decides admission. A root may be enqueued when
its gate is open. A dependent may be enqueued only after every hard
prerequisite is successfully Done. Replays, retries, restarts, and concurrent
completion must not enqueue the same task twice. A cancelled or failed
prerequisite keeps the dependent blocked and must leave the plan in an
observable attention/replanning state.

When reporting completion, include:

1. the parent goal and whether it was kept intact;
2. the final typed task list with outputs and assignees/candidates;
3. each real hard edge with `from`, `to`, exact consumed output, and reason;
4. the server-derived waves and current readiness/gate state;
5. validation or apply failures, if any, without hiding them;
6. what remains for replanning when a prerequisite failed or was cancelled.

Never claim that a prompt, wave, status label, or UI rendering enforces a
dependency. Correctness belongs to the server validator, atomic persistence,
admission gate, and readiness scheduler.
