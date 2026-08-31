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

source_line="$(grep -n '^\. "\$ENV_FILE"' "$root_dir/scripts/dev.sh" | cut -d: -f1)"
clear_line="$(grep -n '^clear_process_only_clerk_env$' "$root_dir/scripts/dev.sh" | cut -d: -f1)"
if [ -z "$source_line" ] || [ -z "$clear_line" ] || [ "$clear_line" -le "$source_line" ]; then
  echo "dev.sh must clear process-only Clerk values after sourcing the checkout env" >&2
  exit 1
fi

echo "POSIX development environment secret sanitization passed"
