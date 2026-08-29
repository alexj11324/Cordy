# Desktop Google auth broker contract

This contract coordinates the hosted accounts flow with Electron PR #627. It
is deliberately separate from the Account Portal implementation and from the
Rust API's existing email-code and direct-Google endpoints.

## Public route

The one authoritative Electron entry URL is:

```text
https://accounts.aspectlylabs.com/oauth/google?platform=desktop
```

The current live accounts Worker does not implement this broker yet; it sends
the route to the old Web login. That observed behavior is a blocker, not a
valid implementation. `/login`, `/sign-in`, `/sign-up`, SSO callback routes and
the Account Portal must continue to work unchanged.

The selected deployment shape is a narrowly scoped HTTPS route in front of a
separately versioned broker artifact. The route pattern is
`https://accounts.aspectlylabs.com/oauth/google*`; Cloudflare's route
specificity makes it take precedence over the existing `accounts` custom
domain only for the OAuth broker paths, while `/`, `/login`, `/sign-in`,
`/sign-up`, SSO callbacks, and other Portal paths remain on
`aspectlylabs-proxy`. Do not attach a second custom domain to `accounts` and do
not replace the Portal Worker. The route is intentionally not deployed until
the broker artifact, API exchange, health checks, and rollback record are
accepted together.

## Flow and security invariants

1. The entry page starts the Clerk Core 3-compatible `oauth_google` sign-in-or-up
   flow. The deployed Clerk JS/React family remains the repository's existing
   Core 3-compatible family (Next 7 / React 6), rather than introducing a new
   Clerk major version.
2. The flow uses an exact registered callback such as
   `https://accounts.aspectlylabs.com/oauth/google/callback`. State, nonce and
   PKCE are generated and checked by the Clerk flow; no wildcard callback or
   user-supplied return URL is accepted.
3. The callback page calls `Clerk.handleRedirectCallback`. It must support a
   first-time user, an existing user, a second-factor/MFA task, and any current
   session task (including `setup-mfa`, `reset-password`, and organization
   selection) before completing the handoff. A task page is not a successful
   desktop login.
4. After Clerk reports a complete session, the page sends the short-lived Clerk
   session credential to the broker over HTTPS in an `Authorization` header.
   The credential is never put in a query string, fragment, log, redirect URL,
   or custom URL scheme.
5. The broker verifies the Clerk session server-side, stores only the minimum
   identity needed for the pending exchange, and returns a cryptographically
   random, opaque, short-lived authorization code. The code record is consumed
   atomically once. A plain KV `get` followed by `delete` is not sufficient for
   this invariant because two concurrent exchanges can both read the record;
   use a Durable Object transaction or another atomic store.
6. The only deep link is:

   ```text
   patchbay://auth/callback?code=<opaque-one-time-code>
   ```

   The deep-link handler must reject `token`, `access_token`, and `id_token`
   parameters. It must never receive a bearer token or Clerk session token.
7. Electron exchanges the code with the API over HTTPS. The API consumes the
   code through the broker's server-to-server exchange and issues the existing
   Patchbay session contract. The API must not trust a browser-provided user ID
   or email and must not accept a code twice.
8. Email in Electron remains the existing `/auth/send-code` and
   `/auth/verify-code` flow. It does not go through this broker.

The MFA and session-task continuation pages are broker-owned internal paths:
`/oauth/google/sign-in` and `/oauth/google/sign-up`. They mount the existing
Clerk JS 6 task UI and return to `/oauth/google/complete?platform=desktop`;
they do not redirect to the Account Portal's `/tasks/*` paths.

## Required endpoints and response rules

| Endpoint | Caller | Required behavior |
| --- | --- | --- |
| `GET /oauth/google?platform=desktop` | Electron/browser | Start the Google flow only for the exact `desktop` platform value; reject or use a safe non-desktop response for other values |
| `GET /oauth/google/callback` | Clerk redirect | Run `handleRedirectCallback`, render continuation for MFA/session tasks, and never form a deep link until the session is complete |
| `GET /oauth/google/healthz` | Deployment monitor | Broker liveness response on the broker-owned route; do not reuse or intercept the Account Portal's health path |
| `GET /oauth/google/readyz` | Deployment monitor | Return ready only when Clerk keys, the shared secret and Durable Object binding are present |
| `GET /oauth/google/complete?platform=desktop` | Broker callback page | Continue any outstanding Clerk session task, then request the one-time code; the `desktop` marker is required |
| `POST /oauth/google/complete` | Broker callback page | Accept a Clerk session credential only over HTTPS, validate origin/CSRF as appropriate, atomically create the pending code, and return a one-use handoff |
| `POST /oauth/google/exchange` | Rust API, server-to-server | Atomically consume the code and return a minimal verified profile; do not return a bearer token from the broker |
| `POST /auth/desktop/exchange` | Electron/API client | Exchange the opaque code and issue the normal Patchbay session; never log the code or identity payload |

The exact Rust route is not implemented on current `main`; current Rust auth
only has email-code and direct Google endpoints and its middleware validates the
Patchbay JWT format, not Clerk JWTs. Therefore the broker cannot be marked live
until the API exchange and a real Rust origin are implemented and deployed.

## CSP and allowlists

The broker page must send a restrictive CSP with a per-response nonce for any
inline bootstrap script. Clerk's mounted UI may require inline styles, so
`style-src 'unsafe-inline'` is allowed for styles only; the policy must not use
`script-src 'unsafe-inline'`, `connect-src *`, or wildcard `frame-src`. The
Google/Clerk callback and `patchbay://` handoff are fixed application values,
not request-controlled URLs.

The acceptance suite must cover:

- first-user sign-in-or-up and returning-user sign-in;
- MFA and session-task continuation, cancellation and failure;
- state/nonce/PKCE mismatch and callback replay;
- code expiry, malformed code, and concurrent double exchange;
- absence of every bearer/token parameter in the deep link;
- exact `patchbay://auth/callback` parsing and API exchange;
- no regression for Account Portal routes and its existing health response.

## Deployment contract

The broker Worker uses these bindings, all supplied outside source control:

| Binding | Kind | Purpose |
| --- | --- | --- |
| `CLERK_PUBLISHABLE_KEY` | non-secret Worker variable | Clerk JS 6 browser bootstrap for the existing Core 3-compatible instance |
| `CLERK_SECRET_KEY` | Worker secret | Verify the Clerk session and read the user profile server-side |
| `BROKER_SHARED_SECRET` | Worker secret | Authenticate only the Rust API's server-to-server code exchange |
| `AUTH_CODE_STORE` | Durable Object namespace | Atomically store and consume one-time codes |

The Rust API uses `AUTH_BROKER_ORIGIN=https://accounts.aspectlylabs.com` and
`AUTH_BROKER_SHARED_SECRET` with the same secret value as
`BROKER_SHARED_SECRET`. The API rejects non-HTTPS broker origins and never
accepts a browser-supplied broker URL. The deployment must also configure the
Clerk OAuth callback allowlist for the exact callback URL above and keep
`NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY`/Clerk keys out of commits and logs.

Until the API origin, broker secrets, Durable Object deployment, the broker
scoped `/oauth/google/healthz` and `/oauth/google/readyz` checks, and all checks
above exist against a real artifact, the route remains code-only; no DNS or
custom-domain change is authorized for this route.
