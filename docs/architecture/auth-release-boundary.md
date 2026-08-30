# Authentication release boundary

Status: accepted for implementation; production cutover pending explicit approval

Contract: [`contracts/auth-broker/v1.json`](../../contracts/auth-broker/v1.json)

## Decision

`accounts.aspectlylabs.com` is a separately built and deployed authentication
broker. It owns the Google/Clerk browser ceremony but does not own a Patchbay
session, a one-time grant, or persistent state. `api.aspectlylabs.com` remains
the authority that verifies the authenticated identity, issues the one-time
desktop grant, redeems PKCE, and returns the Patchbay session. The product Web
application at `patchbay.aspectlylabs.com` is not on the Desktop login path.

The browser flow is:

```text
Desktop
  -> accounts.aspectlylabs.com/oauth/google?state&code_challenge
  -> accounts broker registers the binding with the Rust API
  -> Clerk/Google returns to accounts.aspectlylabs.com/oauth/google/callback
  -> accounts broker forwards the Clerk session bearer to the Rust API
  <- one-time pbd_ grant
  -> patchbay://auth/callback?code&state
  -> Desktop redeems code + local PKCE verifier at api.aspectlylabs.com
  <- Patchbay session
```

The broker is an ordinary HTTP/Next.js container with runtime origin
configuration. It does not require Cloudflare-specific storage or execution
APIs, so a future Cloudflare deployment can remain a transport choice. D1, KV,
and Durable Objects are not authorities or system-of-record storage in this
design.

## Evidence for the earliest coupling point

At baseline commit `fa389eff4`:

- `deploy/cloudflare/accounts-origin-proxy/src/index.js:1-7` explicitly calls
  the accounts hostname a transport alias and defaults it to the product Web
  origin. Lines 54-83 proxy every non-health route to that origin.
- `deploy/cloudflare/accounts-origin-proxy/wrangler.toml:6-13` binds the public
  accounts hostname to that alias and configures
  `https://origin.aspectlylabs.com` as its target.
- `deploy/helm/patchbay/templates/ingress.yaml:16-37` routes the primary Web
  host and every `frontend.additionalHosts` entry to the same frontend service
  on port 3000. `scripts/helm-config.test.sh:96-108` makes
  `accounts.aspectlylabs.com` one of those aliases.
- Commit `4e60c20e5` placed the Google browser pages under `apps/web`, so an
  ordinary product Web image currently contains the authentication ceremony.
- The Desktop boundary is already suitable: `apps/desktop/src/shared/runtime-config.ts:17-25`
  separates accounts, product Web, API, and WebSocket origins;
  `apps/desktop/src/renderer/src/pages/login-url.ts:1-7` depends only on the
  accounts HTTPS origin; and
  `apps/desktop/src/renderer/src/pages/login-handoff.ts:91-173` keeps PKCE
  locally and receives only a one-time code through the custom protocol.

The first responsible coupling is therefore deployment and ownership of the
accounts browser routes, not the Desktop protocol. Splitting only DNS or adding
another proxy would retain the same release coupling.

## Version 1 contract and authorities

| Boundary | Version 1 rule | Authority |
| --- | --- | --- |
| Identity proof | Clerk Google session, sent only in an `Authorization` header | Clerk |
| Attempt binding | `state` and PKCE `code_challenge` | Rust API |
| One-time grant | `pbd_...`, never a bearer token | Rust API |
| Desktop callback | `patchbay://auth/callback?code&state` | Desktop protocol |
| Patchbay session | Issued only after code + verifier redemption | Rust API |
| Broker state | None | N/A |

The broker exposes `/v1/desktop/google/attempt`,
`/v1/desktop/google/complete`, and `/v1/contract`. It forwards the contract
version as `x-patchbay-auth-contract-version: 1`. Breaking changes to a path,
query field, authority, PKCE rule, or token boundary require a new major
contract and a compatibility window. Additive observability fields may remain
in v1 only when v1 clients can ignore them safely.

The broker accepts browser exchange requests only from its configured exact
origin, bounds request and response sizes, does not follow redirects, does not
reflect upstream error bodies, and returns only the fields named in the
contract. The Clerk publishable key and origins are runtime configuration;
they are not compiled into the image. No Clerk secret key is present in the
broker.

## Release boundary

`Dockerfile.auth-broker`, the disabled-by-default
`deploy/helm/patchbay-auth-broker` chart, and the manual
`Auth Broker Release` workflow form an independent artifact lane. The ordinary
product `Release` workflow does not build, publish, deploy, or route this
artifact. Conversely, building the broker artifact does not deploy the Web,
Rust API, or Desktop applications.

`scripts/classify-auth-change.mjs` is the executable release-impact contract:

| Change | Broker release | Full Google OAuth E2E |
| --- | --- | --- |
| Ordinary product Web page | No | No |
| Unrelated Rust endpoint or migration | No | No |
| Desktop UI unrelated to login/runtime origins | No | No |
| Broker copy/style or image/chart mechanics | Yes | No |
| Provider flow, callback, runtime auth origins, or Clerk key | Yes | Yes |
| Versioned contract or broker/Rust session exchange | Yes | Yes |
| Desktop login URL, PKCE handoff, or custom protocol | No | Yes |

The classifier is deliberately conservative at the auth boundary. A full E2E
is still required for provider credential rotation even when the repository
diff cannot observe the external change.

## Alternatives rejected

- Keeping `accounts` as a product Web alias changes transport but not ownership
  or release coupling.
- Moving the flow into a stateful Cloudflare Worker would create a second
  session/grant authority and make a hosting choice part of the protocol.
- Sending a Patchbay bearer through the custom URL would expose it to URL and
  protocol-handler surfaces and weaken the existing one-time PKCE boundary.

## Failure conditions

Do not cut over if the Rust attempt/complete/redeem contract is not deployed,
the configured Clerk callback is not the exact accounts origin, the broker
returns any credential through a URL, the browser-to-Desktop flow has not been
observed end to end, or rollback cannot restore the current accounts-to-Web
route. CI/build evidence alone does not satisfy the browser acceptance gate.
