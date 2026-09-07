# Hosted environment isolation

Patchbay follows the same three-environment split Multica uses for clients
and hosted backends: local development, an internal test stack, and the
public product. Those three never share a database, Clerk application,
cookie domain, session cookie names, desktop `userData` directory, mobile bundle id, or CLI
profile.

## Environments

| | Development | Testing (staging) | Public (production) |
| --- | --- | --- | --- |
| Audience | Contributors on a checkout | Internal QA / pre-production | Paying and public users |
| API | `http://localhost:<port>` from `make up` | `https://api.staging.aspectlylabs.com` | `https://api.aspectlylabs.com` |
| Web | `http://localhost:<port>` | `https://staging.aspectlylabs.com` | `https://patchbay.aspectlylabs.com` |
| Accounts | Local session or the production broker only when a loopback API is in use | `https://accounts.staging.aspectlylabs.com` | `https://accounts.aspectlylabs.com` |
| Desktop | **Orvilo Canary** (`userData`: Patchbay Canary, callback `patchbay-canary-<hash>://`) | **Orvilo Staging** (`userData`: Patchbay Staging, callback `patchbay-staging-<hash>://`) | **Orvilo** (`userData`: Patchbay, callback `patchbay://`) |
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
pnpm dev:desktop   # Orvilo Canary → local backend
pnpm dev:mobile    # Patchbay (Dev) → .env.development.local
```

Worktree isolation is documented in [CONTRIBUTING.md](../../CONTRIBUTING.md).
Nothing in that flow writes `~/.patchbay/config.json` or the production
Electron `userData` directory.

### Testing (staging)

```bash
pnpm dev:desktop:staging   # Orvilo Staging → api.staging.aspectlylabs.com
pnpm dev:mobile:staging    # Patchbay (Staging) → apps/mobile/.env.staging
patchbay setup self-host --profile staging \
  --server-url https://api.staging.aspectlylabs.com \
  --app-url https://staging.aspectlylabs.com
```

Desktop staging uses its own app name, `userData` path, OS callback
scheme (`patchbay-staging-<hash>://`), and renderer port, so a Canary session
against localhost cannot leak cookies or tokens into staging, both channels
can run from the same checkout without `EADDRINUSE`, staging callbacks cannot
open Canary or the packaged production app, and staging cannot leak into
production.

On macOS, each checkout/channel launches its own cached Electron app bundle
under `.patchbay-dev/electron/<version>-<arch>/`, leaving the dependency bundle
and the other channel's native callback registration untouched. Dependency
upgrades select a new cache path; ordinary starts reuse the existing copy.

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
- Cookie domain `.staging.aspectlylabs.com` and cookie names
  `patchbay_staging_auth` / `patchbay_staging_csrf` (production keeps
  `patchbay_auth` / `patchbay_csrf`).

The restricted staging gateway is `/usr/local/bin/patchbay-staging-deploy`.
It refuses production paths, production Compose project names, production
ports, and production product URLs before it mutates anything. Production
deployments continue to use `/usr/local/bin/patchbay-production-deploy` and
the `production` Environment; a staging failure cannot change production
workflow conclusion.

A separate operator-run trial exists at `patchbay-staging.nebula-spaces.com`
(`deploy/origin/nebula-staging.md`). It is a fixed image snapshot on its own
domain and Clerk app, not this Aspectly Labs staging pipeline. Do not reuse
its Compose project, Secret Manager entry, or smoke user here.

## One-time staging bootstrap

1. Create DNS for `staging.aspectlylabs.com`, `api.staging.aspectlylabs.com`,
   `accounts.staging.aspectlylabs.com`, and
   `accounts-origin.staging.aspectlylabs.com`.
   Install a dedicated TLS certificate at
   `/etc/nginx/ssl/staging.aspectlylabs.com/origin.pem` and
   `origin.key` (private key root-owned, mode 0600). The production
   `*.aspectlylabs.com` wildcard does **not** cover nested staging origin
   hosts such as `api.staging.aspectlylabs.com` or
   `accounts-origin.staging.aspectlylabs.com`. Required SANs are
   `staging.aspectlylabs.com` plus `*.staging.aspectlylabs.com`, or the
   explicit names `staging.aspectlylabs.com`,
   `api.staging.aspectlylabs.com`, and
   `accounts-origin.staging.aspectlylabs.com`. Confirm before nginx reload:

   ```bash
   openssl x509 -in /etc/nginx/ssl/staging.aspectlylabs.com/origin.pem -noout -ext subjectAltName
   ```

   Do not copy or symlink `/etc/nginx/ssl/aspectlylabs.com/origin.pem`.
   `accounts.staging.aspectlylabs.com` is terminated at Cloudflare, not this
   origin certificate. Keep Cloudflare Full (strict) validation enabled; do
   not work around a missing SAN by disabling TLS checks.
