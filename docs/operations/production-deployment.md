# Aspectlylabs production delivery

The public product (`patchbay.aspectlylabs.com`) is one of three isolated
environments. Local development and internal staging are documented in
[environments.md](environments.md). This file is the production-only
acceptance contract.

## Goal and acceptance

Every reviewed merge to `main` must produce and deploy one coherent production
revision. Completion means all of the following are true:

1. The normal `CI` workflow succeeded for the exact `main` commit.
2. Backend, Web, Docs, and Auth Broker were all built for `linux/arm64` from
   that same full SHA and published by immutable digest.
3. The restricted server gateway accepted that SHA as the current remote
   `main` and deployed the four digests.
4. The Go API, Web, Docs, and Accounts Auth Broker all report
   `X-Patchbay-Build: sha-<full-sha>` and
   `X-Patchbay-Commit: <full-sha>`.
5. `/login` and `/docs` render with HTTP 200 through the public domain; API
   config and readiness routes return 200; the Accounts OAuth entry remains
   reachable.
6. A real headless Chromium session starts directly at
   `accounts.aspectlylabs.com/oauth/google`, registers the PKCE/state-bound
   attempt through the Go API, and reaches `accounts.google.com`. A separate
   single-use synthetic Clerk ticket completes the Accounts broker flow, redeems
   the resulting one-time code through the Go API, and renders an authenticated
   product route at the exact deployed Web build and commit.

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
  -> authenticated Chromium product acceptance
  -> success, or failure diagnostics with the Go candidate left in place
```

The matrix is only scheduling parallelism. It has no changed-path filter and no
partial-image input. Incremental BuildKit and Go module caches are performance
optimizations; every run still executes each production Dockerfile.

`workflow_run` is accepted only when the upstream workflow is `CI`, the event
was a same-repository push, the branch is `main`, and the conclusion is
successful. The resolver and server gateway both reject a SHA that is no
longer the current `origin/main`, preventing an older queued run from rolling
production backward. Production has no manual dispatch entry that can bypass
the successful-CI prerequisite.

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

The existing `NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY` environment secret is supplied
to the browser verifier. The Clerk secret key stays only in the server's
mode-0600 environment snapshot. After a successful local apply, the gateway
uses it to issue one single-use five-minute sign-in ticket and one short-lived
testing token for the synthetic user. Those values travel only in the
mode-0600 deployment receipt on the ephemeral runner; they are masked, never
uploaded, and are not stored as GitHub secrets.

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
has no digest). Re-running the installer validates and preserves existing
deployment history instead of bootstrapping over it.

Provision `production-smoke@aspectlylabs.com` once in the production Clerk
instance before enabling automatic deployment. It must remain a dedicated
synthetic formal user. No Clerk secret, testing token, sign-in ticket, JWT, or
PKCE verifier belongs in a command line or repository file. Normal deployment
creates only short-lived credentials and does not persist them in GitHub.

That bootstrap target is checked for service readiness only, because it may
predate this pipeline's complete acceptance contract (and may be the broken
version the first deployment is intended to replace). Normal deployment
performs local readiness/version checks; GitHub's authenticated browser gate
owns business-page acceptance and reports failure without mutating production
again.

The server gateway accepts a maximum 64 KiB JSON request. A deployment request
must name `alexj11324/Cordy`, the exact current 40-character `main` SHA, and <!-- legacy-brand-compat -->
exactly four allow-listed `ghcr.io/alexj11324/patchbay-*` sha256 references.
Caller-provided commands, paths, Compose options, tags such as `latest`, and
arbitrary registries are rejected.

## Go-only deployment behavior

The gateway serializes operations with a host lock. It fetches `main` into a
bare cache, creates a detached release worktree, pulls all four digests before
mutation, and then updates the existing Compose projects and ports:

- `cordy632`: Backend on `127.0.0.1:8210`, Web on `127.0.0.1:3110`, retaining <!-- legacy-brand-compat -->
  the existing PostgreSQL and uploads volumes.
- `cordy`: Docs on `127.0.0.1:4000`. <!-- legacy-brand-compat -->
- `patchbay-auth-broker`: Broker on `127.0.0.1:43100`.

The source-controlled origin Nginx configuration routes the public
`patchbay.aspectlylabs.com/docs` path (including its assets and localized
pages) directly to the Docs service on `127.0.0.1:4000`. This host-level route
is part of the public deployment contract; it must be installed together with
the matching `deploy/origin/nginx/aspectlylabs-origin.conf` rather than relying
on a container-only `DOCS_URL` value.

The gateway accepts deployment requests only. There is no rollback action and
no previous-manifest execution path. After a successful state transition, it
keeps only the current detached release worktree. Older release worktrees are
removed through the bare repository; compact JSON history remains for audit.

The Go backend is made ready before Web, so migrations finish before new Web
traffic. Every image update is followed by local readiness, build, and commit
probes. If any command, local probe, short-lived browser credential request, or
later public browser check fails, the workflow fails and the gateway stores
bounded container diagnostics. It does not restore an older image set. Recovery
is a repaired Go revision deployed through the same current-main pipeline.

Database migrations must therefore be forward-safe, retryable, and compatible
with the current production data. A destructive schema migration requires a
separately reviewed expand/migrate/contract sequence.

## Runtime and edge secrets

The installer snapshots existing container configuration rather than inventing
or rotating production credentials. Before bootstrap, the running Go backend
must already have its database/JWT settings and Clerk authority configured,
including `CLERK_SECRET_KEY`, `CLERK_JWT_KEY`, `CLERK_ISSUER`,
`CLERK_AUTHORIZED_PARTIES=https://accounts.aspectlylabs.com`, and the shared
`PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN`. The public URL settings are:

