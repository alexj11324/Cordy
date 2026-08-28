#!/usr/bin/env bash
set -euo pipefail

# Run Cargo from the Rust workspace while preserving the established relative
# local-upload path. Keeping this boundary in one wrapper avoids duplicating
# Cargo path detection across Makefile, dev, and check entrypoints.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$ROOT_DIR/server-rs"

upload_dir="${LOCAL_UPLOAD_DIR:-./data/uploads}"
case "$upload_dir" in
  /*|[A-Za-z]:[\\/]*) ;;
  *) export LOCAL_UPLOAD_DIR="$ROOT_DIR/server/$upload_dir" ;;
esac

cargo_command="${CARGO_BIN:-}"
if [[ -z "$cargo_command" ]]; then
  if command -v cargo >/dev/null 2>&1; then
    cargo_command="$(command -v cargo)"
  elif [[ -n "${CARGO_HOME:-}" && -x "${CARGO_HOME}/bin/cargo" ]]; then
    cargo_command="${CARGO_HOME}/bin/cargo"
  elif [[ -n "${HOME:-}" && -x "${HOME}/.cargo/bin/cargo" ]]; then
    cargo_command="${HOME}/.cargo/bin/cargo"
  else
    echo "run-rust.sh: cargo is not on PATH; set CARGO_BIN or CARGO_HOME" >&2
    exit 127
  fi
fi

if [[ "$cargo_command" == */* ]]; then
  if [[ ! -x "$cargo_command" ]]; then
    echo "run-rust.sh: CARGO_BIN is not executable: $cargo_command" >&2
    exit 127
  fi
elif ! command -v "$cargo_command" >/dev/null 2>&1; then
  echo "run-rust.sh: cargo command not found: $cargo_command" >&2
  exit 127
fi

cd "$RUST_DIR"
exec "$cargo_command" "$@"
