# Auth Broker release boundary

The public authentication transport is `https://accounts.aspectlylabs.com`.
Its versioned contract is [`contracts/auth-broker/v1.json`](../../contracts/auth-broker/v1.json).

Desktop opens `/oauth/google` directly with a random state and S256 PKCE
challenge. Clerk returns only to `/oauth/google/callback`; the broker then asks
the Go API to mint a short-lived, one-time `pbd_` code. The custom-protocol URL
contains only that code and state. The verifier never leaves Desktop, and the
reusable Patchbay bearer is returned only by the HTTPS redeem endpoint.

The broker owns no identity or handoff persistence. Clerk is the identity
authority, while the Go API owns user resolution, attempt freshness, one-time
code storage and Patchbay session issuance. There is no Rust origin dependency.

The broker image is independently named `patchbay-auth-broker`. Release jobs
publish it by digest and the Helm/Compose contracts require an immutable image.
Runtime secrets are injected only at deployment time.
