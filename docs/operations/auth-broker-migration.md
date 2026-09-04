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

Deployment order:

1. Build and publish `Dockerfile.auth-broker`; record its immutable digest.
2. Configure the Go API and broker with the same broker secret, then roll the
   Go API and deploy the broker by Compose or the disabled-by-default Helm chart.
3. Configure the Worker `ORIGIN_AUTH_TOKEN` secret and its two rate-limit
   bindings, then route `accounts.aspectlylabs.com` to the Worker.
4. Verify `/health`, `/readyz`, `/v1/contract`, and a real Desktop Google login,
   including one successful redemption and one rejected replay.

Never route browser OAuth through localhost. Loopback HTTP callbacks are
reserved for the CLI flow and are outside this broker contract.
