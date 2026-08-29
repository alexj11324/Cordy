# Patchbay public domain matrix

This is the deployment contract for Patchbay by Aspectly Labs. It records the
intended public URL shape, the evidence we have for each target, and the gate
before a DNS or Cloudflare custom-domain change is allowed.

Inventory date: 2026-08-29 (America/New_York).

## Frozen v1 shape

The browser-facing product uses one tidy public host for product navigation,
while identity and machine-facing protocols keep their own trust boundaries:

```text
patchbay.aspectlylabs.com/                 product homepage
patchbay.aspectlylabs.com/<workspace>/...  authenticated Web app
patchbay.aspectlylabs.com/docs/...         Fumadocs through the Web edge
patchbay.aspectlylabs.com/downloads        stable plural alias
patchbay.aspectlylabs.com/status           status page candidate (provider required)
accounts.aspectlylabs.com/oauth/google    desktop OAuth broker entry
api.aspectlylabs.com/api/...               Rust API (future, origin required)
api.aspectlylabs.com/ws                   Rust WebSocket (future, origin required)
api.aspectlylabs.com/api/github/setup      GitHub installation callback entry
api.aspectlylabs.com/api/webhooks/github   GitHub webhook receiver
```

`docs.aspectlylabs.com` is an implementation/backing-host candidate for the
Fumadocs Worker. It is not the canonical URL: the canonical and hreflang URLs
remain under `patchbay.aspectlylabs.com/docs`. The Web app's `DOCS_URL` must not
be set until that backing origin has a verified artifact and health check.

## Authoritative matrix

