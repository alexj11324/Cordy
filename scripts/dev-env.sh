#!/usr/bin/env bash

clear_process_only_clerk_env() {
  unset NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY
  unset CLERK_PUBLISHABLE_KEY
  unset CLERK_SECRET_KEY
  unset CLERK_JWT_KEY
  unset CLERK_ISSUER
  unset CLERK_AUTHORIZED_PARTIES
  unset PATCHBAY_DEV_AUTH_READY
}
