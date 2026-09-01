#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
setup="$repo_root/scripts/messaging-self-host-setup.sh"
manifest="$repo_root/deploy/messaging/slack-app-manifest.yaml"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

[[ -x "$setup" ]] || { echo "setup helper is not executable" >&2; exit 1; }
[[ -f "$manifest" ]] || { echo "Slack manifest is missing" >&2; exit 1; }

output="$($setup slack-manifest)"
grep -Fq 'socket_mode_enabled: true' <<<"$output"
grep -Fq 'command: /issue' <<<"$output"
grep -Fq 'message.im' <<<"$output"

env_file="$tmp_dir/.env.messaging"
"$setup" init \
  --app-url https://app.example.com \
  --api-url https://api.example.com \
  --env-file "$env_file" >"$tmp_dir/init.out"
grep -Fq 'PATCHBAY_MESSAGING_MODE=server_configured' "$env_file"
grep -Fq 'PATCHBAY_APP_URL=https://app.example.com' "$env_file"
grep -Fq 'PATCHBAY_PUBLIC_URL=https://api.example.com' "$env_file"
grep -Fq 'PATCHBAY_MESSAGING_BOOTSTRAP=false' "$env_file"
grep -Fq 'PATCHBAY_MESSAGING_WORKSPACE_ID=' "$env_file"
grep -Fq 'SLACK_APP_TOKEN=' "$env_file"
grep -Fq 'TELEGRAM_BOT_TOKEN=' "$env_file"

if "$setup" init --app-url https://localhost:3000 --api-url https://api.example.com \
  --env-file "$tmp_dir/rejected.env" >"$tmp_dir/rejected.out" 2>&1; then
  echo "localhost origin was accepted" >&2
  exit 1
fi

if "$setup" init --app-url 'https://[fe80::1]' --api-url https://api.example.com \
  --env-file "$tmp_dir/rejected-ipv6.env" >"$tmp_dir/rejected-ipv6.out" 2>&1; then
  echo "IPv6 link-local origin was accepted" >&2
  exit 1
fi

echo "messaging self-host setup tests: OK"
