#!/usr/bin/env bash
set -euo pipefail

# Deployment-time guard for the server-managed IM surface. This script never
# prints credential values and never calls the app API. It validates the one
# property the app cannot safely infer: that cross-device binding has a public
# HTTPS origin. Run it from the repository root or set PATCHBAY_ROOT explicitly.

ROOT_DIR="${PATCHBAY_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
MANIFEST="${PATCHBAY_MESSAGING_MANIFEST:-${ROOT_DIR}/deploy/messaging/manifest.yaml}"

fail() {
  echo "messaging self-host check: $*" >&2
  exit 1
}

[[ -f "$MANIFEST" ]] || fail "manifest not found: $MANIFEST"

manifest_version="$(awk '$1 == "version:" { print $2; exit }' "$MANIFEST")"
manifest_mode="$(awk '$1 == "mode:" { print $2; exit }' "$MANIFEST")"
manifest_app_url="$(awk '$1 == "app_url:" { print $2; exit }' "$MANIFEST")"
manifest_api_url="$(awk '$1 == "api_url:" { print $2; exit }' "$MANIFEST")"
[[ "$manifest_version" == "1" ]] || fail "manifest version must be 1"
[[ "$manifest_mode" == "server_configured" ]] || fail "manifest mode must be server_configured"
[[ "$manifest_app_url" == https://* ]] || fail "manifest public.app_url must be an HTTPS URL"
[[ "$manifest_api_url" == https://* ]] || fail "manifest public.api_url must be an HTTPS URL"

for provider in slack telegram lark dingtalk wecom weixin; do
  awk -v provider="$provider" '
    $0 ~ "^  " provider ":$" { found = 1; next }
    found && $1 == "transport:" { transport = $2; next }
    found && $1 ~ /_env:$/ { secret = $2; next }
    found && $0 !~ /^    / { exit }
    END { if (transport == "" || secret == "") exit 1 }
  ' "$MANIFEST" || fail "manifest provider entry is incomplete: $provider"
done

mode="${PATCHBAY_MESSAGING_MODE:-server_configured}"
[[ "$mode" == "server_configured" ]] || fail "PATCHBAY_MESSAGING_MODE must be server_configured"

bootstrap="${PATCHBAY_MESSAGING_BOOTSTRAP:-false}"
case "$bootstrap" in
  false|0|no) ;;
  true|1|yes)
    [[ -n "${PATCHBAY_MESSAGING_WORKSPACE_ID:-}" ]] || fail "PATCHBAY_MESSAGING_WORKSPACE_ID is required when bootstrap is enabled"
    [[ -n "${PATCHBAY_MESSAGING_INSTALLER_USER_ID:-}" ]] || fail "PATCHBAY_MESSAGING_INSTALLER_USER_ID is required when bootstrap is enabled"
    if [[ -n "${SLACK_BOT_TOKEN:-}" || -n "${SLACK_APP_TOKEN:-}" ]]; then
      [[ -n "${SLACK_BOT_TOKEN:-}" && -n "${SLACK_APP_TOKEN:-}" ]] || fail "SLACK_BOT_TOKEN and SLACK_APP_TOKEN must be configured together"
      [[ -n "${SLACK_TEAM_ID:-}" && -n "${SLACK_BOT_USER_ID:-}" ]] || fail "SLACK_TEAM_ID and SLACK_BOT_USER_ID are required for Slack bootstrap"
    fi
    ;;
  *) fail "PATCHBAY_MESSAGING_BOOTSTRAP must be true or false" ;;
esac

app_url="${PATCHBAY_APP_URL:-${FRONTEND_ORIGIN:-}}"
api_url="${PATCHBAY_PUBLIC_URL:-}"
[[ "$app_url" == https://* ]] || fail "PATCHBAY_APP_URL/FRONTEND_ORIGIN must be an HTTPS public URL"
[[ "$api_url" == https://* ]] || fail "PATCHBAY_PUBLIC_URL must be an HTTPS public URL"
# The checked-in manifest is a portable template. Keep the equality check for
# a deployment-specific manifest, but allow its documented example origins so
# an operator does not have to rewrite a tracked file just to deploy.
if [[ "$manifest_app_url" != "https://app.example.com" && "$app_url" != "$manifest_app_url" ]]; then
  fail "app URL does not match manifest public.app_url"
fi
if [[ "$manifest_api_url" != "https://api.example.com" && "$api_url" != "$manifest_api_url" ]]; then
  fail "API URL does not match manifest public.api_url"
fi

for url in "$app_url" "$api_url"; do
  host="${url#https://}"
  host="${host%%/*}"
  if [[ "$host" == \[*\]* ]]; then
    host="${host#\[}"
    host="${host%%\]*}"
    host_lower="$(printf '%s' "$host" | tr '[:upper:]' '[:lower:]')"
    case "$host_lower" in
      ""|::|::1|fc*|fd*|fe8*|fe9*|fea*|feb*)
        fail "private or loopback IPv6 host is not allowed: [$host]"
        ;;
    esac
    continue
  fi
  host="${host%%:*}"
  host_lower="$(printf '%s' "$host" | tr '[:upper:]' '[:lower:]')"
  case "$host_lower" in
    localhost|*.local|0.*|127.*|10.*|100.64.*|169.254.*|192.0.0.*|192.168.*|198.18.*|198.19.*|172.16.*|172.17.*|172.18.*|172.19.*|172.2[0-9].*|172.3[0-1].*)
      fail "private or loopback host is not allowed: $host"
      ;;
  esac
done

echo "messaging self-host check: OK"
echo "  mode: $mode"
echo "  app binding origin: $app_url"
echo "  api origin: $api_url"
echo "  manifest: $MANIFEST"
