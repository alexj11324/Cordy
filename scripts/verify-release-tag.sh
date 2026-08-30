#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: verify-release-tag.sh <tag> <expected-commit>" >&2
  exit 2
fi

tag_name="$1"
expected_commit="$2"
repository="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"

tag_json="$(gh api "repos/$repository/git/ref/tags/$tag_name")"
tag_sha="$(jq -r '.object.sha' <<< "$tag_json")"
tag_type="$(jq -r '.object.type' <<< "$tag_json")"
while [ "$tag_type" = "tag" ]; do
  tag_json="$(gh api "repos/$repository/git/tags/$tag_sha")"
  tag_sha="$(jq -r '.object.sha' <<< "$tag_json")"
  tag_type="$(jq -r '.object.type' <<< "$tag_json")"
done

if [ "$tag_type" != "commit" ] || [ "$tag_sha" != "$expected_commit" ]; then
  echo "::error::Release tag $tag_name no longer resolves to prepared commit $expected_commit." >&2
  exit 1
fi

echo "Release tag $tag_name still resolves to $expected_commit."
