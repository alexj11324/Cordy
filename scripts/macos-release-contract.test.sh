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
require_literal 'github.event.repository.id == 1341050282'
if grep -Fq -- "github.repository == 'patchbay-ai/patchbay'" "$workflow"; then
  echo "macOS release workflow still targets the unavailable Patchbay repository" >&2
  exit 1
fi
require_literal 'group: production-release-${{ inputs.tag || github.event.workflow_run.head_branch || github.run_id }}'
require_literal 'echo "commit_sha=$commit_sha"'
require_literal 'ref: ${{ needs.prepare.outputs.commit_sha }}'
require_literal "startsWith(github.event.workflow_run.head_branch, 'v')"
require_literal "github.event.workflow_run.event == 'workflow_dispatch'"
require_literal 'TRIGGERING_RUN_ID: ${{ github.event.workflow_run.id }}'
require_literal 'actions/runs/$TRIGGERING_RUN_ID/jobs?per_page=100'
require_literal '"rust-platform (macos-latest)"'
require_literal '"rust-platform (windows-latest)"'
require_count 3 'git fetch --no-tags origin +refs/heads/main:refs/remotes/origin/main'
require_literal 'git merge-base --is-ancestor "$commit_sha" origin/main'
require_count 2 'git merge-base --is-ancestor "$EXPECTED_COMMIT" origin/main'
require_literal 'package_matrix: ${{ steps.meta.outputs.package_matrix }}'
require_literal 'if: needs.prepare.outputs.should_publish == '\''true'\'''
require_literal 'matrix: ${{ fromJSON(needs.prepare.outputs.package_matrix) }}'
require_count 3 'EXPECTED_COMMIT: ${{ needs.prepare.outputs.commit_sha }}'
require_count 2 'repos/$GITHUB_REPOSITORY/git/ref/tags/$TAG_NAME'
require_count 2 'if [ "$tag_type" != "commit" ] || [ "$tag_sha" != "$EXPECTED_COMMIT" ]; then'
require_literal 'uses: actions/upload-artifact@v4'
require_literal 'name: macos-release-${{ matrix.arch }}'
require_literal 'uses: actions/download-artifact@v4'
require_literal 'pattern: macos-release-*'
if [ "$(grep -Fc -- 'gh release upload "$TAG_NAME"' "$workflow" || true)" -ne 1 ]; then
  echo "verified macOS assets must reach GitHub Release in one post-matrix upload" >&2
  exit 1
fi
publish_line="$(grep -n '^  publish:$' "$workflow" | cut -d: -f1)"
release_upload_line="$(grep -nF -- 'gh release upload "$TAG_NAME"' "$workflow" | cut -d: -f1)"
if [ -z "$publish_line" ] || [ -z "$release_upload_line" ] || [ "$release_upload_line" -le "$publish_line" ]; then
  echo "GitHub Release assets must be uploaded only from the publish job" >&2
  exit 1
fi
if grep -Fq -- 'BUILD_TARGET' "$workflow"; then
  echo "macOS release workflow must select architectures before allocating matrix runners" >&2
  exit 1
fi
if ! grep -Fq -- 'run_rust:' "$ci_workflow" ||
  ! grep -Fq -- "RUN_RUST: \${{ github.event_name == 'workflow_dispatch' && inputs.run_rust == true }}" "$ci_workflow"; then
  echo "CI must expose an explicit manual-only Rust validation input" >&2
  exit 1
fi
if [ "$(grep -Fc -- "needs.changes.outputs.rust == 'true'" "$ci_workflow" || true)" -lt 5 ]; then
  echo "every Rust worker must use the manual Rust validation gate" >&2
  exit 1
fi

echo "macOS release trigger, matrix, and immutable-commit contract: ok"
