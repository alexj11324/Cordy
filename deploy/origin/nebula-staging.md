# Nebula staging trial

This independent OCI stack runs the four image digests from production run
34172677715, successful attempt 3 (source
83df56a36b50a9971dd3e673ae0aa4d4b7cf5d50, the PR #779 merge).
It does not automatically deploy new main commits or gate production.

- Web: https://orvilo-staging.nebula-spaces.com
- API: https://orvilo-staging-api.nebula-spaces.com
- Accounts: https://orvilo-staging-accounts.nebula-spaces.com
- Clerk application: Orvilo Staging (app_3IthtCgfLn1WhM4DCfzpsKZaF3G), development instance.
- Secret Manager: general-secrets-store / orvilo-nebula-staging.
- Compose project: orvilo-nebula-staging; independent pgdata and uploads volumes.
- Server installation: /home/ubuntu/orvilo-nebula-staging.
- Runtime nginx credential: /run/orvilo-nebula-staging/origin.conf.
- Service: orvilo-nebula-staging.service.

The Web proxies API traffic on the same origin and uses host-only session
cookies. All three public hosts are outside the production cookie domain.
Each container has CPU, memory and PID limits; their combined memory ceiling
is 2560 MiB. PostgreSQL has no published port.

Install the Compose file, starter and the exact manifest.json in the server
installation directory, using the committed `nebula-staging.manifest.json`
as the manifest (retain that filename). Install `nebula-staging-realip.conf`
under `/etc/nginx/snippets/` before validating the nginx configuration.
The starter fetches credentials into process memory;
only nginx's transport token is rendered into the private runtime directory.
The systemd service starts after nginx and reloads it after successful startup.
The wildcard nginx include permits production nginx to start even when staging
cannot retrieve its credential.

Startup probes all four applications for HTTP 200 and the snapshot commit
before publishing the nginx credential and reporting success. Web/API ingress
allows 101 MiB requests for the product's 100 MiB upload limit and multipart
overhead. Cloudflare client addresses are accepted only from published proxy
ranges, nginx overwrites forwarded IP headers, and Go trusts only the discovered
Docker network gateway. Web `/api/` traffic goes directly to Go to preserve that
single trusted proxy hop. Update the real-IP ranges from Cloudflare's published
`https://www.cloudflare.com/ips-v4` and `https://www.cloudflare.com/ips-v6` lists.

Restart with `sudo systemctl restart orvilo-nebula-staging`. Inspect with
`docker compose -p orvilo-nebula-staging ps` and
`journalctl -u orvilo-nebula-staging`. Stopping the oneshot service does not
stop Docker containers. To stop the trial without deleting data, use
`docker ps -q --filter label=com.docker.compose.project=orvilo-nebula-staging`
to identify the five containers, then stop those exact IDs. Do not remove volumes.

This is a Web/internal QA trial. Clerk development-instance sessions require
separate sign-in on Accounts and Web. Authenticated staging browser acceptance
for the current snapshot passed for the Clerk development email OTP flows
documented below, within the stated session and navigation limits.

## Migration verified on 2026-09-07 (America/New_York)

- The snapshot uses the exact `production-manifest.json` artifact from
  [production run 34172677715, attempt 3](https://github.com/alexj11324/Cordy/actions/runs/34172677715/attempts/3),
  which succeeded after PR #779 merged as
  `83df56a36b50a9971dd3e673ae0aa4d4b7cf5d50`.
  The committed `nebula-staging.manifest.json` is byte-for-byte identical to
  that artifact. Its accompanying SHA-256 checksum was verified:
  `1bf744df7671e0246986fee4d5303626859de8a60bc2ffbe1b2ab056592e0332`.
- The existing Clerk application `app_3IthtCgfLn1WhM4DCfzpsKZaF3G` was renamed
  **Orvilo Staging**. Its identity and credentials were preserved.
- Before applying the new application migrations, all 137 compared database
  tables matched in both row counts and contents. This comparison records
  preservation at that boundary; it is not a post-migration equality claim.
- Four public probes on the flat Orvilo Nebula hosts returned HTTP 200 with
  source `83df56a36b50a9971dd3e673ae0aa4d4b7cf5d50`: Web `/login` and Docs
  `/docs` on `orvilo-staging.nebula-spaces.com`, API `/readyz` on
  `orvilo-staging-api.nebula-spaces.com`, and Accounts `/readyz` on
  `orvilo-staging-accounts.nebula-spaces.com`.

## Follow-up browser acceptance and old-stack retirement

This evidence applies to the current snapshot above and is separate from the
September 5 verification.

- Native browser Web sign-in with the Clerk development test email
  `migration+clerk_test@nebula-spaces.com` and documented test OTP `424242`
  completed and landed on `/onboarding`. On reload, CDP
  `Network.responseReceived` captured HTTP 200 for both `/auth/clerk` and
  `/api/workspaces` on `orvilo-staging.nebula-spaces.com`.
- Native browser Accounts email OTP login at
  `https://orvilo-staging-accounts.nebula-spaces.com/login` also completed and
  returned to Orvilo staging `/onboarding`. The existing Web session was already
  signed in; this verifies the observed return navigation, not cross-origin
  session transfer.
- These checks do not establish Google consent, a native application callback,
  or Resend delivery acceptance.
- The three old Patchbay Nebula A records were deleted after exact record ID
  and name validation. The old nginx enabled symlink was moved to
  `/home/ubuntu/orvilo-nebula-migration-20260908/patchbay-nginx-enabled-link`;
  `nginx -t` and reload passed.
- The old service was disabled and all five old containers were stopped. The
  new `orvilo-nebula-staging.service` was enabled and active. Old database and
  uploads volumes and final backups were retained.

## Verified on 2026-09-05

The following evidence belongs to production run 33940998112, source
`61a3adba7ba2d717160ce586a86b07bea9ff6251`, before the current migration.
At that time, packaged Desktop remained production and this deployment did not
correct the PR's shared native callback scheme. No production credentials, DNS
records or database contents were copied for that trial.

- All five services started on OCI; 611 migration ledger entries, 16 MB database.
- Public Web login, Docs, API readiness and Accounts readiness returned 200.
- Browser displayed the staging login form and reached the Google OAuth page.
- Dedicated staging smoke-user tickets exercised Web `/auth/clerk`, broker
  completion, PKCE redemption, `/api/me`, `/api/workspaces`, and authenticated
  onboarding. Accounts and Web used separate tickets for the same test user.
- Browser confirmed `orvilo_auth` is host-only on the staging Web hostname.
- Systemd restart and nginx configuration validation passed. Production image
  references remained identical and production Web/API probes still returned 200.
- Idle aggregate container memory after browser verification was about 451 MiB;
  host root disk retained 22 GB free. No full-host reboot was performed.
