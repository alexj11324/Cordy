#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

stub_dir="$tmp_dir/bin"
mkdir -p "$stub_dir"
tool_log="$tmp_dir/tools.log"
docker_log="$tmp_dir/docker.log"
fixture_value="patchbay"

cat >"$stub_dir/docker" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf 'docker %s\n' "$*" >>"$POSTGRES_DOCKER_TEST_LOG"
case "$*" in
  "compose version") exit 0 ;;
  "info")
    if [ "${POSTGRES_DOCKER_INFO_FAIL:-0}" = "1" ]; then exit 1; fi
    exit 0
    ;;
  *) exit 0 ;;
esac
STUB
cat >"$stub_dir/pg_isready" <<'STUB'
#!/usr/bin/env bash
printf 'pg_isready %s\n' "$*" >>"$POSTGRES_TEST_LOG"
if [ "${POSTGRES_TEST_PG_READY_FAIL:-0}" = "1" ]; then
  exit 1
fi
exit 0
STUB
cat >"$stub_dir/psql" <<'STUB'
#!/usr/bin/env bash
printf 'psql %s\n' "$*" >>"$POSTGRES_TEST_LOG"
if [[ "$*" == *"rolcreatedb OR rolsuper"* ]] || [[ "$*" == *"rolname = current_user AND rolsuper"* ]] || [[ "$*" == *"DROP DATABASE"* ]]; then
  route_mode=socket
  if [[ "$*" == *"-h "* ]]; then route_mode=app; fi
  printf 'route-env mode=%s PGHOST=%s PGHOSTADDR=%s PGPORT=%s PGDATABASE=%s PGUSER=%s PGSERVICE=%s PGSERVICEFILE=%s PGSYSCONFDIR=%s PGPASSWORD=%s\n' \
    "$route_mode" \
    "${PGHOST-}" "${PGHOSTADDR-}" "${PGPORT-}" "${PGDATABASE-}" "${PGUSER-}" \
    "${PGSERVICE-}" "${PGSERVICEFILE-}" "${PGSYSCONFDIR-}" "${PGPASSWORD-}" >>"$POSTGRES_TEST_LOG"
fi
if [[ "$*" == *"rolcreatedb OR rolsuper"* ]]; then
  if [ "${POSTGRES_TEST_APP_CAN_CREATE:-1}" = "1" ]; then printf '1\n'; fi
  exit 0
fi
if [[ "$*" == *"rolname = current_user AND rolsuper"* ]]; then
  if [ "${POSTGRES_TEST_ADMIN_IS_SUPERUSER:-1}" = "1" ]; then printf '1\n'; fi
  exit 0
fi
if [[ "$*" == *"SELECT 1 FROM pg_database"* ]] && [ "${POSTGRES_TEST_DB_EXISTS:-1}" = "1" ]; then
  printf '1\n'
