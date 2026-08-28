# Release runbook

## Normal release

Release from a reviewed commit on `main` by creating and pushing a new semantic
version tag such as `v0.18.4`. A tag push publishes the Rust CLI archives and
desktop installers. It does not build self-hosted container images or the Helm
chart, so desktop downloads are not blocked by server-image publication.

The verification job applies migrations with `patchbay-migrate`, runs every Rust
workspace target, builds the server, CLI, migration runner, and all three
backfill binaries, and runs RustSec before any publishing job starts. The Rust
CLI build matrix then packages release assets for every supported
OS/architecture pair. The vulnerability scan is fail-closed by default.

### Required macOS release secrets

A production release is fail-closed unless the canonical repository has all of
these GitHub Actions secrets:

- `CSC_LINK`: exported Developer ID Application certificate and private key
  (`.p12`, or its base64-encoded contents)
- `CSC_KEY_PASSWORD`: password protecting that export
- `APPLE_ID`: Apple account used by `notarytool`
- `APPLE_APP_SPECIFIC_PASSWORD`: app-specific password for that account
- `APPLE_TEAM_ID`: Apple Developer team that owns the signing identity

The macOS jobs build Intel and Apple Silicon DMG/ZIP assets without publishing
them, verify the Developer ID signature and expected team, validate the stapled
notarization ticket, and require Gatekeeper to report `Notarized Developer ID`.
Only then are the macOS assets uploaded to the draft Release. The public
Release job still waits for both macOS matrix entries, so an unsigned or
unnotarized package cannot become the production auto-update baseline.

### Manual macOS-only release

When only the macOS desktop app is needed, run **Actions → macOS Desktop
Release → Run workflow** and enter an existing semantic version tag. This path
builds only the Apple Silicon and Intel DMG/ZIP artifacts, applies the same
Developer ID/notarization/Gatekeeper gates, uploads both auto-update metadata
files, and publishes the GitHub Release. It does not build server containers,
the Web image, Helm, or non-macOS installers.

## Manual self-hosted publication

Backend/Web container images and the Helm chart are published only through a
manual **Release** workflow run. In **Actions → Release → Run workflow**, enter
an existing semantic version tag. The workflow checks out that exact tag,
builds native `linux/amd64` and `linux/arm64` images, publishes the versioned
multi-architecture manifests, and then publishes the matching Helm chart.

Select **promote_latest** only when the requested tag is a stable release and
the versioned images have been intentionally chosen as the new self-hosted
default. Pre-release tags are never promoted to `latest`.

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
longer installs or executes Go. Audit a downloaded CLI artifact with `patchbay
version --output json`; it reports the release version, commit, build time,
target OS, and target architecture.
