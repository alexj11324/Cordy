# Hosted environment isolation

Patchbay follows the same three-environment split Multica uses for clients
and hosted backends: local development, an internal test stack, and the
public product. Those three never share a database, Clerk application,
cookie domain, desktop `userData` directory, mobile bundle id, or CLI
profile.

## Environments

| | Development | Testing (staging) | Public (production) |
| --- | --- | --- | --- |
| Audience | Contributors on a checkout | Internal QA / pre-production | Paying and public users |
| API | `http://localhost:<port>` from `make up` | `https://api.staging.aspectlylabs.com` | `https://api.aspectlylabs.com` |
| Web | `http://localhost:<port>` | `https://staging.aspectlylabs.com` | `https://patchbay.aspectlylabs.com` |
| Accounts | Local session or the production broker only when a loopback API is in use | `https://accounts.staging.aspectlylabs.com` | `https://accounts.aspectlylabs.com` |
| Desktop | **Patchbay Canary** | **Patchbay Staging** | **Patchbay** |
| Mobile | `ai.patchbay.mobile.dev` | `ai.patchbay.mobile.staging` | `ai.patchbay.mobile` |
| CLI | worktree profile under `~/.patchbay/profiles/dev-*` | `patchbay --profile staging` | default `~/.patchbay/config.json` |
| GitHub Environment | none | `staging` | `production` |
| Official-cloud policy | no | no | yes (`patchbay.aspectlylabs.com` only) |

The machine-readable copy of these URLs and ports is
`deploy/origin/hosted-environments.json`. Client env files, origin nginx, and
the staging gateway must match it. Contract tests fail the build when they
drift.

Staging is not a public support environment. Do not send customers there, do
not point the packaged desktop app at it, and do not treat it as Patchbay
Cloud: daemon auto-update defaults, managed-cloud setup URLs, and
capacity/billing gates stay on the production frontend host alone.

## How to use each one

### Development

```bash
make up            # isolated DB, ports, CLI profile, optional Desktop
pnpm dev:desktop   # Patchbay Canary → local backend
pnpm dev:mobile    # Patchbay (Dev) → .env.development.local
```

Worktree isolation is documented in [CONTRIBUTING.md](../../CONTRIBUTING.md).
Nothing in that flow writes `~/.patchbay/config.json` or the production
Electron `userData` directory.

### Testing (staging)

```bash
pnpm dev:desktop:staging   # Patchbay Staging → api.staging.aspectlylabs.com
pnpm dev:mobile:staging    # Patchbay (Staging) → apps/mobile/.env.staging
patchbay setup self-host --profile staging \
  --server-url https://api.staging.aspectlylabs.com \
  --app-url https://staging.aspectlylabs.com
```

Desktop staging uses its own app name and `userData` path, so a Canary
session against localhost cannot leak cookies or tokens into staging, and
staging cannot leak into the packaged production app.

### Public (production)

Packaged Desktop, `pnpm ios:mobile:device:prod:release`, and the default CLI
profile talk only to `api.aspectlylabs.com` /
`patchbay.aspectlylabs.com` / `accounts.aspectlylabs.com`. Merges to `main`
deploy this stack through the `production` GitHub Environment. See
[production-deployment.md](production-deployment.md).

## Origin isolation

Staging may share the production host, but it must not share runtime state:

- Compose projects `patchbay-staging`, `patchbay-staging-docs`, and
  `patchbay-staging-auth-broker` — never `cordy632`, `cordy`, or
  `patchbay-auth-broker`.
- Loopback ports 8211 / 3111 / 4001 / 43101 — never 8210 / 3110 / 4000 /
  43100.
- State directory `/var/lib/patchbay-staging` — never
  `/var/lib/patchbay-production`.
- A dedicated Clerk application and `staging-smoke@aspectlylabs.com` user.
- Cookie domain `.staging.aspectlylabs.com`.

The restricted staging gateway is `/usr/local/bin/patchbay-staging-deploy`.
It refuses production paths, production Compose project names, production
ports, and production product URLs before it mutates anything. Production
deployments continue to use `/usr/local/bin/patchbay-production-deploy` and
the `production` Environment; a staging failure cannot change production
workflow conclusion.

## One-time staging bootstrap

1. Create DNS for `staging.aspectlylabs.com`, `api.staging.aspectlylabs.com`,
   `accounts.staging.aspectlylabs.com`, and
   `accounts-origin.staging.aspectlylabs.com`.
2. Create a separate Clerk application. Provision
   `staging-smoke@aspectlylabs.com` in that application only.
3. Create the GitHub Environment `staging` (selected-branch policy: `main`)
   with `STAGING_SSH_PRIVATE_KEY`, `STAGING_SSH_KNOWN_HOSTS`,
   `STAGING_SSH_HOST`, and `STAGING_SSH_USER`. Clerk keys stay in the origin
   secret files, not in GitHub.
4. Place mode-0600 `product-env.json` and `auth-broker-env.json` under
   `/var/lib/patchbay-staging/secrets`. They must name the staging URLs and
   ports above and must not mention production product hosts. Use cookie
   domain `.staging.aspectlylabs.com` so staging sessions are not scoped to
   the public product host. Production cookies on `.aspectlylabs.com` may
   still be *presented* to the staging hostname by the browser; a separate
   Clerk application makes those cookies unusable.
5. Install the origin nginx map from `deploy/origin/nginx/aspectlylabs-origin.conf`
   (staging server blocks are in the same file, different ports).
6. From a reviewed checkout:

   ```bash
   sudo deploy/origin/install-staging-deploy.sh /path/to/staging-deploy-key.pub
   ```

The first GitHub Actions run then creates the staging Compose projects and
empty volumes. It never inspects or reattaches production containers.
