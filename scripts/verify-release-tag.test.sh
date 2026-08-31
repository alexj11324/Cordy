#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
guard="$repo_root/scripts/verify-release-tag.sh"
release_workflow="$repo_root/.github/workflows/release.yml"
macos_release_workflow="$repo_root/.github/workflows/macos-release.yml"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

mkdir -p "$tmp_dir/bin"
cat > "$tmp_dir/bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

path="${2:?missing gh api path}"
if [[ "$path" == */git/ref/tags/* ]]; then
  case "${FAKE_GH_MODE:?}" in
    lightweight)
      printf '%s\n' '{"object":{"type":"commit","sha":"expected-sha"}}'
      ;;
    annotated|mismatch)
      printf '%s\n' '{"object":{"type":"tag","sha":"tag-object-sha"}}'
      ;;
  esac
elif [[ "$path" == */git/tags/tag-object-sha ]]; then
  if [ "$FAKE_GH_MODE" = "mismatch" ]; then
    printf '%s\n' '{"object":{"type":"commit","sha":"moved-sha"}}'
  else
    printf '%s\n' '{"object":{"type":"commit","sha":"expected-sha"}}'
  fi
else
  echo "unexpected gh api path: $path" >&2
  exit 1
fi
EOF
chmod +x "$tmp_dir/bin/gh"

for mode in lightweight annotated; do
  PATH="$tmp_dir/bin:$PATH" \
    GITHUB_REPOSITORY=patchbay-ai/patchbay \
    FAKE_GH_MODE="$mode" \
    bash "$guard" v1.2.3 expected-sha >/dev/null
done

release_concurrency='group: production-release-${{ inputs.tag }}'
macos_release_concurrency='group: production-release-${{ inputs.tag || github.event.workflow_run.head_branch || github.run_id }}'
if ! grep -Fq -- "$release_concurrency" "$release_workflow" ||
  ! grep -Fq -- "$macos_release_concurrency" "$macos_release_workflow"; then
  echo "release workflows do not share the tag-scoped production publication lock" >&2
  exit 1
fi

if PATH="$tmp_dir/bin:$PATH" \
  GITHUB_REPOSITORY=patchbay-ai/patchbay \
  FAKE_GH_MODE=mismatch \
  bash "$guard" v1.2.3 expected-sha >/dev/null 2>&1; then
  echo "release tag guard accepted a moved tag" >&2
  exit 1
fi

for mutable_ref in \
  'ref: ${{ needs.verify.outputs.tag_name }}' \
  'ref: ${{ needs.self-hosted-prepare.outputs.tag_name }}'; do
  if grep -Fq -- "$mutable_ref" "$release_workflow"; then
    echo "release workflow still checks out mutable ref: $mutable_ref" >&2
    exit 1
  fi
done

guard_count="$(grep -Fc -- 'verify-release-tag.sh "$TAG_NAME" "$EXPECTED_COMMIT"' "$release_workflow")"
if [ "$guard_count" -lt 7 ]; then
  echo "release workflow is missing immutable-tag guards at publication boundaries" >&2
  exit 1
fi

for release_contract in \
  "$release_concurrency" \
  'git merge-base --is-ancestor "$commit_sha" origin/main' \
  'git merge-base --is-ancestor "$EXPECTED_COMMIT" origin/main'; do
  if ! grep -Fq -- "$release_contract" "$release_workflow"; then
    echo "release workflow is missing main/concurrency contract: $release_contract" >&2
    exit 1
  fi
done

if [ "$(grep -Fc -- 'git fetch --no-tags origin +refs/heads/main:refs/remotes/origin/main' "$release_workflow")" -lt 3 ]; then
  echo "release workflow must re-fetch main at verification and final publication boundaries" >&2
  exit 1
fi

if grep -Fq -- '--publish always' "$release_workflow"; then
  echo "Desktop matrix still publishes before all release jobs succeed" >&2
  exit 1
fi

if [ "$(grep -Fc -- 'gh release upload "$TAG_NAME"' "$release_workflow")" -ne 1 ]; then
  echo "release assets must be uploaded exactly once at the final publication boundary" >&2
  exit 1
fi

early_release_mutations="$(awk '
  /^  publish-release:/ { exit }
  /gh release (create|upload|edit)/ { count += 1 }
  END { print count + 0 }
' "$release_workflow")"
if [ "$early_release_mutations" -ne 0 ]; then
  echo "release workflow mutates the GitHub Release before the final publication job" >&2
  exit 1
fi

for staged_contract in \
  'name: github-release-rust-cli' \
  'name: desktop-release-${{ matrix.target }}-${{ matrix.arch }}' \
  'pattern: desktop-release-*' \
  'needs: [verify, release, desktop, helm-chart]'; do
  if ! grep -Fq -- "$staged_contract" "$release_workflow"; then
    echo "release workflow is missing staged publication contract: $staged_contract" >&2
    exit 1
  fi
done

promote_needs="$(awk '
  /^  promote-self-hosted-latest:/ { capture = 1 }
  capture && /^    if:/ { exit }
  capture { print }
' "$release_workflow")"
for dependency in self-hosted-prepare verify docker-backend-merge docker-web-merge desktop helm-chart publish-release; do
  if ! grep -Fq -- "$dependency" <<<"$promote_needs"; then
    echo "release workflow latest promotion is missing dependency: $dependency" >&2
    exit 1
  fi
done

echo "release tag guard and workflow contract tests: ok"
