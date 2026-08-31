#!/usr/bin/env bash

clerk_env_args=()

capture_process_only_clerk_env() {
  clerk_env_args=()
  local clerk_key
  local clerk_value
  for clerk_key in \
    NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY \
    CLERK_PUBLISHABLE_KEY \
    CLERK_SECRET_KEY \
    CLERK_JWT_KEY \
    CLERK_ISSUER \
    CLERK_AUTHORIZED_PARTIES; do
    clerk_value="${!clerk_key:-}"
    if [ -n "$clerk_value" ]; then
      clerk_env_args+=("$clerk_key=$clerk_value")
    fi
  done
}

restore_process_only_clerk_env() {
  local clerk_entry
  for clerk_entry in "${clerk_env_args[@]}"; do
    export "$clerk_entry"
  done
}

clear_process_only_clerk_env() {
  unset NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY
  unset CLERK_PUBLISHABLE_KEY
  unset CLERK_SECRET_KEY
  unset CLERK_JWT_KEY
  unset CLERK_ISSUER
  unset CLERK_AUTHORIZED_PARTIES
  unset PATCHBAY_DEV_AUTH_READY
}