| External URL | Responsibility | Repository/deployment target | Current status | TLS | Auth boundary | CORS / redirects | Owner and acceptance |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `https://patchbay.aspectlylabs.com/*` | Product homepage and unified Web entry | `apps/web`, intended Cloudflare Worker `patchbay-web` | Code exists; Cloudflare artifact/config is being prepared; DNS/custom domain missing | Cloudflare-managed after custom domain creation; no certificate currently attached | Clerk protects authenticated app routes; landing, docs, health and download routes are public | Same-origin browser navigation. API/WS remain explicit runtime origins; do not use parent-domain cookies | Web owner. Verify DNS, HTTPS, `/`, `/healthz`, `/docs`, `/docs/healthz`, `/downloads` 308 to `/download`, robots, sitemap and manifest before binding |
| `https://patchbay.aspectlylabs.com/docs/*` | Canonical public documentation URL | Existing `apps/docs` Fumadocs app, reached through Web rewrite | Code exists; no verified docs upstream | Inherited from Web edge for the public URL | Public; no Clerk dependency in Fumadocs | Server-side rewrite to `DOCS_URL` only after a real docs origin exists; no browser CORS dependency | Docs owner. Verify English default route, `/docs/zh`, `/docs/ko`, `/docs/ja`, canonical, all real hreflang variants, sitemap and robots |
| `https://docs.aspectlylabs.com/docs/*` | Optional direct backing origin for Fumadocs | Intended Cloudflare Worker `patchbay-docs` | Candidate only; no DNS, Pages project or verified upstream | Cloudflare-managed only after a custom domain is explicitly attached | Public; no auth cookies | Must not become the canonical URL; Web rewrite keeps the browser on `/docs` | Docs/platform owner. Create only after the standalone artifact serves `/docs/healthz` and a rollback version is recorded |
| `https://patchbay.aspectlylabs.com/download` | Existing release-backed download page | `apps/web/app/(landing)/download` | Code exists; upstream is GitHub Releases at request time, not a separate download service | Web edge | Public | No CORS; GitHub API is server-side | Web owner. Verify page renders with a release response and degraded/error state without inventing an installer URL |
| `https://patchbay.aspectlylabs.com/downloads` | Stable plural product alias | `apps/web/app/(landing)/downloads` | Code-only alias; no separate origin | Web edge | Public | Permanent redirect to `/download` | Web owner. Verify exact 308/redirect target and that no second release fetch path was created |
| `https://patchbay.aspectlylabs.com/status` | Unified status-page candidate | No status provider or maintained status data source was found | **Missing/blocker**: do not add a route or placeholder response until a real provider is selected | Web edge only after a real status implementation exists | Public | No CORS; no redirect or fake success response | Platform owner. Select and monitor a real status provider, then verify incident data, cache behavior and failure/degraded states before adding this path |
| `https://accounts.aspectlylabs.com/*` | Clerk Account Portal and auth-only entrypoint | Existing Worker `aspectlylabs-proxy`, custom domain `accounts.aspectlylabs.com` | **Live**: current Worker returns the auth page and `/health` returns `ok`; this is the only live application Worker observed | Existing Cloudflare custom-domain TLS | Auth UI only. Never use this host as the GitHub App Homepage or as a Web/API proxy | Existing script currently contains the old Web origin; updating it is gated on the verified Web origin and broker artifact. Do not move or replace the Portal | Identity owner. Preserve `/`, `/login`, `/sign-in`, `/sign-up`, SSO callback paths. Separately verify `/oauth/google?platform=desktop` and that no bearer reaches `patchbay://` |
| `https://accounts.aspectlylabs.com/oauth/google?platform=desktop` | Electron Google sign-in-or-up broker entry | Separately versioned `patchbay-auth-broker` Worker on the narrower `https://accounts.aspectlylabs.com/oauth/google*` route; existing `aspectlylabs-proxy` remains the custom-domain origin for all other paths | **Code-only / missing live route**: artifact, Durable Object binding, and route declaration exist; current live Worker still redirects this path to old Web login | Existing accounts TLS; route reuses the existing hostname certificate | Clerk Core 3-compatible browser flow; state/nonce/PKCE; MFA/session tasks; only a short-lived opaque code in the deep link | Exact callback allowlist; no wildcard return URL; broker CORS only for its own HTTPS page, never `*` with credentials | Identity + desktop owners. Verify broker `/oauth/google/healthz` and `/oauth/google/readyz`, Portal regression, first-user, existing-user, MFA/task continuation, replay rejection, expiry and App/API exchange before deploying the route |
| `https://api.aspectlylabs.com/api/*` | Rust API | `server-rs` production backend | **Missing/blocker**: no DNS, deploy, or verified Rust origin was found | Not provisioned | Own Patchbay auth/JWT or an explicitly trusted identity exchange; do not assume Clerk JWTs are accepted by current middleware | Exact `FRONTEND_ORIGIN=https://patchbay.aspectlylabs.com`; credentials and CSRF rules must be tested; no broad origin list | Backend/platform owner. First deploy a real Rust origin, then verify `/api/health`, authenticated API, CORS preflight and error behavior |
| `wss://api.aspectlylabs.com/ws` | Rust WebSocket | `server-rs` WebSocket endpoint | **Missing/blocker**: same missing Rust origin | Not provisioned | Same API session boundary; token must not be placed in the URL | Verify allowed `Origin`, cookie/token handshake and reconnect behavior independently of HTTP CORS | Backend/platform owner. Verify a real workspace event round trip; do not mark complete from an HTTP 200 |
| `https://api.aspectlylabs.com/api/github/setup` | GitHub App installation/setup callback | Rust API route `GET /api/github/setup` | Code-only; no public API origin | Not provisioned | GitHub state/HMAC and authenticated Patchbay setup flow as implemented by Rust | Redirects to the Web settings URL derived from the verified Web origin; exact GitHub callback URL | Backend/GitHub owner. Verify state creation, GitHub callback, replay rejection and final Web redirect |
| `https://api.aspectlylabs.com/api/webhooks/github` | GitHub App webhook receiver | Rust API route `POST /api/webhooks/github` | Code-only; no public API origin | Not provisioned | GitHub HMAC signature and replay protection; no browser auth | No browser CORS requirement. Preserve raw body, reject invalid signatures and enforce payload limit | Backend/GitHub owner. Verify valid signature `202`, invalid/missing signature `401`, missing secret `503`, and replay handling |
| `https://hooks.aspectlylabs.com/github` | Possible dedicated webhook host | No verified repository route or deployment target | **Not selected / missing** | Not provisioned | Would be GitHub-signature-only | Would require a separate receiver and fixed URL migration | Do not create. Keep the API route until a real isolated webhook service exists and a migration is approved |
| `https://downloads.aspectlylabs.com/*` | Possible download CDN | No artifact or storage origin found | **Missing** | Not provisioned | Public signed/release URLs would need an owner | No DNS or placeholder response | Do not create; use the existing Web release page and GitHub release assets |
| `https://status.aspectlylabs.com/*` | Possible status page | No status provider or Worker found | **Missing** | Not provisioned | Public | No DNS or placeholder response | Do not create until a real status provider is selected and monitored |

The apex `aspectlylabs.com` is deliberately not a target in this matrix. The
observed zone had no A/AAAA/CNAME record for the apex. Cloudflare Anycast
addresses and the `accounts` placeholder AAAA (`100::`) are not application
origins and must never be copied to the apex or to a new subdomain.

## GitHub App values to use after the API origin exists

These are the authoritative values for the GitHub App form. Creating or
submitting the App is outside this task and still requires the user's
immediate confirmation. The setup and webhook values intentionally stay on
the API host; `accounts` is auth-only and `patchbay` is not a machine receiver.

