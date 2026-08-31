#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cargo_command="${CARGO_BIN:-${CARGO:-cargo}}"

if ! cargo_identity="$("$cargo_command" -vV 2>/dev/null)"; then
  echo "run-dev-rust.sh: Cargo is unavailable; install the pinned Rust toolchain first." >&2
  exit 1
fi
rust_target="$(awk '/^host:/ { print $2; exit }' <<<"$cargo_identity")"
if [ -z "$rust_target" ]; then
  echo "run-dev-rust.sh: Cargo did not report a host target." >&2
  exit 1
fi

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo_root/server-rs/target}"
exec "$repo_root/scripts/run-rust.sh" "$1" --target "$rust_target" "${@:2}"
