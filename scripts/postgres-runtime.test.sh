#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

stub_dir="$tmp_dir/bin"
mkdir -p "$stub_dir"
tool_log="$tmp_dir/tools.log"

cat >"$stub_dir/docker" <<'STUB'
#!/usr/bin/env bash
exit 1
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
PATH="$stub_dir:$PATH" POSTGRES_TEST_LOG="$tool_log" \
  bash "$repo_root/scripts/ensure-postgres.sh" "$env_file" >"$output"
grep -Fq "PostgreSQL ready (local native)" "$output"
if [ -s "$tool_log" ]; then
  echo "native PostgreSQL preflight created an existing database" >&2
  exit 1
fi

PATH="$stub_dir:$PATH" POSTGRES_TEST_LOG="$tool_log" POSTGRES_TEST_DB_EXISTS=0 \
  bash "$repo_root/scripts/ensure-postgres.sh" "$env_file" >"$output"
grep -Fq -- "createdb -h 127.0.0.1 -p 55432 -U patchbay --owner patchbay patchbay_native_test" "$tool_log"

: >"$tool_log"
printf 'y\n' | PATH="$stub_dir:$PATH" POSTGRES_TEST_LOG="$tool_log" \
  bash "$repo_root/scripts/drop-database.sh" "$env_file" >"$output"
grep -Fq -- "dropdb -h 127.0.0.1 -p 55432 --username patchbay --maintenance-db postgres --if-exists --force -- patchbay_native_test" "$tool_log"
grep -Fq "Dropped database 'patchbay_native_test'." "$output"

echo "native PostgreSQL development runtime tests passed"
