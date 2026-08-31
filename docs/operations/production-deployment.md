# Aspectlylabs production delivery

## Goal and acceptance

Every reviewed merge to `main` must produce and deploy one coherent production
revision. Completion means all of the following are true:

1. The normal `CI` workflow succeeded for the exact `main` commit.
2. Backend, Web, Docs, and Auth Broker were all built for `linux/arm64` from
   that same full SHA and published by immutable digest.
3. The restricted server gateway accepted that SHA as the current remote
   `main`, deployed the four digests, and retained the preceding manifest.
4. The API reports `server_version=sha-<full-sha>`; Web, Docs, and Auth Broker
   report `X-Patchbay-Build: sha-<full-sha>`.
5. `/login`, `/acme/issues`, `/acme/task-graph`, `/docs`, and the Accounts
   OAuth entry route return a successful or redirect response (HTTP 2xx/3xx)
   through the public domains. API config and readiness routes must return 200.

A merged PR, green image build, healthy Accounts domain, or successful SSH
command alone does not satisfy this acceptance contract.

## Pipeline

```text
merge to main
  -> CI (exact main SHA)
  -> build complete four-image set in parallel
  -> assemble and verify immutable digest manifest
  -> production Environment + serialized deploy
  -> restricted server gateway
  -> local runtime/version probes
  -> public domain/version/route probes
  -> success, or automatic rollback to previous manifest
```

The matrix is only scheduling parallelism. It has no changed-path filter and no
partial-image input. Incremental BuildKit and Cargo caches are performance
optimizations; every run still executes each production Dockerfile.

`workflow_run` is accepted only when the upstream workflow is `CI`, the event
was a same-repository push, the branch is `main`, and the conclusion is
successful. The resolver and server gateway both reject a SHA that is no
longer the current `origin/main`, preventing an older queued run from rolling
production backward.

## GitHub Environment

Create the `production` Environment with a selected-branch policy containing
only `main`. Store these values as Environment secrets, never repository
secrets:

- `PRODUCTION_SSH_PRIVATE_KEY`
- `PRODUCTION_SSH_KNOWN_HOSTS`
- `PRODUCTION_SSH_HOST`
- `PRODUCTION_SSH_USER`

The private key must be dedicated to this pipeline. Its public key is installed
with OpenSSH `restrict` plus a forced command, so it cannot open a shell,
forward ports, copy files, or choose a server command. The forced command is
the root-owned `/usr/local/bin/patchbay-production-deploy` gateway.

## One-time server bootstrap

From a reviewed checkout, generate a dedicated Ed25519 key and run:

```bash
sudo deploy/origin/install-production-deploy.sh /path/to/deploy-key.pub
```

The installer copies the gateway and static Compose overlays to root-owned
paths, appends the restricted public-key entry for the deployment user, and
captures the currently running containers' required environment values into
mode-0600 JSON under `/var/lib/patchbay-production/secrets`. It never prints
those values. The initial current manifest records the existing image digests
(falling back to an allow-listed configured tag only when local Docker metadata
has no digest) so the first automatic deployment also has a rollback target.
Re-running the installer validates and preserves existing deployment history
instead of bootstrapping over it.

That bootstrap target is checked for service readiness only, because it may
predate this pipeline's complete route contract (and may be the broken version
the first deployment is intended to replace). After the first successful
automated deployment, every normal deployment and rollback must pass the full
version and business-route checks.

The server gateway accepts a maximum 64 KiB JSON request. A deployment request
must name `alexj11324/Cordy`, the exact current 40-character `main` SHA, and <!-- legacy-brand-compat -->
exactly four allow-listed `ghcr.io/alexj11324/patchbay-*` sha256 references.
Caller-provided commands, paths, Compose options, tags such as `latest`, and
arbitrary registries are rejected.

## Deployment and rollback behavior

The gateway serializes operations with a host lock. It fetches `main` into a
bare cache, creates a detached release worktree, pulls all four digests before
mutation, and then updates the existing Compose projects and ports:

- `cordy632`: Backend on `127.0.0.1:8210`, Web on `127.0.0.1:3110`, retaining <!-- legacy-brand-compat -->
  the existing PostgreSQL and uploads volumes.
- `cordy`: Docs on `127.0.0.1:4000`. <!-- legacy-brand-compat -->
- `patchbay-auth-broker`: Broker on `127.0.0.1:43100`.

Backend is made ready before Web, so migrations finish before new Web traffic.
Every image update is followed by local version and route probes. If any
command or local probe fails, the gateway immediately reapplies the preceding
manifest. If the later public-domain probes fail, GitHub Actions sends a
separate rollback request; it is accepted only when the failed SHA is still the
current deployment, preventing rollback from racing a newer release.

Database migrations must remain backward-compatible with the immediately prior
application image. Automatic image rollback cannot reverse a destructive
schema migration safely; such a migration requires a separately reviewed
expand/migrate/contract sequence.

## Legacy infrastructure identities

Two narrowly scoped legacy identities remain: the current GitHub repository
identity used by the gateway allow-list, and the existing production Compose
project/container names used to reattach the live database and upload volumes.
The platform owner owns both residuals. The repository literals are deleted
only after an approved GitHub repository rename; the Compose/container literals
are deleted only after a separately reviewed, backed-up volume migration. The
legacy-marker CI check and the gateway bootstrap/deployment contracts verify
that no unlisted product-facing legacy spelling is introduced.

## Main checkout and active tasks after merge

Repository agents never implement in the primary `main` checkout. They create a
branch and dedicated worktree from the latest `origin/main`. After an
agent-owned merge, the agent fetches and fast-forwards the primary checkout only
when it is clean, on `main`, and non-divergent. Dirty checkouts are preserved
and reported; active task worktrees are never reset or automatically rebased.

The merge-owning agent then sends every running task/thread for this repository
an informational event containing the PR number, merge commit, and old/new
`main` SHAs. Existing tasks keep their frozen checkout until a safe checkpoint;
new tasks start from the new baseline. GitHub Actions cannot directly reach an
offline developer checkout or the Codex desktop thread API, so this final local
handoff is an agent/runtime responsibility rather than an SSH deployment step.
