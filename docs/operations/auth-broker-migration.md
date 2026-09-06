# Auth Broker deployment

Required broker values:

- `PATCHBAY_API_ORIGIN=https://api.aspectlylabs.com`
- `PATCHBAY_AUTH_BROKER_ORIGIN=https://accounts.aspectlylabs.com`
- `CLERK_PUBLISHABLE_KEY`
- `PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN` (64 lowercase hexadecimal characters;
  identical in broker and Go API)
- `PATCHBAY_ORIGIN_AUTH_TOKEN` (a distinct 64-character hexadecimal secret;
  identical in the Cloudflare Worker and broker origin)

Required Go API values are `PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN`,
`CLERK_SECRET_KEY`, `CLERK_JWT_KEY`, `CLERK_ISSUER`, and
`CLERK_AUTHORIZED_PARTIES=https://accounts.aspectlylabs.com`.

Production changes follow [the production delivery pipeline](production-deployment.md):
merge to `main`, successful exact-SHA CI, four immutable images, restricted
deploy gateway, public version probes, and authenticated browser acceptance.

Never route browser OAuth through localhost. Loopback HTTP callbacks are
reserved for the CLI flow and are outside this broker contract.

## Local Desktop identity

Local API development uses the same HTTPS Accounts and Clerk identity origin.
Desktop registers `state` and an S256 challenge with its own API, then opens
Accounts `/login?platform=desktop&state=...&code_challenge=...&session_mode=local`.
The browser registers the hosted attempt, completes fresh Clerk sign-in, and
submits its Clerk session only to the same-origin Accounts completion endpoint.
The broker authenticates its request to the hosted Go API.

For local mode, Go mints a `pbl_` identity grant. Accounts returns only that code
and state through `patchbay://auth/callback`. Electron supplies its saved
verifier and state to its configured local API. With
`PATCHBAY_HOSTED_DESKTOP_IDENTITY=1` (enabled by local development scripts),
that API claims its unexpired initiation row in a transaction and exchanges
the grant against the fixed HTTPS authority at
`https://api.aspectlylabs.com/api/desktop-identity/redeem`.

The authority atomically consumes the grant with its state and PKCE challenge,
within one minute of completion. It returns only email, name, and avatar;
it never returns a production session. Local user mapping and initiation
consumption commit together before a local session is returned. If a consumed
remote grant cannot be committed locally, the user starts a fresh login.

Production `pbd_` and local `pbl_` codes hash their complete prefixed value and
are rejected by the other purpose's exchange. Production leaves the local
identity consumer disabled. No Clerk bearer, PKCE verifier, API address, or
session token belongs in the browser callback. Legacy `session_api` URLs are
rejected; initiate a new login after upgrading an old development client.

Remote staging and self-hosted APIs retain their own Accounts/product origin,
and an explicit Accounts setting takes precedence. The local identity mode is
selected only for a loopback API paired with the canonical hosted Accounts.
The existing self-hosted product-web completion path remains available.

This uses the external-browser and authorization-code/PKCE boundaries described
in [RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.html#section-8.1).
Clerk session-token `azp` identifies the browser's originating application;
it does not authenticate an arbitrary localhost recipient
([Clerk session tokens](https://clerk.com/docs/guides/sessions/session-tokens)).
