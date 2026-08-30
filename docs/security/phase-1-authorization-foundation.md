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
6. Codex/Claude provider identity is a distinct protected resource. A runtime
   owner must explicitly grant `credential.use` to a user, team, or Agent
   definition; the grant binds workspace, grantee, provider, action, model or
   token budget, expiry, runtime/device, and optionally one task. Deny and
   `require_approval` win and fail closed. A delegated child needs an exact
   task-bound grant, so a generic standing grant cannot be transferred or
   widened through subdelegation.
7. The task claim receives only the server-computed provider-authorization
   descriptor. The daemon keeps the long-lived API key, Codex `auth.json`, or
   Claude `.credentials.json` on the host and substitutes it only on the
   broker's upstream request. A task receives a random, task-local broker
   bearer and loopback URL, never a long-lived provider token, refresh token,
   host credential path, or provider login document in its environment, task
   directory, claim IPC, deep link, or logs. OAuth refresh occurs under a
   daemon-side mutex before expiry and once after an upstream 401; refresh
   rotation is atomically persisted with mode `0600` on Unix. Invalidated or
   revoked provider sessions require the host user to sign in again. The
   broker permits only provider inference operations (`responses`,
   `responses/compact`, `chat/completions`, `messages`, and Anthropic token
   counting); account, organization, model-administration, file, and other
   provider APIs are denied before budget consumption or credential use.
8. Every provider process tree is launched behind a fail-closed OS/filesystem
   boundary. macOS uses a deny-by-default sandbox profile; Linux uses
   bubblewrap with isolated process/IPC/UTS/cgroup namespaces, a hidden host
   home, read-only system/provider files, and writes limited to task-owned
   roots. Unsupported hosts and missing sandbox executables cannot launch a
   provider task. Ambient daemon variables are cleared and rebuilt from an
   inert allowlist before every provider spawn.
9. Runtime read/update is enforced by the same authorizer. Workspace admins do
   not automatically read or mutate another user's private runtime. Public
   runtime metadata remains available to workspace members. Local runtime use
   by another member requires both the explicit provider-identity grant above
   and the brokered-provider execution attribute; ordinary workspace role or
   public visibility cannot supply the runtime owner's provider identity.
10. Every authorizer result appends an explain record that can answer: who,
   on whose behalf, via which agent, on which device, action, resource,
   decision, why, matched grants, policy version, obligations, and delegation
   chain. Explain reads are actor-scoped; workspace owners may inspect their
   workspace, while admins do not receive a private-resource bypass.

Required negative tests cover cross-user shared-agent credential isolation,
cross-member provider denial without an explicit grant, provider grant
revocation/model/budget/device/task fencing, child-scope narrowing,
expiry/revocation/task completion, private-resource admin denial,
`require_approval` fail-closed behavior, and concurrent/replayed claim fencing.

## Fail-closed deployment boundary

Provider tasks are available only where the daemon can prove the process and
filesystem boundary: the system `sandbox-exec` on macOS or bubblewrap on Linux.
Windows and other unsupported hosts reject provider execution in this slice;
they do not fall back to exposing the host account. A trusted device still
performs its first provider login locally. After that, the daemon renews a
valid OAuth session without daily user interaction; explicit logout,
revocation, refresh-session expiry, or provider rejection is the re-login
boundary. API-key logins have no refresh protocol and remain valid only while
the host-managed key is valid. OpenClaw is not broker-integrated in this slice:
any host OpenClaw configuration or Gateway bearer rejects task launch, and the
daemon removes legacy task-local OpenClaw snapshots before attempting
preparation. It never includes a host OpenClaw config or writes a Gateway token
into a task wrapper.

## Root cause and risk boundary

The immediate confused-deputy root cause spans claim assembly and provider
launch: successful `agent.invoke` admission historically doubled as permission
to use the Agent or Runtime owner's ambient provider login. The originator was
carried only for attribution, while the provider process inherited the host
account and could read its credential files. A shared agent therefore turned
owner execution configuration into ambient authority for callers.

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

Phase 1 fixes those earliest boundaries by requiring an independently matched
provider grant, issuing a constraint-only claim descriptor, revalidating the
task lease and standing grant on every broker request, and isolating the entire
provider process tree from the host login. It intentionally does not rewrite all
legacy resource handlers, introduce a universal resource registry, expose
long-lived secrets, add an external policy service, or reinterpret
`team_member.role`. That field remains an orchestration label (`leader`,
`worker`, `reviewer`, and similar); future team security membership must use a
separate field/table. Workspace database roles remain owner/admin/member.
Server-backed guest accounts merged on `main` are identified separately from
membership role, are limited to one workspace, and remain excluded from formal
account operations such as invitations and external authorization. The
authorizer vocabulary keeps `guest` as a deny-by-default policy boundary; this
slice does not reinterpret a guest account's single-workspace owner row as a
general cross-member delegation role.

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

Later slices should add lease-bound tool, Directory/repository, and remaining
integration brokers before re-enabling additional Agent execution configuration
or local checkout projection; move remaining private
Agent, Chat, Integration/Connection, plugin/tool, and device-management
handlers onto the same interface; add separate team
security membership; extend guest-specific authorizer context beyond the
existing formal-account guards; add an explicit System-task workspace
principal; and replace remaining member-shaped
task-token compatibility reads. Runtime listing should also batch relationship
and grant evaluation before it becomes a large-workspace path; Phase 1 keeps the
per-resource audit writes for explainability. Those are not prerequisites for
this slice's real enforcement consumers and must not weaken the Phase 1
boundaries while pending.