- `PATCHBAY_PUBLIC_URL=https://api.aspectlylabs.com`
- `PATCHBAY_APP_URL=https://patchbay.aspectlylabs.com`
- `FRONTEND_ORIGIN=https://patchbay.aspectlylabs.com`

The Accounts broker requires `CLERK_PUBLISHABLE_KEY`,
`PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN`, and `PATCHBAY_ORIGIN_AUTH_TOKEN`.
Cloudflare stores the same origin token as `ORIGIN_AUTH_TOKEN`. Nginx validates
it after the Cloudflare source-range gate, forwards it to the Accounts broker
for an independent constant-time check, and the broker strips it before route
handling. The private
`accounts-origin.aspectlylabs.com` transport remains Cloudflare-only and is
not a fourth product endpoint.

## Legacy infrastructure identities

Two narrowly scoped legacy identities remain: the current GitHub repository
identity used by the gateway allow-list, and the existing production Compose
project/container names used to reattach the live database and upload volumes.
The platform owner owns both residuals. The repository literals are deleted
only after an approved GitHub repository rename; the Compose/container literals
are deleted only after a separately reviewed, backed-up volume migration.

What holds those residuals in place is narrow and worth stating precisely,
because there is no repository-wide legacy-spelling scanner. The gateway's
allow-lists reject any repository, registry, or image name outside the four
pinned `ghcr.io/alexj11324/patchbay-*` entries, and
`scripts/production-deployment-contract.test.mjs` asserts that neither the
production workflow nor the origin Nginx map mentions a `patchbay.ai` domain.
Both are enforced by the `production-delivery` CI job. A legacy spelling
introduced anywhere else in the tree is not caught automatically.

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

## What automation proves, and what it cannot

The `production-delivery` job in `CI` runs the deployment path's contract
suites on every change: `node --test` over the manifest assembler, the
workflow/Dockerfile/Nginx/gateway contract, and both verifiers' pure logic,
plus `python3 -m unittest` over the restricted origin gateway. Those suites use
only the Node and Python standard libraries. They deliberately assert no
network behavior, so a green `production-delivery` proves the pipeline is
internally consistent -- not that a deployment works.

The following steps are provable only against the real production
Environment, because each needs a credential or a host that does not exist
outside it. Each fails loudly rather than degrading to a simulated success:

- GHCR publication and digest resolution: needs `packages: write` on a real
  runner. `assemble-production-manifest.mjs` rejects a missing or mismatched
  image record; the workflow rejects any digest that is not
  `sha256:<64 hex>`.
- The SSH deployment itself: needs `PRODUCTION_SSH_*`. The workflow exits
  non-zero on any missing secret before it opens a connection, and pins
  `StrictHostKeyChecking=yes`.
- The gateway's local apply and probes: need Docker, the live Compose
  projects, and the mode-0600 environment snapshot. `--check` refuses to run
  before `--bootstrap` rather than assuming a default.
- The browser acceptance run: needs the production Clerk instance, the
  `production-smoke@aspectlylabs.com` user, and
  `NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY`. `verify-production-browser.mjs`
  validates the publishable key and the deployment receipt before it launches
  Chromium, so a missing credential fails immediately instead of producing a
  browser session that silently proves nothing.
