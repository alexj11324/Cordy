#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
. "$script_dir/postgres-runtime.sh"

ENV_FILE="${1:-.env}"
LOCAL_ONLY_MODE="${2:-}"

if [ -n "$LOCAL_ONLY_MODE" ] && [ "$LOCAL_ONLY_MODE" != "--local-only" ]; then
  echo "Unknown option: $LOCAL_ONLY_MODE" >&2
  exit 1
fi

if [ ! -f "$ENV_FILE" ]; then
  echo "Missing env file: $ENV_FILE"
  echo "Create .env from .env.example, or run 'make worktree-env' and use .env.worktree."
  exit 1
fi

set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

POSTGRES_DB="${POSTGRES_DB:-patchbay}"
POSTGRES_USER="${POSTGRES_USER:-patchbay}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-}"
DATABASE_URL="${DATABASE_URL:-}"

export PGPASSWORD="$POSTGRES_PASSWORD"

db_timeout_seconds="${PATCHBAY_DEV_DB_TIMEOUT_SECONDS:-120}"
if [[ ! "$db_timeout_seconds" =~ ^[1-9][0-9]*$ ]]; then
  echo "PATCHBAY_DEV_DB_TIMEOUT_SECONDS must be a positive number of seconds." >&2
  exit 1
fi

wait_until_ready() {
  local description=$1 remediation=$2
  shift 2
  local deadline=$((SECONDS + db_timeout_seconds))
  until "$@" >/dev/null 2>&1; do
    if [ "$SECONDS" -ge "$deadline" ]; then
      echo "✗ PostgreSQL did not become ready at $description within ${db_timeout_seconds}s." >&2
      echo "  $remediation" >&2
      return 1
    fi
    sleep 1
  done
}

db_host=""
db_port="${POSTGRES_PORT:-5432}"
db_name="$POSTGRES_DB"

