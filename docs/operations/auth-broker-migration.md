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
3. Place `CLERK_PUBLISHABLE_KEY` in the deployment's existing Secret. Do not
   add a Clerk secret key to this workload.
4. Configure the Clerk/Google application for the exact HTTPS callback and
   authorized-party origins required by contract v1. Provider credential or
   callback-domain changes require a complete Google OAuth E2E.

## Stage without public traffic

1. Run **Auth Broker Release (manual)** for the reviewed commit and build an
   versioned image tag. Publishing the image does not deploy it.
2. Resolve the published digest and render the independent chart with
   `enabled=true` and that digest. Keep `ingress.enabled=false`.
3. Install the chart under its own Helm release name and Secret. The ordinary
   Patchbay release must not be upgraded as part of this step.
4. Through a private port-forward or internal probe, require:

   - `/healthz` returns process health and contract version 1;
   - `/readyz` returns ready only with valid runtime configuration;
   - `/v1/contract` exactly matches `contracts/auth-broker/v1.json`;
   - malformed, cross-origin, oversized, and unauthenticated completion calls
     fail closed;
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
