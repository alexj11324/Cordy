# Auth broker migration and rollback

This runbook does not authorize a production change. The repository change
creates an independent artifact and a disabled deployment definition; public
DNS, Cloudflare routes, the existing accounts Worker, and current Helm ingress
remain unchanged until the operator receives immediate confirmation for the
cutover window.

## Preconditions

1. PR #645 (or an equivalent reviewed implementation) is merged and deployed
   to `api.aspectlylabs.com`. Confirm that the Rust API owns the attempt,
   completion, one-time grant, redemption, and Patchbay session boundaries.
   This broker PR does not modify or depend on #645's branch state.
2. Record the broker commit, immutable image digest, current accounts route,
   current product Web image, and current Rust API revision. Keep those values
   in the private change record; do not paste credentials or session material.
3. Place `CLERK_PUBLISHABLE_KEY` and a 64-character lowercase hexadecimal
   `PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN` in the broker deployment's existing
   Secret. Put the same broker token in the Rust API's existing Secret.
   Generate and transfer it through the approved secrets store; never print it
   in a shell command, commit it, or reuse `ORIGIN_AUTH_TOKEN`. Do not add a
   Clerk secret key to the broker workload.
4. Configure the Clerk/Google application for the exact HTTPS callback and
   authorized-party origins required by contract v1. Provider credential or
   callback-domain changes require a complete Google OAuth E2E.
5. Use Wrangler 4.36.0 or newer. Confirm the Worker configuration contains the
   distinct `DESKTOP_ATTEMPT_RATE_LIMITER` and
   `DESKTOP_COMPLETE_RATE_LIMITER` bindings, and confirm Cloudflare exposes
   both bindings before deploying. Missing bindings intentionally make the two
   POST routes return 503 without contacting the origin.

## Ordered secret provisioning and deployment

The two secrets protect different hops. `ORIGIN_AUTH_TOKEN` authenticates the
accounts Worker to the origin nginx proxy. The desktop broker token proves only
that the independent broker called Rust and permits only a skip of the shared
peer-IP limiter. It carries no user or client-IP identity. Never copy one value
into the other role.

For an existing production route, preserve `ORIGIN_AUTH_TOKEN` unless this is
an explicitly coordinated rotation. Verify only the secret name with
`wrangler secret list`; do not retrieve or log its value. For a new route or a
rotation, provision the root-owned
`/etc/nginx/snippets/patchbay-accounts-origin-auth.conf` comparison from the
approved secrets store first, keep the source-controlled
`cloudflare-only.conf` and origin-auth includes enabled, then run
`wrangler secret put ORIGIN_AUTH_TOKEN` using the Worker config, verify the
origin rejects a missing or wrong origin header, and only then deploy the
Worker.

For a **first cutover** to `accounts-origin.aspectlylabs.com`, stage the Broker
and Rust credential privately first. Confirm Broker readiness, direct-origin
403 responses without the origin token, and Rust's valid/invalid broker-token
state machine before changing the Worker's `ORIGIN` or public route. The Worker
origin switch is the cutover and must not be used as a staging step.

On the OCI origin host, deploy `deploy/origin/auth-broker.compose.yml` with an
immutable `PATCHBAY_AUTH_BROKER_IMAGE` digest. Its durable loopback publication
on `127.0.0.1:43100` is the upstream owned by the source-controlled nginx
server. Require the Compose health check and a host-side `/readyz` probe to pass
before the Worker switch. Do not substitute a transient port-forward. A Helm
deployment instead requires an explicitly provisioned durable ingress or
reverse-proxy upstream; the chart's default ClusterIP cannot satisfy the OCI
nginx loopback contract.

When the public Worker already routes through the ready Broker and the origin
gate has been verified, deploy the reviewed limiter-boundary change in this
order:

1. Deploy the accounts Worker with both rate-limit bindings. Rust's old limiter
   still applies during this temporary double-limit phase. Verify attempt and
   completion use different limiter bindings and two controlled source IPs
   reach distinct edge buckets.
2. Store the same new `PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN` in the Rust API and
   broker Secret objects, then deploy Rust. Requests without the broker header
   continue using the original peer-IP limiter; a supplied but invalid header
   is rejected and never falls back.
