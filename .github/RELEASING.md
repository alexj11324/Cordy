# Release runbook

## Normal release

Release from a reviewed commit on `main` by creating and pushing a new semantic
version tag such as `v0.18.4`. The Release workflow intentionally has no manual
trigger: a tag push is the only event that can publish binaries, desktop
installers, container images, and the Helm chart.

The verification job applies migrations with `cordy-migrate`, runs every Rust
workspace target, builds the server, CLI, migration runner, and all three
backfill binaries, and runs RustSec before any publishing job starts. The Rust
CLI build matrix then packages release assets for every supported
OS/architecture pair. The vulnerability scan is fail-closed by default.

## Emergency vulnerability-scan bypass

Use the bypass only when RustSec or its live advisory database is unavailable,
or when maintainers have documented a confirmed false positive that blocks an
urgent release. Never use it to publish a release with an unresolved
vulnerability.

1. Record the reason and maintainer approval in the release issue or pull
   request, and confirm no other release is in progress.
2. In **Settings → Secrets and variables → Actions → Variables**, set the
   repository variable `ALLOW_VULN_BYPASS_FOR_TAG` to the exact release tag,
   for example `v0.18.4`.
3. Re-run the failed Release workflow for that tag. A different tag, an empty
   value, or any typo keeps the scan enabled.
4. Confirm the verification log contains the explicit bypass warning and retain
   the workflow URL in the incident record.
5. Delete `ALLOW_VULN_BYPASS_FOR_TAG` immediately after the release run
   completes. The tag-scoped value prevents a concurrent release with another
   tag from inheriting the bypass.

The release and container assets are Rust binaries and the release workflow no
longer installs or executes Go. Audit a downloaded CLI artifact with `cordy
version --output json`; it reports the release version, commit, build time,
target OS, and target architecture.