| GitHub App field | Value |
| --- | --- |
| Homepage URL | `https://patchbay.aspectlylabs.com/` |
| Callback URL | Leave blank; the current integration does not use GitHub OAuth callbacks |
| Setup URL | `https://api.aspectlylabs.com/api/github/setup` |
| Redirect on update | Enabled |
| Webhook URL | `https://api.aspectlylabs.com/api/webhooks/github` |
| Webhook secret | Generate and store in the deployment secret store; never commit it |

Required repository permissions are Metadata, Pull requests, Checks and
Commit statuses, all read-only. Subscribe to Pull request, Check suite, Check
run and Status events. The API origin must be live and its setup/webhook
contract verified before these values are submitted; until then these URLs are
authoritative configuration, not reachable endpoints.

## What may be combined

The homepage, authenticated Web routes, docs presentation, and download alias
can share the `patchbay` Web edge because they are browser navigation surfaces
and can be versioned as one frontend artifact. `/docs` is still a separate
Fumadocs build behind a server-side rewrite so its content tree and build can be
rolled back independently. If a direct docs custom domain is not needed, the
same Worker may serve a future combined artifact; that choice requires a real
build result and must not be inferred from a DNS record.

Auth, API/WS, and GitHub webhooks remain separate responsibilities:

- Auth has a different secret and cookie boundary, and an Account Portal outage
  must not expose or rewrite product/API traffic.
- API and WS have different rate-limit, connection-lifetime, CSRF and origin
  requirements. A shared parent cookie such as `.aspectlylabs.com` would make
  accidental credential scope wider, so the browser uses explicit auth/token
  contracts instead.
- GitHub webhook requests are machine-to-machine signed requests. They do not
  need browser CORS, and routing them through a marketing/Web Worker would
  couple GitHub delivery to frontend deploys and make raw-body signature
  verification harder to audit.

Cloudflare edge routing could eventually proxy these services under tidy paths,
but only after every upstream is real, independently health-checked, and
covered by a route-level rate-limit and rollback plan. It is not a substitute
for an origin, and no such proxy route is enabled by this change.

## Before-state and safe change gate

The observed `aspectlylabs.com` zone contained only five records: Clerk DNS
records (`clerk`, two DKIM records and `clkmail`) plus proxied
`AAAA accounts.aspectlylabs.com -> 100::`. The only Worker was
`aspectlylabs-proxy`, attached to the `accounts` custom domain. There were no
Cloudflare Pages projects and no verified Web, docs or Rust API custom domain.

Before creating a record or binding a custom domain, the operator must capture:

1. The exact zone, target Worker/project, artifact digest and health response.
2. The current DNS record and custom-domain attachment, including the accounts
   Worker version; unrelated accounts records must remain unchanged.
3. The environment contract and rollback version ID.
4. HTTPS checks for the new origin before the custom domain is attached.

Rollback is a Worker version rollback (`wrangler rollback <version-id>`),
followed by restoring the captured custom-domain/DNS state if the attachment
itself was changed. Never use `wrangler deploy --delete-vars`, detach the
accounts custom domain, or point a hostname at an unverified origin as a
rollback shortcut.

## Environment contract

Values below are names and invariants, not values to commit:

| Component | Required runtime contract |
| --- | --- |
| Web Worker | `NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY` is a browser-visible deployment variable; `DOCS_URL` is unset until `docs.aspectlylabs.com` is live; `NEXT_PUBLIC_API_URL` and `NEXT_PUBLIC_WS_URL` remain unset until the Rust origin is live; `NEXT_PUBLIC_APP_VERSION` identifies the artifact |
| Docs Worker | No auth or API secret; Fumadocs owns canonical generation and language routes; `SITE_ORIGIN` is the unified Web host in source |
| Rust API | `FRONTEND_ORIGIN=https://patchbay.aspectlylabs.com`, explicit CORS/CSRF configuration, database/secrets from the existing secret store, and a separately verified WebSocket origin |
| Existing accounts Worker | Preserve its current Clerk publishable-key setup and Portal routes; change `APP_ORIGIN` only after Web HTTPS is verified; do not copy secrets into this repository |
| Desktop OAuth broker | `CLERK_PUBLISHABLE_KEY` variable, `CLERK_SECRET_KEY` and `BROKER_SHARED_SECRET` Worker secrets, exact callback allowlist, and a Durable Object for one-time codes; Rust API uses `AUTH_BROKER_ORIGIN=https://accounts.aspectlylabs.com` plus matching `AUTH_BROKER_SHARED_SECRET`; all secrets remain in secret-manager/Cloudflare bindings, never source files or deep links |
| CI/deploy | `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN` are CI credentials; Wrangler config keeps dashboard vars (`keep_vars`) and intentionally contains no account ID or secret |

No secret is present in this document, the repository, or the intended deep
link. A missing value blocks deployment rather than receiving a guessed
placeholder.