3. Deploy the broker last. It must be unready when its Rust broker token is
   absent, must preserve the Clerk bearer only on completion, and must replace
   any caller-supplied broker credential with the server-configured header.
4. Run the complete browser acceptance gate below and inspect redacted logs to
   confirm they contain no origin secret, broker secret, Clerk token, handoff
   code, state, or PKCE verifier.

Rollback in reverse order: deploy the previous broker first so traffic returns
to the old Rust limiter, then roll back Rust, and finally roll back the Worker
if needed. Remove the broker/Rust token only after both old components are
serving. For an origin-token rotation, restore the previous Worker
route/configuration before removing either side of the origin token.
This order keeps the Rust direct path on its original limiter throughout and
prevents a secret-removal outage.

## Stage without public traffic

1. Merge the reviewed commit and wait for **Aspectlylabs production** to build
   the complete commit-addressed image set. Record the Auth Broker digest from
   that deployment manifest. The workflow deploys the broker on the staged
   same-host port, but it does not change Cloudflare traffic.
2. Resolve the published digest. For the OCI origin, render and validate
   `deploy/origin/auth-broker.compose.yml`; for Kubernetes, render the
   independent chart with `enabled=true` and that digest.
3. Install only the selected independent Broker deployment and its Secret. The
   ordinary Patchbay release must not be upgraded as part of this step. Keep a
   Kubernetes ingress disabled only while staging; provision and verify its
   durable production transport before changing Worker traffic.
4. Through a private port-forward or internal probe, require:
   - `/healthz` returns process health and contract version 1;
   - `/readyz` returns ready only with valid runtime configuration;
   - `/v1/contract` exactly matches `contracts/auth-broker/v1.json`;
   - malformed, cross-origin, oversized, and unauthenticated completion calls
     fail closed;
   - missing/invalid broker credential fails closed before the Rust handler,
     while direct Rust requests and redemption retain the peer-IP limiter;
   - pod logs contain no Clerk bearer, grant, verifier, or secret.

## Browser acceptance gate

Before changing public traffic, exercise the candidate through a controlled
HTTPS hostname registered with the same provider policy. Record redacted
screenshots or a trace proving this visible sequence:

1. Desktop opens the broker with a fresh `state` and S256 challenge.
2. The browser reaches the real Clerk Google chooser and returns to the broker
   callback.
3. The broker completes against the deployed Rust API and the browser opens
   `patchbay://auth/callback` with only `code` and `state`.
4. Desktop validates state, redeems with its local verifier, receives a
   Patchbay session from the Rust API, and loads an authenticated frontend API
   response.
5. Reusing the grant fails and the grant expires at the configured boundary.

Static source inspection, unit tests, image creation, health checks, or green
CI do not replace this gate.

## Production cutover

Obtain immediate confirmation before any step below:

1. Freeze auth-boundary releases for the window and re-check the recorded
   revisions.
2. Route `accounts.aspectlylabs.com` to the staged broker by selecting one
   approved transport (the current Cloudflare control plane or the dedicated
   ingress). Do not create a guessed or placeholder origin.
3. Remove the accounts host from the product Web alias only as part of the same
   controlled routing change. Leave `patchbay.aspectlylabs.com` on the product
   Web service and `api.aspectlylabs.com` on the Rust API.
4. Repeat the full visible Desktop login acceptance on the canonical domains.
5. Monitor broker readiness, Rust completion/redeem status, and Desktop login
   success without recording credentials.

## Rollback

Rollback changes transport only; it must not change the version 1 protocol or
session authority.

1. Route `accounts.aspectlylabs.com` back to the recorded current Web origin
   using the preserved accounts Worker/ingress configuration.
2. Confirm `/oauth/google`, callback, Desktop custom protocol, redemption, and
   authenticated API hydration on the restored path.
3. Disable the dedicated broker ingress, then scale or uninstall its separate
   release after traffic is absent. Retain the image digest and redacted
   evidence for diagnosis.
4. Do not roll back the Rust API merely because the broker transport was
   reverted; revert an API revision only for a separately proven API failure.

The existing accounts Worker, product Web route, and production traffic are
intentionally untouched by the implementation PR so this rollback remains
available until an explicitly approved cutover.
