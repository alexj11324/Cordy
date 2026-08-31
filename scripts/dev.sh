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
# The public Node launcher selects/creates this file, loads it into the child
# environment on every platform, and then dispatches to this POSIX phase.
if [ -z "${ENV_FILE:-}" ] || [ ! -f "$ENV_FILE" ]; then
  echo "✗ Complete development environment was not prepared. Run 'pnpm dev'."
  exit 1
fi

echo "==> Using $ENV_FILE"

set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

# shellcheck disable=SC1091
. scripts/local-env.sh

dev_mode="${PATCHBAY_DEV_MODE:-local}"
for arg in "$@"; do
  if [ "$arg" = "--hosted" ]; then
    dev_mode="hosted"
  fi
done
case "$dev_mode" in
  local|hosted) ;;
  *)
    echo "✗ Unsupported development runtime mode: $dev_mode" >&2
    exit 1
    ;;
esac
export PATCHBAY_DEV_MODE="$dev_mode"

if [ "$dev_mode" = "hosted" ]; then
  # This is the explicit production-service development profile. Keep the
  # tuple immutable so a stale local VITE_* variable cannot create a mixed
  # OAuth/API flow.
  export PATCHBAY_DEV_API_URL="https://api.aspectlylabs.com"
  export PATCHBAY_DEV_WS_URL="wss://api.aspectlylabs.com/ws"
  export PATCHBAY_DEV_APP_URL="https://patchbay.aspectlylabs.com"
  export PATCHBAY_DEV_ACCOUNTS_URL="https://accounts.aspectlylabs.com"
else
  # Complete local dev owns a real Next.js listener for every browser-facing
  # URL. Do not inherit a stale/custom FRONTEND_ORIGIN that this launcher does
  # not serve, and never generate 127.0.0.1 as the browser OAuth origin.
  export FRONTEND_ORIGIN="http://localhost:${FRONTEND_PORT:-3000}"
  export PATCHBAY_DEV_API_URL="http://127.0.0.1:${PORT:-8080}"
  export PATCHBAY_DEV_WS_URL="ws://127.0.0.1:${PORT:-8080}/ws"
  export PATCHBAY_DEV_APP_URL="$FRONTEND_ORIGIN"
  export PATCHBAY_DEV_ACCOUNTS_URL="$FRONTEND_ORIGIN"
fi
export PATCHBAY_APP_URL="$PATCHBAY_DEV_APP_URL"

# Preserve the upload location used by the established run-rust.sh launcher.
# The backend itself runs from server-rs, so a relative value would otherwise
# move existing attachments beneath that directory.
upload_dir="${LOCAL_UPLOAD_DIR:-./data/uploads}"
case "$upload_dir" in
  /*|[A-Za-z]:[\\/]*) ;;
  *) export LOCAL_UPLOAD_DIR="$REPO_ROOT/server/$upload_dir" ;;
esac

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

export VITE_API_URL="$PATCHBAY_DEV_API_URL"
export VITE_WS_URL="$PATCHBAY_DEV_WS_URL"
export VITE_APP_URL="$PATCHBAY_DEV_APP_URL"
export VITE_ACCOUNTS_URL="$PATCHBAY_DEV_ACCOUNTS_URL"
export NEXT_PUBLIC_API_URL="$VITE_API_URL"
export NEXT_PUBLIC_WS_URL="$VITE_WS_URL"

if [ "$dev_mode" = "hosted" ]; then
  export PATCHBAY_PUBLIC_URL="$PATCHBAY_DEV_API_URL"
  export PATCHBAY_SERVER_URL="$PATCHBAY_DEV_WS_URL"
  export PATCHBAY_REQUIRE_SOURCE_CLI=1
  export PATCHBAY_DEV_ENV_FILE="$ENV_FILE"
  echo ""
  echo "✓ Hosted Desktop development environment"
  echo "  OAuth:   $PATCHBAY_DEV_ACCOUNTS_URL"
  echo "  API:     $PATCHBAY_DEV_API_URL"
  echo "  Renderer: local Electron/Vite hot reload"
  echo ""
  node apps/desktop/scripts/dev.mjs "$@"
  exit $?
fi

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
web_pid=""
cleanup() {
  if [ -n "$web_pid" ] && kill -0 "$web_pid" >/dev/null 2>&1; then
    kill "$web_pid" >/dev/null 2>&1 || true
    wait "$web_pid" 2>/dev/null || true
  fi
  if [ -n "$backend_pid" ] && kill -0 "$backend_pid" >/dev/null 2>&1; then
    kill "$backend_pid" >/dev/null 2>&1 || true
    wait "$backend_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

backend_ready_url="http://127.0.0.1:${PORT:-8080}/healthz"
if curl --fail --silent --show-error "$backend_ready_url" >/dev/null 2>&1; then
  echo "✗ Backend port ${PORT:-8080} is already serving another Patchbay instance." >&2
  echo "  Stop it or use this checkout's isolated PORT before running pnpm dev." >&2
  exit 1
fi

(cd server-rs && "$dev_backend") &
backend_pid=$!

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
if ! kill -0 "$backend_pid" >/dev/null 2>&1; then
  wait "$backend_pid" || true
  echo "✗ Spawned backend exited during readiness verification." >&2
  exit 1
fi

frontend_ready_url="$FRONTEND_ORIGIN/"
if curl --fail --silent --show-error "$frontend_ready_url" >/dev/null 2>&1; then
  echo "✗ Frontend port ${FRONTEND_PORT:-3000} is already serving another Patchbay instance." >&2
  echo "  Stop it or use this checkout's isolated FRONTEND_PORT before running pnpm dev." >&2
  exit 1
fi

echo "==> Starting the browser/share/login origin at $FRONTEND_ORIGIN..."
(
  cd apps/web
  exec node node_modules/next/dist/bin/next dev --webpack --port "${FRONTEND_PORT:-3000}"
) &
web_pid=$!

frontend_deadline=$((SECONDS + 120))
until curl --fail --silent --show-error "$frontend_ready_url" >/dev/null 2>&1; do
  if ! kill -0 "$web_pid" >/dev/null 2>&1; then
    wait "$web_pid" || true
    echo "✗ Frontend exited before its browser-link health check passed." >&2
    exit 1
  fi
  if [ "$SECONDS" -ge "$frontend_deadline" ]; then
    echo "✗ Frontend did not become reachable within 2 minutes: $frontend_ready_url" >&2
    exit 1
  fi
  sleep 1
done

missing_google_keys=()
for key in NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY CLERK_SECRET_KEY CLERK_JWT_KEY CLERK_ISSUER CLERK_AUTHORIZED_PARTIES; do
  if [ -z "${!key:-}" ]; then
    missing_google_keys+=("$key")
  fi
done
if [ ${#missing_google_keys[@]} -gt 0 ]; then
  echo "! Google sign-in is unavailable; add these values to $ENV_FILE: ${missing_google_keys[*]}"
fi

export PATCHBAY_REQUIRE_SOURCE_CLI=1
export PATCHBAY_DEV_ENV_FILE="$ENV_FILE"

node apps/desktop/scripts/dev.mjs "$@"
