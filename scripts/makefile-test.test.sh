#!/usr/bin/env bash
set -eu

rust_output=$(make -n test)
if ! grep -Fq -- "cd server-rs && cargo test --workspace --all-targets --locked" <<<"$rust_output"; then
  echo "make test must resolve to the Rust workspace test command:" >&2
  echo "$rust_output" >&2
  exit 1
fi
if grep -Fq -- "scripts/test-go.sh" <<<"$rust_output"; then
  echo "make test unexpectedly resolved to the legacy Go test wrapper:" >&2
  echo "$rust_output" >&2
  exit 1
fi

go_output=$(make -n go-test)
if ! grep -Fq -- "bash scripts/test-go.sh --race" <<<"$go_output"; then
  echo "go-test must retain the explicit Go compatibility test command:" >&2
  echo "$go_output" >&2
  exit 1
fi

echo "✓ make test uses Rust; go-test retains explicit Go compatibility coverage"
