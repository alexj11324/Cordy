#!/usr/bin/env bash
set -euo pipefail

pattern='(?<![A-Za-z0-9_])(?:MUL|Mul|mul)(?![A-Za-z0-9_])|MUL[-_:0-9]|mul[_:-]'

if LC_ALL=C git grep -n -P "$pattern" -- . \
  ':!scripts/check-legacy-brand-markers.sh'; then
  echo "Legacy MUL branding remains in tracked text." >&2
  exit 1
fi

if LC_ALL=C git ls-files | grep -P "$pattern"; then
  echo "Legacy MUL branding remains in a tracked path." >&2
  exit 1
fi
