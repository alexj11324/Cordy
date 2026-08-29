# Phase 1 authorization foundation

Status: implementation contract for the first production slice.

## Observable acceptance

Phase 1 is complete only when all of the following are observable in production
code and in CI:

1. One authorizer accepts `Principal + Action + Resource + Context +
   DelegationChain` and returns `allow`, `deny`, or `require_approval` with a
   reason, matched grant IDs, a policy version, and obligations. Callers treat
   only `allow` as permission to continue.
2. The vocabulary distinguishes user, team, agent definition, task/run,
   device/runtime, service, and system principals. Agent invocation is a
   separate `agent.invoke` decision; it never implies tool, credential, runtime,
   or directory authority.
3. `task_token` is the persisted capability lease for a run. The server writes
   its scope, expiry, revocation, claim fence, parent lease, parent fence, and
   delegation depth. Authentication rejects a lease when its task is terminal,
   its claim has been superseded, it or an ancestor is expired/revoked, an
   ancestor fence changed, or a child scope is not a subset of its parent.
4. A delegated task lease is derived from the parent lease and cannot add an
   action/resource capability. Delegation is capped at eight hops. Concurrent
   or replayed finalization cannot create two active leases for one task claim,
   and revocation is monotonic.
5. Permanent grants are persisted separately from resources. Explicit deny
   wins; grants can require approval; task/run decisions still require a valid
   lease, so a standing grant cannot widen a task scope.
6. A task claim never receives stored Agent `custom_env`, custom arguments,
   MCP configuration, Runtime configuration, workspace plugin tools, connected
   apps, Composio overlays, local directory paths, or repository URLs. These
   paths can expose long-lived owner/workspace credentials or the runtime
   owner's checkout and credential helper, and there is no lease-bound broker
   for them in Phase 1. Human
   connection management remains available; a later broker must consume
   `credential.use` and return only a short-lived, lease-bound session.
   The server claim payload never returns long-lived credential material,
   including for an owner-originated run. Stored current/prior work directories, branch names,
   durable directories, and provider session IDs are also stripped so reruns
   and chat continuity cannot recover another caller's local execution state.
7. Runtime read/update is enforced by the same authorizer. Workspace admins do
   not automatically read or mutate another user's private runtime. Public
   runtime metadata remains available to workspace members. Local runtime use
   is owner-only even when the runtime is public or a foreign-user grant
   exists, because the current daemon sandbox exposes its account HOME and
   credential helpers. Non-local public compute may retain workspace sharing;
   private runtime read/use remains owner- or explicit-grant-only where the
   local-device guardrail does not apply.
8. Every authorizer result appends an explain record that can answer: who,
   on whose behalf, via which agent, on which device, action, resource,
   decision, why, matched grants, policy version, obligations, and delegation
   chain. Explain reads are actor-scoped; workspace owners may inspect their
   workspace, while admins do not receive a private-resource bypass.

Required negative tests cover cross-user shared-agent credential isolation,
child-scope narrowing, expiry/revocation/task completion, private-resource admin
denial, `require_approval` fail-closed behavior, and concurrent/replayed claim
fencing.

## Known release blocker

The current local daemon still materializes provider login state (for example,
the Codex `auth.json`) inside task-visible runtime state, while its provider
shell can execute as the daemon OS user with full filesystem access. The server
boundary in this phase prevents a foreign caller from receiving another
user's local runtime, but it cannot truthfully enforce `credential.read_secret`
against an owner-originated local process that can read the file directly.

This slice must not be released as satisfying the acceptance above until the
product chooses one safe boundary: disable task claims for local providers that
materialize long-lived login state, or add enforced process/filesystem
isolation plus a short-lived credential broker. This is a release decision, not
a policy-engine follow-up that can be hidden behind the server interface.

## Root cause and risk boundary

The immediate confused-deputy root cause is earlier than daemon execution:
claim assembly treats successful `agent.invoke` admission as permission to
copy Agent env/MCP/Runtime configuration and workspace plugin/connection
credentials into the run. The originator is carried only for attribution. A
shared agent therefore turns owner or workspace execution configuration into
ambient authority for callers.

The existing `mat_` token fixes actor-header forgery but is an identity token,
not yet a complete lease: it carries no scope or parent chain, and
authentication historically checked expiry only. Completed and failed tasks
did not eagerly revoke it. Runtime handlers also contain role shortcuts that
let an admin enumerate or edit private runtimes.

The compatibility `task_token.user_id` projection also makes every ordinary
user route a potential confused deputy. Phase 1 therefore admits task leases
only to an explicit data-plane route set and rejects credential, account,
Agent-definition, workspace-control, connector, billing, attachment, Chat,
plugin/tool, and other human control-plane routes before their handlers run.
Task read/update endpoints bind the requested task to the lease task; single
Issue reads, comments, and mutations that remain available consume the
project-resource lease and bind to the task's Issue.

Phase 1 fixes those earliest boundaries. It intentionally does not rewrite all
legacy resource handlers, introduce a universal resource registry, expose
long-lived secrets, add an external policy service, or reinterpret
`team_member.role`. That field remains an orchestration label (`leader`,
`worker`, `reviewer`, and similar); future team security membership must use a
separate field/table. Workspace database roles remain owner/admin/member; the
authorizer vocabulary reserves guest as a deny-by-default boundary without
making guest a valid persisted membership in this migration.

## Effective-permission invariant

For task/run principals, the engine applies boundaries in this order:

1. workspace guardrails and non-delegable hard denies;
2. an active, claim-bound task lease and every ancestor lease;
3. parent-scope containment and delegation-depth/fence checks;
4. resource relationship/visibility and request attributes;
5. explicit deny / require-approval / allow grants;
6. device and approval obligations.

No later layer can turn an earlier deny into allow. A standing grant may grant a
human access, but it cannot add an action/resource pair absent from a task's
lease. A child lease stores only the intersection of its requested scope and
the parent's effective scope.

## Migration and rollback

The migration is additive and compatible with rows from current `main`:

- add authorization grant and append-only decision-audit tables without
  foreign keys or cascades;
- add lease columns to `task_token`, backfilling existing live tokens with a
  conservative invocation scope, current claim timestamp, and initiating-user
  compatibility identity; historical tokens without an initiating user are
  revoked;
- remove duplicate historical claim rows before creating the unconditional
  unique claim-fence index, so revocation cannot make a claim consumable again;
- build every new index concurrently in its own migration;
- keep raw bearer values out of the new tables and audit payloads.

Rollback first removes concurrent indexes and deletes every task bearer before
dropping the two additive tables and the added `task_token` columns. The legacy
schema cannot represent scope, delegation, identity, revocation, or claim
fences, so retaining even an active Phase 1 token would widen it. Unrelated
task/user data remains, but running tasks must claim a new token after rollback.
Rollback is not availability- or permission-preserving: it must be treated as a
security rollback, not a normal operational toggle.

## Explicit follow-ups

Later slices should add lease-bound short-lived credential/tool and
Directory/repository brokers before re-enabling Agent execution configuration,
plugin/connection tools, or local checkout projection; move remaining private
Agent, Chat, Integration/Connection, plugin/tool, and device-management
handlers onto the same interface; add separate team
security membership; add Guest membership storage and invitation rules; add an
explicit System-task workspace principal; and replace remaining member-shaped
task-token compatibility reads. Runtime listing should also batch relationship
and grant evaluation before it becomes a large-workspace path; Phase 1 keeps the
per-resource audit writes for explainability. Those are not prerequisites for
this slice's real enforcement consumers and must not weaken the Phase 1
boundaries while pending.
