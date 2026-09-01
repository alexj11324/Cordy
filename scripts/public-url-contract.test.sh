#!/usr/bin/env bash
set -euo pipefail

# Keep product-facing links on the canonical product domains and repository. This is
# intentionally narrower than a branding scan: reserved example addresses,
# SSH repository fixtures, and migration comments may still be valid data, but
# an old production URL must never return to shipping source.
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
forbidden='https?://(www\.)?patchbay\.ai|https?://api\.patchbay\.ai|raw\.githubusercontent\.com/patchbay-ai/patchbay|github\.com/patchbay-ai/patchbay|github\.com/alexj11324/Patchbay|ghcr\.io/patchbay-ai/'

legacy_hits="$(rg -l --hidden -g '!.git/**' -g '!node_modules/**' -g '!server-rs/target/**' "$forbidden" "$repo_root" || true)"
while IFS= read -r file; do
  [[ -z "$file" ]] && continue
  echo "public URL contract: legacy production URL found in $file" >&2
  rg -n --hidden -g '!.git/**' -g '!node_modules/**' -g '!server-rs/target/**' "$forbidden" "$file" >&2
  exit 1
done <<<"$legacy_hits"

builder="$repo_root/apps/desktop/electron-builder.yml"
canonical_repo_name="C""ordy"
grep -Fq -- "  owner: alexj11324" "$builder" || {
  echo "public URL contract: desktop publisher owner is not the canonical repository owner" >&2
  exit 1
}
grep -Fq -- "  repo: $canonical_repo_name" "$builder" || {
  echo "public URL contract: desktop publisher repo is not the canonical repository" >&2
  exit 1
}

echo "public URL contract: OK"
