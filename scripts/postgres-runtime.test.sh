#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

stub_dir="$tmp_dir/bin"
mkdir -p "$stub_dir"
tool_log="$tmp_dir/tools.log"
docker_log="$tmp_dir/docker.log"

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
exit 0
STUB
cat >"$stub_dir/psql" <<'STUB'
#!/usr/bin/env bash
if [ "${POSTGRES_TEST_DB_EXISTS:-1}" = "1" ]; then
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
chmod +x "$stub_dir"/*

env_file="$tmp_dir/worktree.env"
cat >"$env_file" <<'ENV'
POSTGRES_DB=patchbay_native_test
POSTGRES_USER=patchbay
POSTGRES_PASSWORD=patchbay
POSTGRES_PORT=55432
DATABASE_URL=postgres://patchbay:patchbay@127.0.0.1:55432/patchbay_native_test?sslmode=disable
ENV

output="$tmp_dir/output"
PATH="$stub_dir:$PATH" POSTGRES_TEST_LOG="$tool_log" POSTGRES_DOCKER_TEST_LOG="$docker_log" \
  bash "$repo_root/scripts/ensure-postgres.sh" "$env_file" >"$output"
grep -Fq "PostgreSQL ready (local native)" "$output"
if [ -s "$docker_log" ]; then
  echo "custom native endpoint probed or used Docker" >&2
  exit 1
fi
if [ -s "$tool_log" ]; then
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
cat >"$default_env" <<'ENV'
POSTGRES_DB=patchbay_native_default
POSTGRES_USER=patchbay
POSTGRES_PASSWORD=patchbay
POSTGRES_PORT=5432
DATABASE_URL=postgres://patchbay:patchbay@127.0.0.1:5432/patchbay_native_default?sslmode=disable
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

echo "native PostgreSQL development runtime tests passed"