2. Create a separate Clerk application. Provision
   `staging-smoke@aspectlylabs.com` in that application only.
3. Create the GitHub Environment `staging` (selected-branch policy: `main`)
   with `STAGING_SSH_PRIVATE_KEY`, `STAGING_SSH_KNOWN_HOSTS`,
   `STAGING_SSH_HOST`, `STAGING_SSH_USER`, and the staging Clerk
   `NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY` (used by authenticated browser
   verification). The Clerk secret key stays in the origin secret files, not
   in GitHub.
4. Place mode-0600 `product-env.json` and `auth-broker-env.json` under
   `/var/lib/patchbay-staging/secrets`. They must name the staging URLs and
   ports above and must not mention production product hosts. Use cookie
   domain `.staging.aspectlylabs.com` so staging sessions are not scoped to
   the public product host. Production cookies on `.aspectlylabs.com` may
   still be *presented* to the staging hostname by the browser; the overlay
   therefore pins `AUTH_COOKIE_NAME=patchbay_staging_auth` and
   `CSRF_COOKIE_NAME=patchbay_staging_csrf` so Go does not read the older
   production JWT first. Cookie-name overrides support only the documented
   production and staging names; arbitrary names fall back to production
   because the shared browser client must recognize the CSRF cookie.
   A separate Clerk application also makes those
   production cookies unusable. Generate a staging-only
   `ORVILO_ORIGIN_AUTH_TOKEN`; do not copy the production Worker secret.
   The broker overlay pins `ORVILO_PRODUCT_ORIGIN=https://staging.aspectlylabs.com`;
   its server layout supplies this public runtime origin to every browser login
   and OAuth return path, even when reusing the production-built image.
   Include every required Compose variable, including `CORS_ALLOWED_ORIGINS`,
   `CLERK_JWT_KEY`, `CLERK_ISSUER`, and the matching desktop broker credential
   in both snapshots. Bootstrap validates the selected checkout before granting
   deployment access. `ALLOWED_EMAILS` may list individual QA addresses; the
   gateway always adds `staging-smoke@aspectlylabs.com`, while the overlay disables
   open signup and domain-wide allowlists.
5. Install the origin nginx map from `deploy/origin/nginx/aspectlylabs-origin.conf`
   (staging server blocks are in the same file, different ports). Staging HTTPS
   server blocks load `/etc/nginx/ssl/staging.aspectlylabs.com/origin.pem`; nginx
   `-t` fails closed if those files are missing. Before reloading
   nginx, install a root-owned mode-0600 snippet at
   `/etc/nginx/snippets/orvilo-staging-accounts-origin-auth.conf`. It must reject
   requests unless `$http_x_patchbay_origin_auth` equals the staging-only
   `ORVILO_ORIGIN_AUTH_TOKEN` from the broker snapshot — the same value as
   `wrangler secret put ORIGIN_AUTH_TOKEN --env staging`. Do not copy
   `/etc/nginx/snippets/patchbay-accounts-origin-auth.conf`; that file holds
   the production Worker token and would 403 staging `/readyz` before traffic
   reaches port 43101. Use an nginx `if` with `return 403` for a mismatch;
   never log or commit the token. Validate the configuration and verify that
   missing and production credentials are rejected before traffic is enabled.
   The staging overlay forces `ALLOW_SIGNUP=false`; provision QA users
   explicitly instead of allowing public registration.
6. Deploy the staging Accounts edge Worker. Wrangler environments do not
   inherit bindings, so this Worker has its own route, origin, origin-auth
   secret, and rate-limit namespaces (`21410502821` / `21410502822`) and must
   never reuse the production counters:

   ```bash
   cd deploy/cloudflare/accounts-origin-proxy
   npx wrangler secret put ORIGIN_AUTH_TOKEN --env staging
   npx wrangler deploy --env staging
   ```

   The token must match both `/var/lib/patchbay-staging/secrets` and the
   staging nginx origin-auth snippet. Deploying without `--env staging`
   would publish over the production Worker.
7. From a reviewed checkout:

   ```bash
   sudo deploy/origin/install-staging-deploy.sh /path/to/staging-deploy-key.pub
   ```

The first GitHub Actions run then creates the staging Compose projects and
empty volumes. It never inspects or reattaches production containers.
Each deployment loads all staging overlays from its validated source checkout,
so configuration and images advance together without reinstalling static files.