parse_database_url() {
  local rest authority hostport path port_part

  rest="${DATABASE_URL#*://}"
  rest="${rest%%\?*}"
  authority="${rest%%/*}"
  path="${rest#*/}"

  if [ "$authority" = "$rest" ]; then
    path=""
  fi

  hostport="${authority##*@}"

  if [[ "$hostport" == \[* ]]; then
    db_host="${hostport#\[}"
    db_host="${db_host%%]*}"
    port_part="${hostport#*\]}"
    if [[ "$port_part" == :* ]] && [ -n "${port_part#:}" ]; then
      db_port="${port_part#:}"
    fi
  else
    db_host="${hostport%%:*}"
    if [[ "$hostport" == *:* ]] && [ -n "${hostport##*:}" ]; then
      db_port="${hostport##*:}"
    fi
  fi

  if [ -n "$path" ]; then
    db_name="${path%%/*}"
  fi
}

if [ -n "$DATABASE_URL" ]; then
  parse_database_url
fi

is_local() {
  [ -z "$DATABASE_URL" ] || postgres_host_is_local "$db_host"
}

if [ "$LOCAL_ONLY_MODE" = "--local-only" ] && ! is_local; then
  echo "Refusing local PostgreSQL setup: DATABASE_URL points at a remote host." >&2
  exit 1
fi

if is_local; then
  postgres_provider="$(postgres_runtime_provider "$DATABASE_URL" "$db_port")"
  if [ "$postgres_provider" = "docker" ]; then
    # ---------- Local Docker ----------
    echo "==> Ensuring shared PostgreSQL container is running on localhost:5432..."
    docker compose up -d postgres

    echo "==> Waiting for PostgreSQL to be ready..."
    if ! wait_until_ready \
      "localhost:5432" \
      "Run 'docker compose ps postgres' and 'docker compose logs postgres'." \
      docker compose exec -T postgres pg_isready -U "$POSTGRES_USER" -d postgres; then
      docker compose ps postgres >&2 || true
      exit 1
    fi

    echo "==> Ensuring database '$POSTGRES_DB' exists..."
    db_exists="$(docker compose exec -T postgres \
      psql -U "$POSTGRES_USER" -d postgres -Atqc "SELECT 1 FROM pg_database WHERE datname = '$POSTGRES_DB'")"

    if [ "$db_exists" != "1" ]; then
      docker compose exec -T postgres \
        psql -U "$POSTGRES_USER" -d postgres -v ON_ERROR_STOP=1 \
        -c "CREATE DATABASE \"$POSTGRES_DB\"" \
        > /dev/null
    fi

    echo "✓ PostgreSQL ready (local Docker). Database: $POSTGRES_DB"
  else
    # ---------- Local native PostgreSQL ----------
    psql_command="$(find_postgres_tool psql || true)"
    ready_command="$(find_postgres_tool pg_isready || true)"
    createdb_command="$(find_postgres_tool createdb || true)"
    if [ -z "$psql_command" ] || [ -z "$ready_command" ] || [ -z "$createdb_command" ]; then
      echo "✗ Native PostgreSQL tools are required for ${db_host:-localhost}:$db_port." >&2
      echo "  Install PostgreSQL, or use localhost:5432 with PATCHBAY_POSTGRES_RUNTIME=docker." >&2
      exit 1
    fi
    if [[ ! "$POSTGRES_DB" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
      echo "✗ Unsafe local database name: $POSTGRES_DB" >&2
      exit 1
    fi
    parse_postgres_endpoint "$DATABASE_URL" "$db_port"
    echo "==> Using native PostgreSQL at ${POSTGRES_RUNTIME_HOST}:${POSTGRES_RUNTIME_PORT}..."
    wait_until_ready \
      "${POSTGRES_RUNTIME_HOST}:${POSTGRES_RUNTIME_PORT}" \
      "Verify the native PostgreSQL service and DATABASE_URL." \
      postgres_clean_libpq_routing "$ready_command" -h "$POSTGRES_RUNTIME_HOST" -p "$POSTGRES_RUNTIME_PORT" -U "$POSTGRES_USER" -d postgres
    db_exists="$(postgres_clean_libpq_routing "$psql_command" -X -h "$POSTGRES_RUNTIME_HOST" -p "$POSTGRES_RUNTIME_PORT" -U "$POSTGRES_USER" -d postgres -Atqc "SELECT 1 FROM pg_database WHERE datname = '$POSTGRES_DB'")"
    if [ "$db_exists" != "1" ]; then
      if ! postgres_clean_libpq_routing "$createdb_command" -h "$POSTGRES_RUNTIME_HOST" -p "$POSTGRES_RUNTIME_PORT" -U "$POSTGRES_USER" --owner "$POSTGRES_USER" "$POSTGRES_DB" 2>/dev/null; then
        # Homebrew PostgreSQL commonly makes the macOS account the local
        # cluster administrator while the application role owns databases but
        # cannot create them. The socket fallback stays local and creates only
        # the already-validated checkout database, owned by POSTGRES_USER.
        postgres_clean_libpq_routing env -u PGPASSWORD "$createdb_command" -p "$POSTGRES_RUNTIME_PORT" --owner "$POSTGRES_USER" "$POSTGRES_DB"
      fi
    fi
    echo "✓ PostgreSQL ready (local native). Database: $POSTGRES_DB"
  fi
else
  # ---------- Remote: skip Docker, verify connectivity ----------
  echo "==> Remote database detected (host: $db_host). Skipping Docker."
  if command -v pg_isready > /dev/null 2>&1; then
    echo "==> Waiting for PostgreSQL at $db_host:$db_port to be ready..."
    wait_until_ready \
      "$db_host:$db_port" \
      "Verify DATABASE_URL, network access, and the remote PostgreSQL service." \
      pg_isready -d "$DATABASE_URL"
    echo "✓ PostgreSQL ready (remote: $db_host:$db_port). Database: $db_name"
  else
    echo "==> pg_isready not found. Skipping remote connectivity preflight."
    echo "✓ PostgreSQL configured (remote: $db_host:$db_port). Database: $db_name"
  fi
fi
