#!/usr/bin/env bash
set -euo pipefail

# The default `make build` path is Rust. The explicit legacy `make go-build`
# path still has to name its outputs the way the target platform expects.
# Windows refuses to execute an extensionless file, so a Windows source build
# whose artifacts are named `cordy` produces a CLI that cannot re-exec itself
# as a daemon (#7255) — the build succeeds and the failure surfaces later as a
# misleading "not found" at startup.
#
# The suffix is derived from GOOS, which reaches a build two ways: as an
# environment variable and as a Make variable on the command line. `go build`
# honors both, so a suffix that honors only one silently rebuilds the original
# bug. Nothing else covers this: the Go test suite never runs the Makefile.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "$1" >&2
  exit 1
}

# Source-facing entrypoints must resolve to the Rust ports. The Go commands
# remain available only through explicit go-* compatibility targets until the
# final Go source retirement.
for target in cli cordy; do
  rust_output="$(make -n "$target" CORDY_ARGS=version)"
  grep -Fq -- "./scripts/run-rust.sh run --locked -p cordy-cli -- version" <<<"$rust_output" ||
    fail "$target: expected the Rust CLI entrypoint, got:\n$rust_output"
  if grep -Fq -- "cd server && go run" <<<"$rust_output"; then
    fail "$target: unexpectedly resolved to the legacy Go CLI:\n$rust_output"
  fi
done

quoted_output="$(make -n cli 'CORDY_ARGS=issue create --title "hello world"')"
grep -Fq -- './scripts/run-rust.sh run --locked -p cordy-cli -- issue create --title "hello world"' <<<"$quoted_output" ||
  fail "cli: embedded argument quoting was not preserved:\n$quoted_output"

for target in server rust-server; do
  rust_server_output="$(make -n "$target")"
  grep -Fq -- "./scripts/run-rust.sh run --locked -p cordy-server" <<<"$rust_server_output" ||
    fail "$target: expected the Rust server entrypoint, got:\n$rust_server_output"
  if grep -Fq -- "cd server && go run ./cmd/server" <<<"$rust_server_output"; then
    fail "$target: unexpectedly resolved to the legacy Go server:\n$rust_server_output"
  fi
done

go_server_output="$(make -n go-server)"
grep -Fq -- "cd server && go run ./cmd/server" <<<"$go_server_output" ||
  fail "go-server: expected the explicit legacy Go server entrypoint, got:\n$go_server_output"

for target in migrate-up rust-migrate-up migrate-down rust-migrate-down; do
  migration_output="$(make -n "$target")"
  case "$target" in
    migrate-up|rust-migrate-up)
      grep -Fq -- "./scripts/run-rust.sh run --locked -p cordy-migrate -- up" <<<"$migration_output" ||
        fail "$target: expected the Rust up migration runner, got:\n$migration_output"
      ;;
    migrate-down|rust-migrate-down)
      grep -Fq -- "./scripts/run-rust.sh run --locked -p cordy-migrate -- down" <<<"$migration_output" ||
        fail "$target: expected the Rust down migration runner, got:\n$migration_output"
      ;;
  esac
  if grep -Fq -- "cd server && go run ./cmd/migrate" <<<"$migration_output"; then
    fail "$target: unexpectedly resolved to the legacy Go migration runner:\n$migration_output"
  fi
done

for target in go-migrate-up go-migrate-down; do
  legacy_migration_output="$(make -n "$target")"
  grep -Fq -- "cd server && go run ./cmd/migrate" <<<"$legacy_migration_output" ||
    fail "$target: expected the explicit legacy Go migration runner, got:\n$legacy_migration_output"
done

# The default build emits compatibility-named artifacts from Rust, including
# all three migrated operator backfills, and must not probe the Go toolchain.
for target in build rust-build; do
  rust_build_output="$(make -n "$target")"
  grep -Fq -- "./scripts/run-rust.sh build --release --locked -p cordy-server -p cordy-cli -p cordy-migrate --bins" <<<"$rust_build_output" ||
    fail "$target: expected the Rust release build, got:\n$rust_build_output"
  for artifact in cordy-server cordy cordy-migrate backfill_task_usage_hourly backfill_issue_last_activity backfill_codex_usage_cache; do
    grep -Eq -- "cp \".*/server-rs/target/release/${artifact}" <<<"$rust_build_output" ||
      fail "$target: expected Rust artifact ${artifact} to be copied, got:\n$rust_build_output"
  done
  if grep -Fq -- "go build" <<<"$rust_build_output"; then
    fail "$target: unexpectedly resolved to the legacy Go build:\n$rust_build_output"
  fi
done

# The recipe reads `-o bin/server$(EXE) ./cmd/server`, so the trailing space is
# what keeps an expected `bin/server` from matching an emitted `bin/server.exe`.
require_outputs() {
  local label=$1 suffix=$2 output=$3 binary

  for binary in server cordy migrate; do
    grep -Fq -- "-o bin/${binary}${suffix} " <<<"$output" ||
      fail "$label: expected 'go build ... -o bin/${binary}${suffix}', got:
$output"
  done
}

# A `go` shim that records every invocation, so the assertions below can tell
# "the Makefile probed the toolchain" from "the Makefile did not" without
# depending on how the host PATH is laid out.
probe_dir="$(mktemp -d)"
trap 'rm -rf "$probe_dir"' EXIT
real_go="$(command -v go || true)"
cat >"$probe_dir/go" <<EOF
#!/usr/bin/env bash
echo "\$@" >>"$probe_dir/invocations"
[ -n "$real_go" ] || exit 1
exec "$real_go" "\$@"
EOF
chmod +x "$probe_dir/go"

probe_count() {
  [ -f "$probe_dir/invocations" ] || { echo 0; return; }
  wc -l <"$probe_dir/invocations" | tr -d ' '
}

require_outputs "GOOS=windows in the environment" .exe \
  "$(GOOS=windows make -n go-build)"
require_outputs "GOOS=windows as a Make variable" .exe \
  "$(make -n go-build GOOS=windows)"
require_outputs "GOOS=linux in the environment" "" \
  "$(GOOS=linux make -n go-build)"
require_outputs "GOOS=darwin as a Make variable" "" \
  "$(make -n go-build GOOS=darwin)"

# Non-build targets must not reach for a Go toolchain: `make help` and
# `make clean` are the first thing a frontend-only contributor runs, and a
# global suffix assignment makes every one of them print `go: Command not
# found` on a checkout with no Go installed.
PATH="$probe_dir:$PATH" make -n clean >/dev/null
PATH="$probe_dir:$PATH" make help >/dev/null
[ "$(probe_count)" = 0 ] ||
  fail "non-build targets invoked go $(probe_count) time(s): $(cat "$probe_dir/invocations")"

# With no GOOS given, the suffix has to follow the toolchain's own default.
if [ -n "$real_go" ]; then
  host_suffix=""
  [ "$("$real_go" env GOOS)" = windows ] && host_suffix=.exe
  require_outputs "no GOOS set" "$host_suffix" \
    "$(PATH="$probe_dir:$PATH" make -n go-build)"
  [ "$(probe_count)" != 0 ] ||
    fail "no GOOS set: expected go-build to resolve GOOS via go env"
else
  echo "skipping the host-default case: no go toolchain on PATH"
fi

echo "✓ make build uses Rust; go-build names legacy outputs for the target platform"
