# Nebula staging trial

This independent OCI stack runs the four image digests from production run
33940998112 (source 61a3adba7ba2d717160ce586a86b07bea9ff6251).
It does not automatically deploy new main commits or gate production.

- Web: https://patchbay-staging.nebula-spaces.com
- API: https://patchbay-staging-api.nebula-spaces.com
- Accounts: https://patchbay-staging-accounts.nebula-spaces.com
- Clerk application: Patchbay Staging (app_3IthtCgfLn1WhM4DCfzpsKZaF3G), development instance.
- Secret Manager: general-secrets-store / patchbay-nebula-staging.
- Compose project: patchbay-nebula-staging; independent pgdata and uploads volumes.
- Server installation: /home/ubuntu/patchbay-nebula-staging.
- Runtime nginx credential: /run/patchbay-nebula-staging/origin.conf.
- Service: patchbay-nebula-staging.service.

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

Restart with `sudo systemctl restart patchbay-nebula-staging`. Inspect with
`docker compose -p patchbay-nebula-staging ps` and
`journalctl -u patchbay-nebula-staging`. Stopping the oneshot service does not
stop Docker containers. To stop the trial without deleting data, use
`docker ps -q --filter label=com.docker.compose.project=patchbay-nebula-staging`
to identify the five containers, then stop those exact IDs. Do not remove volumes.

This is a Web/internal QA trial. Clerk development-instance sessions require
separate sign-in on Accounts and Web. Packaged Desktop remains production;
the PR's shared native callback scheme is not corrected by this deployment.
No production credentials, DNS records or database contents were copied.

## Verified on 2026-09-05

- All five services started on OCI; 611 migration ledger entries, 16 MB database.
- Public Web login, Docs, API readiness and Accounts readiness returned 200.
- Browser displayed the staging login form and reached the Google OAuth page.
- Dedicated staging smoke-user tickets exercised Web `/auth/clerk`, broker
  completion, PKCE redemption, `/api/me`, `/api/workspaces`, and authenticated
  onboarding. Accounts and Web used separate tickets for the same test user.
- Browser confirmed `patchbay_auth` is host-only on the staging Web hostname.
- Systemd restart and nginx configuration validation passed. Production image
  references remained identical and production Web/API probes still returned 200.
- Idle aggregate container memory after browser verification was about 451 MiB;
  host root disk retained 22 GB free. No full-host reboot was performed.
