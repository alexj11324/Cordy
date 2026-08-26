#!/usr/bin/env bash
set -euo pipefail

# Build the Rust CLI into Cargo's normal target directory, then execute a
# content-addressed copy. A daemon started by this CLI therefore never owns
# Cargo's live output: the next source invocation can rebuild it safely on
# Windows, where replacing a running executable is forbidden.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$ROOT_DIR/server-rs"

cd "$RUST_DIR"

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [[ "$TARGET_DIR" != /* ]]; then
	TARGET_DIR="$RUST_DIR/$TARGET_DIR"
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
		echo "run-rust-cli.sh: cargo is not on PATH; set CARGO_BIN or CARGO_HOME" >&2
		exit 127
	fi
fi

if [[ "$CARGO_COMMAND" == */* ]]; then
	if [[ ! -x "$CARGO_COMMAND" ]]; then
		echo "run-rust-cli.sh: CARGO_BIN is not executable: $CARGO_COMMAND" >&2
		exit 127
	fi
elif ! command -v "$CARGO_COMMAND" >/dev/null 2>&1; then
	echo "run-rust-cli.sh: cargo command not found: $CARGO_COMMAND" >&2
	exit 127
fi

CARGO_TARGET_DIR="$TARGET_DIR" "$CARGO_COMMAND" build -p cordy-cli

if [[ "${OS:-}" == "Windows_NT" || "${OSTYPE:-}" == msys* || "${OSTYPE:-}" == cygwin* ]]; then
	EXE_SUFFIX=".exe"
else
	EXE_SUFFIX=""
fi

BUILT_BINARY="$TARGET_DIR/debug/cordy${EXE_SUFFIX}"
RUN_DIR="$TARGET_DIR/cordy-cli-runs"
mkdir -p "$RUN_DIR"

# Keep stale copies bounded without touching a recently launched daemon. On
# Windows an in-use executable cannot be deleted; the failure is intentionally
# ignored and the next cleanup pass can remove it after shutdown.
find "$RUN_DIR" -maxdepth 1 -type f -name "cordy-*${EXE_SUFFIX}" -mmin +10080 -delete 2>/dev/null || true

if command -v sha256sum >/dev/null 2>&1; then
	DIGEST="$(sha256sum "$BUILT_BINARY" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
	DIGEST="$(shasum -a 256 "$BUILT_BINARY" | awk '{print $1}')"
else
	DIGEST="$(cksum "$BUILT_BINARY" | awk '{print $1 "-" $2}')"
fi

RUN_BINARY="$RUN_DIR/cordy-${DIGEST}${EXE_SUFFIX}"
if [[ ! -e "$RUN_BINARY" ]]; then
	cp "$BUILT_BINARY" "$RUN_BINARY"
	chmod +x "$RUN_BINARY"
fi

exec "$RUN_BINARY" "$@"