fi
STUB
cat >"$stub_dir/createdb" <<'STUB'
#!/usr/bin/env bash
printf 'createdb %s\n' "$*" >>"$POSTGRES_TEST_LOG"
STUB
cat >"$stub_dir/dropdb" <<'STUB'
#!/usr/bin/env bash
printf 'dropdb %s\n' "$*" >>"$POSTGRES_TEST_LOG"
STUB
cat >"$stub_dir/migrate" <<'STUB'
#!/usr/bin/env bash
printf 'migrate %s\n' "$*" >>"$POSTGRES_TEST_LOG"
STUB
chmod +x "$stub_dir"/*

env_file="$tmp_dir/worktree.env"
cat >"$env_file" <<ENV
POSTGRES_DB=patchbay_native_test
POSTGRES_USER=patchbay
POSTGRES_PASSWORD=$fixture_value
POSTGRES_PORT=55432
DATABASE_URL=postgres://patchbay:\${POSTGRES_PASSWORD}@127.0.0.1:55432/patchbay_native_test?sslmode=disable
ENV

output="$tmp_dir/output"
PATH="$stub_dir:$PATH" POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" \
  bash "$repo_root/scripts/ensure-postgres.sh" "$env_file" >"$output"
grep -Fq "PostgreSQL ready (local native)" "$output"
if [ -s "$docker_log" ]; then
  echo "custom native endpoint probed or used Docker" >&2
  exit 1
fi
if grep -Fq 'createdb ' "$tool_log"; then
  echo "native PostgreSQL preflight created an existing database" >&2
  exit 1
fi

PATH="$stub_dir:$PATH" POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" POSTGRES_TEST_DB_EXISTS=0 \
  bash "$repo_root/scripts/ensure-postgres.sh" "$env_file" >"$output"
grep -Fq -- "createdb -h 127.0.0.1 -p 55432 -U patchbay --owner patchbay patchbay_native_test" "$tool_log"

: >"$tool_log"
printf 'y\n' | PATH="$stub_dir:$PATH" POSTGRES_TEST_LOG="$tool_log" \
  POSTGRES_DOCKER_TEST_LOG="$docker_log" \
  bash "$repo_root/scripts/drop-database.sh" "$env_file" >"$output"
grep -Fq -- "dropdb -h 127.0.0.1 -p 55432 --username patchbay --maintenance-db postgres --if-exists --force -- patchbay_native_test" "$tool_log"
grep -Fq "Dropped database 'patchbay_native_test'." "$output"

default_env="$tmp_dir/default.env"
cat >"$default_env" <<ENV
POSTGRES_DB=patchbay_native_default
POSTGRES_USER=patchbay
POSTGRES_PASSWORD=$fixture_value
POSTGRES_PORT=5432
DATABASE_URL=postgres://patchbay:\${POSTGRES_PASSWORD}@127.0.0.1:5432/patchbay_native_default?sslmode=disable
ENV

: >"$docker_log"
PATCHBAY_POSTGRES_RUNTIME=native PATH="$stub_dir:$PATH" \
  POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" \
  bash "$repo_root/scripts/ensure-postgres.sh" "$default_env" >"$output"
grep -Fq "PostgreSQL ready (local native)" "$output"
if [ -s "$docker_log" ]; then
  echo "explicit native mode invoked Docker" >&2
  exit 1
fi

: >"$docker_log"
POSTGRES_DOCKER_INFO_FAIL=1 PATH="$stub_dir:$PATH" \
  POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" \
  bash "$repo_root/scripts/ensure-postgres.sh" "$default_env" >"$output"
grep -Fq "PostgreSQL ready (local native)" "$output"
grep -Fq "docker info" "$docker_log"

if PATCHBAY_POSTGRES_RUNTIME=docker PATH="$stub_dir:$PATH" \
  POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" \
  bash "$repo_root/scripts/ensure-postgres.sh" "$env_file" >"$output" 2>&1; then
  echo "explicit Docker mode accepted a custom native endpoint" >&2
  exit 1
fi
grep -Fq "requires localhost:5432" "$output"

reset_env="$tmp_dir/reset.env"
cat >"$reset_env" <<ENV
POSTGRES_DB=patchbay_reset_test
POSTGRES_USER=patchbay
POSTGRES_PASSWORD=$fixture_value
POSTGRES_PORT=55432
DATABASE_URL=postgres://patchbay:\${POSTGRES_PASSWORD}@127.0.0.1:55432/patchbay_reset_test?sslmode=disable
ENV

: >"$tool_log"
: >"$docker_log"
PGHOST=db.example.test PGHOSTADDR=203.0.113.10 PGPORT=6432 \
  PGDATABASE=remote_database PGUSER=remote_admin PGSERVICE=remote_service \
  PGSERVICEFILE=/tmp/remote-service.conf PGSYSCONFDIR=/tmp/remote-service-dir \
  PSQLRC=/tmp/remote.psqlrc PATH="$stub_dir:$PATH" \
  POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" \
  bash "$repo_root/scripts/reset-database.sh" "$reset_env" >"$output"
grep -Fq -- 'psql -X -h 127.0.0.1 -p 55432 -U patchbay -d postgres -Atqc SELECT 1 FROM pg_roles WHERE rolname = current_user AND (rolcreatedb OR rolsuper);' "$tool_log"
grep -Fq -- 'psql -X -h 127.0.0.1 -p 55432 -U patchbay -d postgres -v ON_ERROR_STOP=1 -c DROP DATABASE IF EXISTS "patchbay_reset_test" WITH (FORCE); -c CREATE DATABASE "patchbay_reset_test" OWNER "patchbay";' "$tool_log"
if [ "$(grep -Fc 'route-env mode=app PGHOST= PGHOSTADDR= PGPORT= PGDATABASE= PGUSER= PGSERVICE= PGSERVICEFILE= PGSYSCONFDIR= PGPASSWORD=patchbay' "$tool_log")" -ne 2 ]; then
  echo "application-role reset inherited libpq routing environment" >&2
  exit 1
fi
if [ -s "$docker_log" ]; then
  echo "auto-selected native reset invoked Docker" >&2
  exit 1
fi

: >"$tool_log"
: >"$docker_log"
PATCHBAY_POSTGRES_RUNTIME=native PATH="$stub_dir:$PATH" \
  POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" \
  bash "$repo_root/scripts/reset-database.sh" "$default_env" >"$output"
grep -Fq -- 'psql -X -h 127.0.0.1 -p 5432 -U patchbay -d postgres -v ON_ERROR_STOP=1 -c DROP DATABASE IF EXISTS "patchbay_native_default" WITH (FORCE); -c CREATE DATABASE "patchbay_native_default" OWNER "patchbay";' "$tool_log"
if [ -s "$docker_log" ]; then
  echo "explicit native reset invoked Docker" >&2
  exit 1
fi

: >"$tool_log"
: >"$docker_log"
PATH="$stub_dir:$PATH" POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" \
  bash "$repo_root/scripts/reset-database.sh" "$default_env" >"$output"
grep -Fq -- 'docker compose exec -T postgres psql -U patchbay -d postgres -v ON_ERROR_STOP=1 -c DROP DATABASE IF EXISTS "patchbay_native_default" WITH (FORCE); -c CREATE DATABASE "patchbay_native_default";' "$docker_log"
if [ -s "$tool_log" ]; then
  echo "Docker reset invoked native psql" >&2
  exit 1
fi

remote_reset_env="$tmp_dir/remote-reset.env"
cat >"$remote_reset_env" <<ENV
POSTGRES_DB=patchbay_remote_reset
POSTGRES_USER=patchbay
POSTGRES_PASSWORD=$fixture_value
POSTGRES_PORT=5432
DATABASE_URL=postgres://patchbay:\${POSTGRES_PASSWORD}@localhost:5432@db.example.test:5432/patchbay_remote_reset
ENV

: >"$tool_log"
: >"$docker_log"
if PATH="$stub_dir:$PATH" POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" \
  bash "$repo_root/scripts/reset-database.sh" "$remote_reset_env" >"$output" 2>&1; then
  echo "reset accepted a remote endpoint containing a localhost userinfo fragment" >&2
  exit 1
fi
grep -Fq "DATABASE_URL points at a remote host" "$output"
if [ -s "$tool_log" ] || [ -s "$docker_log" ]; then
  echo "remote reset invoked a PostgreSQL provider" >&2
  exit 1
fi

if printf 'y\n' | PATH="$stub_dir:$PATH" POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" \
  bash "$repo_root/scripts/drop-database.sh" "$remote_reset_env" >"$output" 2>&1; then
  echo "drop accepted a remote endpoint containing a localhost userinfo fragment" >&2
  exit 1
fi
grep -Fq "DATABASE_URL points at a remote host" "$output"
if [ -s "$tool_log" ] || [ -s "$docker_log" ]; then
  echo "remote drop invoked a PostgreSQL provider" >&2
  exit 1
fi

: >"$tool_log"
: >"$docker_log"
if POSTGRES_TEST_PG_READY_FAIL=1 PATH="$stub_dir:$PATH" \
  POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" \
  make --no-print-directory -C "$repo_root" db-reset ENV_FILE="$remote_reset_env" \
    RUST_MIGRATE_CMD="$stub_dir/migrate" >"$output" 2>&1; then
  echo "make db-reset accepted a remote endpoint" >&2
  exit 1
fi
grep -Fq "DATABASE_URL points at a remote host" "$output"
if [ -s "$tool_log" ] || [ -s "$docker_log" ]; then
  echo "make db-reset touched a provider or migration before refusing the remote endpoint" >&2
  exit 1
fi

for protected_database in postgres template0 template1; do
  protected_env="$tmp_dir/protected-$protected_database.env"
  cat >"$protected_env" <<ENV
POSTGRES_DB=$protected_database
POSTGRES_USER=patchbay
POSTGRES_PASSWORD=$fixture_value
POSTGRES_PORT=55432
DATABASE_URL=postgres://patchbay:\${POSTGRES_PASSWORD}@127.0.0.1:55432/$protected_database
ENV
  if PATH="$stub_dir:$PATH" POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" \
    bash "$repo_root/scripts/reset-database.sh" "$protected_env" >"$output" 2>&1; then
    echo "reset accepted protected database '$protected_database'" >&2
    exit 1
  fi
  grep -Fq "Refusing to reset protected PostgreSQL database '$protected_database'" "$output"
done
if [ -s "$tool_log" ] || [ -s "$docker_log" ]; then
  echo "protected database reset invoked a PostgreSQL provider" >&2
  exit 1
fi

: >"$tool_log"
: >"$docker_log"
PGHOST=db.example.test PGHOSTADDR=203.0.113.10 PGPORT=6432 \
  PGDATABASE=remote_database PGUSER=remote_admin PGSERVICE=remote_service \
  PGSERVICEFILE=/tmp/remote-service.conf PGSYSCONFDIR=/tmp/remote-service-dir \
  PGPASSWORD=remote_password \
  POSTGRES_TEST_APP_CAN_CREATE=0 POSTGRES_TEST_ADMIN_IS_SUPERUSER=1 \
  PATCHBAY_POSTGRES_RUNTIME=native PATH="$stub_dir:$PATH" \
  POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" \
  bash "$repo_root/scripts/reset-database.sh" "$reset_env" >"$output"
grep -Fq -- 'psql -X -h 127.0.0.1 -p 55432 -U patchbay -d postgres -Atqc SELECT 1 FROM pg_roles WHERE rolname = current_user AND (rolcreatedb OR rolsuper);' "$tool_log"
grep -Fq -- 'psql -X -p 55432 -d postgres -Atqc SELECT 1 FROM pg_roles WHERE rolname = current_user AND rolsuper;' "$tool_log"
grep -Fq -- 'psql -X -p 55432 -d postgres -v ON_ERROR_STOP=1 -c DROP DATABASE IF EXISTS "patchbay_reset_test" WITH (FORCE); -c CREATE DATABASE "patchbay_reset_test" OWNER "patchbay";' "$tool_log"
if [ "$(grep -Fc 'route-env mode=socket PGHOST= PGHOSTADDR= PGPORT= PGDATABASE= PGUSER= PGSERVICE= PGSERVICEFILE= PGSYSCONFDIR= PGPASSWORD=' "$tool_log")" -ne 2 ]; then
  echo "local socket reset inherited libpq routing environment" >&2
  exit 1
fi
if grep -Fq -- 'psql -X -h 127.0.0.1 -p 55432 -U patchbay -d postgres -v ON_ERROR_STOP=1 -c DROP DATABASE' "$tool_log"; then
  echo "application role without CREATEDB performed the reset" >&2
  exit 1
fi

: >"$tool_log"
: >"$docker_log"
if POSTGRES_TEST_APP_CAN_CREATE=0 POSTGRES_TEST_ADMIN_IS_SUPERUSER=0 \
  PATCHBAY_POSTGRES_RUNTIME=native PATH="$stub_dir:$PATH" \
  POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" \
  bash "$repo_root/scripts/reset-database.sh" "$reset_env" >"$output" 2>&1; then
  echo "reset succeeded without any role capable of recreating the database" >&2
  exit 1
fi
grep -Fq "lacks CREATEDB and no local socket superuser is available" "$output"
if grep -Fq 'DROP DATABASE' "$tool_log" || grep -Fq 'DROP DATABASE' "$docker_log"; then
  echo "reset dropped the database before proving it could recreate it" >&2
  exit 1
fi

make_output="$(make --no-print-directory -n -C "$repo_root" db-reset ENV_FILE="$reset_env")"
grep -Fq -- "bash scripts/reset-database.sh \"$reset_env\"" <<<"$make_output"
if grep -Fq -- 'docker compose exec -T postgres psql' <<<"$make_output"; then
  echo "Makefile db-reset still invokes Docker directly" >&2
  exit 1
fi

echo "native PostgreSQL development runtime tests passed"
