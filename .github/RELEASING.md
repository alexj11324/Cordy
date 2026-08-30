# Release runbook

## Automatic macOS ARM release

Release from a reviewed commit on `main` by creating and pushing a new semantic
version tag such as `v0.18.4`. The version-tag push first runs the normal CI
workflow. Only after that CI run succeeds does `macos-release.yml` publish the
Apple Silicon (`arm64`) desktop release. Pull-request CI and ordinary `main`
pushes never create a Release.

The automatic path publishes no Rust CLI archive, Intel macOS package, Linux or
Windows installer, container image, or Helm chart. Those assets are all
manual-only release paths.

### Required macOS release secrets

A production release is fail-closed unless the canonical repository has all of
these GitHub Actions secrets:

- `CSC_LINK`: exported Developer ID Application certificate and private key
  (`.p12`, or its base64-encoded contents)
- `CSC_KEY_PASSWORD`: password protecting that export
- `APPLE_ID`: Apple account used by `notarytool`
- `APPLE_APP_SPECIFIC_PASSWORD`: app-specific password for that account
- `APPLE_TEAM_ID`: Apple Developer team that owns the signing identity

The automatic macOS ARM job builds the Apple Silicon DMG/ZIP assets without
publishing them first, verifies the Developer ID signature and expected team,
validates the stapled notarization ticket, and requires Gatekeeper to report
`Notarized Developer ID`. Only then are the ARM assets uploaded to and
published from the draft Release.

### Manual macOS-only release

When a non-automatic macOS package is needed, run **Actions → macOS Desktop
Release → Run workflow**, enter an existing semantic version tag, and choose
`x64`, `arm64`, or `all`. This path is manual and applies the same Developer
ID/notarization/Gatekeeper gates. The ARM choice is also useful for rerunning
the automatic artifact after a transient failure.

## Manual publication for all other assets

Rust CLI archives, non-ARM desktop installers, backend/Web container images,
and the Helm chart are published only through a manual **Release** workflow
run. In **Actions → Release → Run workflow**, enter an existing semantic
version tag. The workflow checks out that exact tag and runs the full manually
requested release path; it does not run from a tag push automatically.

The manual Web image path also requires the repository Actions secret
`NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY`. It is a public browser key, but it must be
passed into `Dockerfile.web` at build time because Next.js embeds
`NEXT_PUBLIC_*` values in the client bundle. The Helm
`frontend.config.clerkPublishableKey` value must match that build-time key; the
runtime environment variable alone cannot change an already-built bundle.
The published Web image also embeds `NEXT_PUBLIC_DESKTOP_APP_ORIGIN` as
`https://app.patchbay.ai`; this exact origin is the only browser-hosted desktop
handoff destination accepted by the accounts app.

The manual **Release** workflow first applies migrations with
`patchbay-migrate`, runs every Rust workspace target, builds the server, CLI,
migration runner, and all three backfill binaries, and runs RustSec before any
publishing job starts. It then packages the requested non-macOS assets and
publishes backend/Web container images and the Helm chart. The vulnerability
scan is fail-closed by default.

Backend/Web container images and the Helm chart build native `linux/amd64` and
`linux/arm64` images, publish the versioned multi-architecture manifests, and
can optionally promote stable images to `latest`.

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
