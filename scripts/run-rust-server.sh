#!/usr/bin/env bash
set -euo pipefail

# Run Cargo from the Rust server workspace while preserving the legacy server
# working-directory semantics for relative local-upload paths. The explicit
# CARGO_BIN/CARGO_HOME fallbacks also make non-interactive rustup installs work
# when ~/.cargo/bin is not on PATH.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$ROOT_DIR/server-rs"

if [[ -n "${LOCAL_UPLOAD_DIR:-}" ]]; then
	UPLOAD_DIR="$LOCAL_UPLOAD_DIR"
else
	UPLOAD_DIR="./data/uploads"
fi
if [[ "$UPLOAD_DIR" != /* && "$UPLOAD_DIR" != [A-Za-z]:[\\/]* ]]; then
	export LOCAL_UPLOAD_DIR="$ROOT_DIR/server/$UPLOAD_DIR"
fi

CARGO_COMMAND="${CARGO_BIN:-}"
if [[ -z "$CARGO_COMMAND" ]]; then
	if command -v cargo >/dev/null 2>&1; then
		CARGO_COMMAND="$(command -v cargo)"
	elif [[ -n "${CARGO_HOME:-}" && -x "${CARGO_HOME}/bin/cargo" ]]; then
		CARGO_COMMAND="${CARGO_HOME}/bin/cargo"
	elif [[ -n "${HOME:-}" && -x "${HOME}/.cargo/bin/cargo" ]]; then
		CARGO_COMMAND="${HOME}/.cargo/bin/cargo"
	else
		echo "run-rust-server.sh: cargo is not on PATH; set CARGO_BIN or CARGO_HOME" >&2
		exit 127
	fi
fi

if [[ "$CARGO_COMMAND" == */* ]]; then
	if [[ ! -x "$CARGO_COMMAND" ]]; then
		echo "run-rust-server.sh: CARGO_BIN is not executable: $CARGO_COMMAND" >&2
		exit 127
	fi
elif ! command -v "$CARGO_COMMAND" >/dev/null 2>&1; then
	echo "run-rust-server.sh: cargo command not found: $CARGO_COMMAND" >&2
	exit 127
fi

cd "$RUST_DIR"
exec "$CARGO_COMMAND" "$@"
