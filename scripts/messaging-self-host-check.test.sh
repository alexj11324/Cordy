#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
guard="$repo_root/scripts/messaging-self-host-check.sh"
output="$(mktemp)"
trap 'rm -f "$output"' EXIT

ORVILO_APP_URL=https://app.example.com \
ORVILO_PUBLIC_URL=https://api.example.com \
ORVILO_MESSAGING_MODE=server_configured \
  bash "$guard" >"$output"
grep -Fq 'messaging self-host check: OK' "$output"

if ORVILO_APP_URL=https://app.example.com \
  ORVILO_PUBLIC_URL=https://api.example.com \
  ORVILO_MESSAGING_MODE=server_configured \
  ORVILO_MESSAGING_BOOTSTRAP=true \
  bash "$guard" >"$output" 2>&1; then
  echo "bootstrap accepted without a workspace scope" >&2
  exit 1
fi

ORVILO_APP_URL=https://app.example.com \
ORVILO_PUBLIC_URL=https://api.example.com \
ORVILO_MESSAGING_MODE=server_configured \
ORVILO_MESSAGING_BOOTSTRAP=true \
ORVILO_MESSAGING_WORKSPACE_ID=00000000-0000-0000-0000-000000000001 \
ORVILO_MESSAGING_INSTALLER_USER_ID=00000000-0000-0000-0000-000000000002 \
  bash "$guard" >"$output"
grep -Fq 'messaging self-host check: OK' "$output"

ORVILO_APP_URL=https://app.customer.example \
ORVILO_PUBLIC_URL=https://api.customer.example \
ORVILO_MESSAGING_MODE=server_configured \
  bash "$guard" >"$output"
grep -Fq 'messaging self-host check: OK' "$output"

if ORVILO_APP_URL=https://localhost:13769 \
  ORVILO_PUBLIC_URL=https://api.example.com \
  ORVILO_MESSAGING_MODE=server_configured \
  bash "$guard" >"$output" 2>&1; then
  echo "localhost binding origin was accepted" >&2
  exit 1
fi

if ORVILO_APP_URL='https://[fe80::1]' \
  ORVILO_PUBLIC_URL=https://api.example.com \
  ORVILO_MESSAGING_MODE=server_configured \
  bash "$guard" >"$output" 2>&1; then
  echo "IPv6 link-local binding origin was accepted" >&2
  exit 1
fi

if ORVILO_APP_URL=https://app.example.com \
  ORVILO_PUBLIC_URL=https://api.example.com \
  ORVILO_MESSAGING_MODE=managed \
  bash "$guard" >"$output" 2>&1; then
  echo "managed mode was accepted by the self-host guard" >&2
  exit 1
fi

if ORVILO_APP_URL=https://app.example.com \
  ORVILO_PUBLIC_URL=https://api.example.com \
  ORVILO_MESSAGING_MODE=server_configured \
  ORVILO_MESSAGING_BOOTSTRAP=true \
  ORVILO_MESSAGING_WORKSPACE_ID=00000000-0000-0000-0000-000000000001 \
  ORVILO_MESSAGING_INSTALLER_USER_ID=00000000-0000-0000-0000-000000000002 \
  WEIXIN_BOT_TOKEN=fixture-token \
  bash "$guard" >"$output" 2>&1; then
  echo "partial Weixin bootstrap credentials were accepted" >&2
  exit 1
fi

echo "messaging self-host check tests: OK"
