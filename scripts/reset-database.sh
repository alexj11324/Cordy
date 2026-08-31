#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck disable=SC1091
. "$root_dir/scripts/postgres-runtime.sh"
env_input="${1:-.env}"

if [[ "$env_input" = /* ]]; then
  env_file="$env_input"
else
  env_file="$PWD/$env_input"
fi

if [ ! -f "$env_file" ]; then
  echo "Missing env file: $env_input" >&2
  exit 1
fi

set -a
# shellcheck disable=SC1090
. "$env_file"
set +a

POSTGRES_DB="${POSTGRES_DB:-patchbay}"
POSTGRES_USER="${POSTGRES_USER:-patchbay}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
DATABASE_URL="${DATABASE_URL:-}"
export PGPASSWORD="$POSTGRES_PASSWORD"

postgres_validate_local_reset "$DATABASE_URL" "$POSTGRES_PORT" "$POSTGRES_DB"

# Keep provider startup inside the already-validated reset workflow. The
# local-only guard is intentionally repeated by ensure-postgres so a later
# refactor cannot begin waiting on a remote endpoint before refusing it.
bash "$root_dir/scripts/ensure-postgres.sh" "$env_file" --local-only

cd "$root_dir"
echo "==> Dropping and recreating database '$POSTGRES_DB'..."
postgres_reset_database "$DATABASE_URL" "$POSTGRES_PORT" "$POSTGRES_USER" "$POSTGRES_DB"
