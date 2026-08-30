#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHART_DIR="$ROOT_DIR/deploy/helm/patchbay-auth-broker"

helm lint "$CHART_DIR"

default_render="$(helm template patchbay "$CHART_DIR")"
if [[ -n "$default_render" ]]; then
  echo "Auth broker chart must render no workload unless explicitly enabled"
  exit 1
fi

if helm template patchbay "$CHART_DIR" --set enabled=true >/dev/null 2>&1; then
  echo "Enabled auth broker must require an immutable image digest"
  exit 1
fi

rendered="$({
  helm template patchbay "$CHART_DIR" \
    --set enabled=true \
    --set-string image.digest=sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
})"

for expected in \
  'image: "ghcr.io/patchbay-ai/patchbay-auth-broker@sha256:' \
  'value: "https://api.aspectlylabs.com"' \
  'value: "https://accounts.aspectlylabs.com"' \
  'name: "patchbay-auth-broker"' \
  'key: "CLERK_PUBLISHABLE_KEY"' \
  'automountServiceAccountToken: false' \
  'readOnlyRootFilesystem: true'; do
  if ! grep -Fq -- "$expected" <<<"$rendered"; then
    echo "Missing expected auth broker Helm value: $expected"
    exit 1
  fi
done

for forbidden in 'kind: Ingress' 'D1' 'KV' 'DurableObject' 'CLERK_SECRET_KEY'; do
  if grep -Fq -- "$forbidden" <<<"$rendered"; then
    echo "Forbidden auth broker Helm value: $forbidden"
    exit 1
  fi
done

echo "auth broker helm rendering ok"
