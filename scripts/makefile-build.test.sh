#!/usr/bin/env bash
set -euo pipefail

# Static contract checks for source-matched development runtime entrypoints. This script
# intentionally uses dry runs and never invokes a compiler or test runner.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "$1" >&2
  exit 1
}

for target in cli patchbay; do
  output="$(make -n "$target" PATCHBAY_ARGS=version)"
  grep -Fq -- "node scripts/dev-runtime-command.mjs cli version" <<<"$output" ||
    fail "$target: expected the source-matched CLI entrypoint, got:\n$output"
done

quoted_output="$(make -n cli 'PATCHBAY_ARGS=issue create --title "hello world"')"
grep -Fq -- 'node scripts/dev-runtime-command.mjs cli issue create --title "hello world"' <<<"$quoted_output" ||
  fail "cli: embedded argument quoting was not preserved:\n$quoted_output"

for target in server rust-server; do
  output="$(make -n "$target")"
  grep -Fq -- "node scripts/dev-runtime-command.mjs backend" <<<"$output" ||
    fail "$target: expected the source-matched backend entrypoint, got:\n$output"
done

for target in migrate-up rust-migrate-up migrate-down rust-migrate-down; do
  output="$(make -n "$target")"
  case "$target" in
    migrate-up|rust-migrate-up)
      grep -Fq -- "node scripts/dev-runtime-command.mjs migrations up" <<<"$output" ||
        fail "$target: expected the source-matched up migration runner, got:\n$output"
      ;;
    migrate-down|rust-migrate-down)
      grep -Fq -- "node scripts/dev-runtime-command.mjs migrations down" <<<"$output" ||
        fail "$target: expected the source-matched down migration runner, got:\n$output"
      ;;
  esac
done

for target in build rust-build; do
  output="$(make -n "$target")"
  grep -Fq -- "./scripts/run-rust.sh build --release --locked -p patchbay-server -p patchbay-cli -p patchbay-migrate --bins" <<<"$output" ||
    fail "$target: expected the Rust release build, got:\n$output"
  for artifact in server patchbay migrate backfill_task_usage_hourly backfill_issue_last_activity backfill_codex_usage_cache; do
    grep -Eq -- "cp .* \"bin/${artifact}(\.exe)?\"" <<<"$output" ||
      fail "$target: expected bin/${artifact} output, got:\n$output"
  done
done

dev_output="$(make -n dev)"
[[ "$dev_output" == "pnpm dev" ]] ||
  fail "dev: expected the single complete launcher entrypoint, got:\n$dev_output"
if grep -Fq -- 'ENV_FILE=' <<<"$dev_output"; then
  fail "dev: legacy Make-level ENV_FILE manipulation remains:\n$dev_output"
fi

for removed in setup start setup-main start-main setup-worktree start-worktree check-main check-worktree; do
  if make -n "$removed" >/dev/null 2>&1; then
    fail "$removed: legacy development target still exists"
  fi
done

echo "✓ Makefile development entrypoints use source-matched runtime artifacts"
