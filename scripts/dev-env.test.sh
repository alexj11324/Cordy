#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY=pk_test_leaked
export CLERK_PUBLISHABLE_KEY=pk_test_leaked
export CLERK_SECRET_KEY=sk_test_leaked
export CLERK_JWT_KEY=jwt_leaked
export CLERK_ISSUER=https://leaked.example
export CLERK_AUTHORIZED_PARTIES=http://leaked.example
export PATCHBAY_DEV_AUTH_READY=1

# shellcheck disable=SC1091
. "$root_dir/scripts/dev-env.sh"
clear_process_only_clerk_env

for key in NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY CLERK_PUBLISHABLE_KEY CLERK_SECRET_KEY CLERK_JWT_KEY CLERK_ISSUER CLERK_AUTHORIZED_PARTIES PATCHBAY_DEV_AUTH_READY; do
  if [ -n "${!key:-}" ]; then
    echo "process-only Clerk value survived sanitization: $key" >&2
    exit 1
  fi
done

export NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY=pk_test_process
export CLERK_SECRET_KEY=sk_test_process
unset CLERK_JWT_KEY
capture_process_only_clerk_env

# Simulate stale values sourced from .env.worktree after the process-only
# snapshot. They must be cleared, and a key absent from the snapshot must not
# be resurrected by the restore step.
export NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY=pk_test_checkout
export CLERK_SECRET_KEY=sk_test_checkout
export CLERK_JWT_KEY=jwt_checkout
clear_process_only_clerk_env
restore_process_only_clerk_env

if [ "${NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY:-}" != "pk_test_process" ] || \
  [ "${CLERK_SECRET_KEY:-}" != "sk_test_process" ]; then
  echo "explicit process-only Clerk values were not restored" >&2
  exit 1
fi
if [ -n "${CLERK_JWT_KEY:-}" ]; then
  echo "checkout-file Clerk value entered the process-only snapshot" >&2
  exit 1
fi
clear_process_only_clerk_env

capture_line="$(grep -n '^capture_process_only_clerk_env$' "$root_dir/scripts/dev.sh" | cut -d: -f1)"
source_line="$(grep -n '^\. "\$ENV_FILE"' "$root_dir/scripts/dev.sh" | cut -d: -f1)"
clear_line="$(grep -n '^clear_process_only_clerk_env$' "$root_dir/scripts/dev.sh" | cut -d: -f1)"
if [ -z "$capture_line" ] || [ -z "$source_line" ] || [ -z "$clear_line" ] || \
  [ "$capture_line" -ge "$source_line" ] || [ "$clear_line" -le "$source_line" ]; then
  echo "dev.sh must capture process-only Clerk values before sourcing and clear checkout values afterwards" >&2
  exit 1
fi
if grep -F '[[ -v' "$root_dir/scripts/dev.sh" "$root_dir/scripts/dev-env.sh" >/dev/null; then
  echo "development scripts must remain compatible with macOS Bash 3.2" >&2
  exit 1
fi
if grep -F 'exec "$@"' "$root_dir/scripts/dev.sh" >/dev/null; then
  echo "dev.sh must keep its cleanup trap around injected child processes" >&2
  exit 1
fi
/bin/bash -n "$root_dir/scripts/dev.sh"
/bin/bash -n "$root_dir/scripts/dev-env.sh"

echo "POSIX development environment secret sanitization passed"
