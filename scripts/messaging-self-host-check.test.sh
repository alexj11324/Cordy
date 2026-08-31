#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
guard="$repo_root/scripts/messaging-self-host-check.sh"
output="$(mktemp)"
trap 'rm -f "$output"' EXIT

PATCHBAY_APP_URL=https://app.example.com \
PATCHBAY_PUBLIC_URL=https://api.example.com \
PATCHBAY_MESSAGING_MODE=server_configured \
  bash "$guard" >"$output"
grep -Fq 'messaging self-host check: OK' "$output"

if PATCHBAY_APP_URL=https://app.example.com \
  PATCHBAY_PUBLIC_URL=https://api.example.com \
  PATCHBAY_MESSAGING_MODE=server_configured \
  PATCHBAY_MESSAGING_BOOTSTRAP=true \
  bash "$guard" >"$output" 2>&1; then
  echo "bootstrap accepted without a workspace scope" >&2
  exit 1
fi

PATCHBAY_APP_URL=https://app.example.com \
PATCHBAY_PUBLIC_URL=https://api.example.com \
PATCHBAY_MESSAGING_MODE=server_configured \
PATCHBAY_MESSAGING_BOOTSTRAP=true \
PATCHBAY_MESSAGING_WORKSPACE_ID=00000000-0000-0000-0000-000000000001 \
PATCHBAY_MESSAGING_INSTALLER_USER_ID=00000000-0000-0000-0000-000000000002 \
  bash "$guard" >"$output"
grep -Fq 'messaging self-host check: OK' "$output"

PATCHBAY_APP_URL=https://app.cordy.example \
PATCHBAY_PUBLIC_URL=https://api.cordy.example \
PATCHBAY_MESSAGING_MODE=server_configured \
  bash "$guard" >"$output"
grep -Fq 'messaging self-host check: OK' "$output"

if PATCHBAY_APP_URL=https://localhost:13769 \
  PATCHBAY_PUBLIC_URL=https://api.example.com \
  PATCHBAY_MESSAGING_MODE=server_configured \
  bash "$guard" >"$output" 2>&1; then
  echo "localhost binding origin was accepted" >&2
  exit 1
fi

if PATCHBAY_APP_URL='https://[fe80::1]' \
  PATCHBAY_PUBLIC_URL=https://api.example.com \
  PATCHBAY_MESSAGING_MODE=server_configured \
  bash "$guard" >"$output" 2>&1; then
  echo "IPv6 link-local binding origin was accepted" >&2
  exit 1
fi

if PATCHBAY_APP_URL=https://app.example.com \
  PATCHBAY_PUBLIC_URL=https://api.example.com \
  PATCHBAY_MESSAGING_MODE=managed \
  bash "$guard" >"$output" 2>&1; then
  echo "managed mode was accepted by the self-host guard" >&2
  exit 1
fi

echo "messaging self-host check tests: OK"
