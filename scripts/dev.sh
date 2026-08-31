#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# Complete Desktop development entrypoint. It owns the local backend and
# database preparation, then launches Electron only after the backend, DB,
# source-matched CLI and local capability probes all pass.

# ---------- Check prerequisites ----------
missing=()
command -v node >/dev/null 2>&1 || missing+=("node")
command -v pnpm >/dev/null 2>&1 || missing+=("pnpm")
command -v curl >/dev/null 2>&1 || missing+=("curl")

if [ ${#missing[@]} -gt 0 ]; then
  echo "✗ Missing prerequisites: ${missing[*]}"
  echo "  Please install: Node.js 22, pnpm 10.28.2, curl"
  echo "  Rust/Cargo is required only when the source-matched runtime cache misses."
  echo "  PostgreSQL may run through Docker Compose or a native local installation."
  exit 1
fi

# ---------- Environment file ----------
if [ -n "${ENV_FILE:-}" ]; then
  if [ ! -f "$ENV_FILE" ]; then
    echo "✗ Configured env file does not exist: $ENV_FILE"
    exit 1
  fi
elif [ -f .git ]; then
  # Inside a git worktree (.git is a file, not a directory)
  ENV_FILE=".env.worktree"
  if [ ! -f "$ENV_FILE" ]; then
    echo "==> Worktree detected. Generating $ENV_FILE..."
    bash scripts/init-worktree-env.sh "$ENV_FILE"
  fi
else
  ENV_FILE=".env"
  if [ ! -f "$ENV_FILE" ]; then
    echo "==> Creating $ENV_FILE from .env.example..."
    cp .env.example "$ENV_FILE"
  fi
fi

echo "==> Using $ENV_FILE"

node scripts/ensure-dev-integration-secrets.mjs "$ENV_FILE"

set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

# shellcheck disable=SC1091
. scripts/local-env.sh

# Keep the shared compiler cache valuable and bounded. Worktree target
# directories remain independent and are never redirected into this cache.
export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-10G}"

# ---------- Install dependencies ----------
if [ ! -d node_modules ]; then
  echo "==> Installing dependencies..."
  pnpm install
fi

# ---------- Source-matched development runtime ----------
# Cache hits stage the CLI, backend and migration runner without Cargo. A
# source/toolchain/target/profile miss performs one worktree-local incremental
# build and stores the three exact binaries in the shared artifact cache.
node apps/desktop/scripts/prepare-dev-runtime.mjs

runtime_suffix=""
if [ "$(node -p 'process.platform')" = "win32" ]; then
  runtime_suffix=".exe"
fi
dev_backend="$REPO_ROOT/.patchbay-dev/bin/patchbay-server${runtime_suffix}"
dev_migrate="$REPO_ROOT/.patchbay-dev/bin/patchbay-migrate${runtime_suffix}"

# ---------- Database ----------
bash scripts/ensure-postgres.sh "$ENV_FILE"

echo "==> Running migrations..."
(cd server-rs && "$dev_migrate" up)

# ---------- Start complete Desktop stack ----------
echo ""
echo "✓ Database ready. Starting the complete Desktop environment..."
echo "  Backend:  http://localhost:${PORT:-8080}"
echo "  Electron renderer: hot reload enabled"
echo "  Dev CLI: source fingerprint cache, then incremental Cargo on miss"
echo ""

backend_pid=""
cleanup() {
  if [ -n "$backend_pid" ] && kill -0 "$backend_pid" >/dev/null 2>&1; then
    kill "$backend_pid" >/dev/null 2>&1 || true
    wait "$backend_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

(cd server-rs && "$dev_backend") &
backend_pid=$!

backend_ready_url="http://127.0.0.1:${PORT:-8080}/healthz"
deadline=$((SECONDS + 1800))
until curl --fail --silent --show-error "$backend_ready_url" >/dev/null 2>&1; do
  if ! kill -0 "$backend_pid" >/dev/null 2>&1; then
    wait "$backend_pid" || true
    echo "✗ Backend exited before its database readiness check passed."
    exit 1
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    echo "✗ Backend did not become ready within 30 minutes: $backend_ready_url"
    exit 1
  fi
  sleep 1
done

export VITE_API_URL="http://127.0.0.1:${PORT:-8080}"
export VITE_WS_URL="ws://127.0.0.1:${PORT:-8080}/ws"
export PATCHBAY_REQUIRE_SOURCE_CLI=1
export PATCHBAY_DEV_ENV_FILE="$ENV_FILE"

node apps/desktop/scripts/dev.mjs
