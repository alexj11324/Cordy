#!/usr/bin/env bash
set -euo pipefail

pattern='(?<![A-Za-z0-9_])(?:MUL|Mul|mul|CORDY|Cordy|cordy|CODY|Cody|cody)(?![A-Za-z0-9_])|(?:MUL|mul|CORDY|cordy|CODY|cody)[-_:0-9]'

text_hits="$({
  LC_ALL=C git grep -n -P "$pattern" -- . \
    ':!migrations/**' \
    ':!scripts/check-legacy-brand-markers.sh' || true
} | grep -v 'legacy-brand-compat' || true)"

if [[ -n "$text_hits" ]]; then
  printf '%s\n' "$text_hits"
  echo "Unexpected legacy branding remains in tracked text." >&2
  exit 1
fi

if LC_ALL=C git ls-files | grep -P "$pattern"; then
  echo "Legacy branding remains in a tracked path." >&2
  exit 1
fi
