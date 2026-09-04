#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# Complete Desktop development entrypoint. It prepares the isolated database,
# stages one source-matched Go CLI/backend/migration set, starts that backend,
# waits for its readiness endpoint, and only then opens Electron.

missing=()
command -v node >/dev/null 2>&1 || missing+=("node")
command -v pnpm >/dev/null 2>&1 || missing+=("pnpm")
command -v curl >/dev/null 2>&1 || missing+=("curl")
command -v docker >/dev/null 2>&1 || missing+=("docker")

if [ ${#missing[@]} -gt 0 ]; then
  echo "✗ Missing prerequisites: ${missing[*]}"
  echo "  Please install: Node.js 22, pnpm 10.28.2, curl, Docker"
  echo "  Go is required only when the source-matched runtime cache misses."
  exit 1
fi

if [ -n "${ENV_FILE:-}" ]; then
  if [ ! -f "$ENV_FILE" ]; then
    echo "✗ Configured env file does not exist: $ENV_FILE"
    exit 1
  fi
elif [ -f .git ]; then
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

if [ ! -d node_modules ]; then
  echo "==> Installing dependencies..."
  pnpm install
fi

set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

# Derive the same port and local-origin defaults used by every other Make
# entrypoint. This also keeps older generated .env.worktree files usable.
# shellcheck disable=SC1091
. scripts/local-env.sh

# Prepare source-matched Go artifacts. A complete cache hit does not need Go;
# a miss fails explicitly instead of selecting a stale release/PATH binary.
node apps/desktop/scripts/prepare-dev-runtime.mjs

runtime_suffix=""
if [ "$(node -p 'process.platform')" = "win32" ]; then
  runtime_suffix=".exe"
fi
dev_backend="$REPO_ROOT/.patchbay-dev/bin/server${runtime_suffix}"
dev_migrate="$REPO_ROOT/.patchbay-dev/bin/migrate${runtime_suffix}"

bash scripts/ensure-postgres.sh "$ENV_FILE"

echo "==> Running migrations..."
(cd server && "$dev_migrate" up)

backend_pid=""
cleanup() {
  if [ -n "$backend_pid" ] && kill -0 "$backend_pid" >/dev/null 2>&1; then
    kill "$backend_pid" >/dev/null 2>&1 || true
    wait "$backend_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

backend_url="http://127.0.0.1:${PORT:-8080}"
backend_ready_url="$backend_url/readyz"
echo "==> Starting Go backend at $backend_url"
(cd server && "$dev_backend") &
backend_pid=$!

deadline=$((SECONDS + 1800))
until curl --fail --silent --show-error --max-time 5 "$backend_ready_url" >/dev/null 2>&1; do
  if ! kill -0 "$backend_pid" >/dev/null 2>&1; then
    echo "✗ Go backend exited before readiness: $backend_ready_url" >&2
    exit 1
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    echo "✗ Go backend did not become ready within 30 minutes: $backend_ready_url" >&2
    exit 1
  fi
  sleep 1
done

export VITE_API_URL="$backend_url"
export VITE_WS_URL="ws://127.0.0.1:${PORT:-8080}/ws"
export PATCHBAY_REQUIRE_SOURCE_CLI=1
export PATCHBAY_DEV_ENV_FILE="$ENV_FILE"

echo "✓ Go backend ready. Starting Electron with the source-matched CLI."
node apps/desktop/scripts/dev.mjs
