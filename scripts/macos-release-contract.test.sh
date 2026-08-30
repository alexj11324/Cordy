#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workflow="$repo_root/.github/workflows/macos-release.yml"
ci_workflow="$repo_root/.github/workflows/ci.yml"

require_literal() {
  local value="$1"
  if ! grep -Fq -- "$value" "$workflow"; then
    echo "missing macOS release contract: $value" >&2
    exit 1
  fi
}

require_count() {
  local minimum="$1"
  local value="$2"
  local count
  count="$(grep -Fc -- "$value" "$workflow" || true)"
  if [ "$count" -lt "$minimum" ]; then
    echo "expected at least $minimum macOS release contract matches for: $value" >&2
    exit 1
  fi
}

require_literal 'commit_sha: ${{ steps.meta.outputs.commit_sha }}'
require_literal 'echo "commit_sha=$commit_sha"'
require_literal 'ref: ${{ needs.prepare.outputs.commit_sha }}'
require_count 3 'EXPECTED_COMMIT: ${{ needs.prepare.outputs.commit_sha }}'
require_count 2 'repos/$GITHUB_REPOSITORY/git/ref/tags/$TAG_NAME'
require_count 2 'if [ "$tag_type" != "commit" ] || [ "$tag_sha" != "$EXPECTED_COMMIT" ]; then'
if ! grep -Fq -- "- '.github/workflows/macos-release.yml'" "$ci_workflow"; then
  echo "macOS release workflow changes must run the release contract test" >&2
  exit 1
fi

echo "macOS release immutable-commit contract: ok"
