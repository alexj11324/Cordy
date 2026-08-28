#!/usr/bin/env bash
set -euo pipefail

# Static contract checks for the Rust-only Makefile entrypoints. This script
# intentionally uses dry runs and never invokes a compiler or test runner.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "$1" >&2
  exit 1
}

for target in cli cordy; do
  output="$(make -n "$target" CORDY_ARGS=version)"
  grep -Fq -- "./scripts/run-rust.sh run --locked -p cordy-cli -- version" <<<"$output" ||
    fail "$target: expected the Rust CLI entrypoint, got:\n$output"
done

quoted_output="$(make -n cli 'CORDY_ARGS=issue create --title "hello world"')"
grep -Fq -- './scripts/run-rust.sh run --locked -p cordy-cli -- issue create --title "hello world"' <<<"$quoted_output" ||
  fail "cli: embedded argument quoting was not preserved:\n$quoted_output"

for target in server rust-server; do
  output="$(make -n "$target")"
  grep -Fq -- "./scripts/run-rust.sh run --locked -p cordy-server" <<<"$output" ||
    fail "$target: expected the Rust server entrypoint, got:\n$output"
done

for target in migrate-up rust-migrate-up migrate-down rust-migrate-down; do
  output="$(make -n "$target")"
  case "$target" in
    migrate-up|rust-migrate-up)
      grep -Fq -- "./scripts/run-rust.sh run --locked -p cordy-migrate -- up" <<<"$output" ||
        fail "$target: expected the Rust up migration runner, got:\n$output"
      ;;
    migrate-down|rust-migrate-down)
      grep -Fq -- "./scripts/run-rust.sh run --locked -p cordy-migrate -- down" <<<"$output" ||
        fail "$target: expected the Rust down migration runner, got:\n$output"
      ;;
  esac
done

for target in build rust-build; do
  output="$(make -n "$target")"
  grep -Fq -- "./scripts/run-rust.sh build --release --locked -p cordy-server -p cordy-cli -p cordy-migrate --bins" <<<"$output" ||
    fail "$target: expected the Rust release build, got:\n$output"
  for artifact in server cordy migrate backfill_task_usage_hourly backfill_issue_last_activity backfill_codex_usage_cache; do
    grep -Eq -- "cp .* \"bin/${artifact}(\.exe)?\"" <<<"$output" ||
      fail "$target: expected bin/${artifact} output, got:\n$output"
  done
done

echo "✓ Makefile entrypoints and artifacts are Rust-only"
