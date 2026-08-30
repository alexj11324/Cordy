#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHART_DIR="$ROOT_DIR/deploy/helm/patchbay"

require_rendered_value() {
  local rendered=$1
  local expected=$2

  if ! grep -Fq -- "$expected" <<<"$rendered"; then
    echo "Missing expected Helm-rendered config value:"
    echo "  $expected"
    exit 1
  fi
}

reject_rendered_value() {
  local rendered=$1
  local forbidden=$2

  if grep -Fq -- "$forbidden" <<<"$rendered"; then
    echo "Forbidden Helm-rendered config value:"
    echo "  $forbidden"
    exit 1
  fi
}

helm lint "$CHART_DIR"

default_config="$(
  helm template patchbay "$CHART_DIR" \
    --show-only templates/configmap.yaml
)"
require_rendered_value "$default_config" 'PATCHBAY_VCS_INTEGRATION_ENABLED: "true"'
require_rendered_value "$default_config" 'PATCHBAY_ENTITLEMENT_POLICY_ENABLED: "false"'
require_rendered_value "$default_config" 'PATCHBAY_ENTITLEMENT_POLICY_URL: ""'
reject_rendered_value "$default_config" 'PATCHBAY_ENTITLEMENT_SERVICE_TOKEN'

if helm template patchbay "$CHART_DIR" \
  --set backend.replicas=2 >/dev/null 2>&1; then
  echo "Helm must reject multiple backend replicas without shared Redis"
  exit 1
fi

redis_backend="$(
  helm template patchbay "$CHART_DIR" \
    --show-only templates/backend.yaml \
    --set backend.replicas=2 \
    --set backend.redis.enabled=true
)"
require_rendered_value "$redis_backend" 'replicas: 2'
require_rendered_value "$redis_backend" '- name: REDIS_URL'
require_rendered_value "$redis_backend" 'key: "REDIS_URL"'

disabled_config="$(
  helm template patchbay "$CHART_DIR" \
    --show-only templates/configmap.yaml \
    --set backend.config.vcsIntegrationEnabled=false
)"
require_rendered_value "$disabled_config" 'PATCHBAY_VCS_INTEGRATION_ENABLED: "false"'

entitlement_config="$(
  helm template patchbay "$CHART_DIR" \
    --show-only templates/configmap.yaml \
    --set backend.config.entitlementPolicy.enabled=true \
    --set-string backend.config.entitlementPolicy.url=https://patchbay-cloud.internal \
    --set-string backend.config.entitlementPolicy.timeout=2s \
    --set-string backend.config.entitlementPolicy.staleGrace=10m \
    --set backend.config.entitlementPolicy.emergencyDisabled=false
)"
require_rendered_value "$entitlement_config" 'PATCHBAY_ENTITLEMENT_POLICY_ENABLED: "true"'
require_rendered_value "$entitlement_config" 'PATCHBAY_ENTITLEMENT_POLICY_URL: "https://patchbay-cloud.internal"'
require_rendered_value "$entitlement_config" 'PATCHBAY_ENTITLEMENT_POLICY_TIMEOUT: "2s"'
require_rendered_value "$entitlement_config" 'PATCHBAY_ENTITLEMENT_STALE_GRACE: "10m"'
require_rendered_value "$entitlement_config" 'PATCHBAY_ENTITLEMENT_EMERGENCY_DISABLED: "false"'
reject_rendered_value "$entitlement_config" 'PATCHBAY_ENTITLEMENT_SERVICE_TOKEN'

echo "helm config rendering ok"
