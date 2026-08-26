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

CARGO_TARGET_DIR="$TARGET_DIR" cargo build -p cordy-cli

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
