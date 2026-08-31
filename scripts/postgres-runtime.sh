#!/usr/bin/env bash

postgres_docker_available() {
  command -v docker >/dev/null 2>&1 &&
    docker compose version >/dev/null 2>&1 &&
    docker info >/dev/null 2>&1
}

find_postgres_tool() {
  local name="$1"
  local candidate
  if command -v "$name" >/dev/null 2>&1; then
    command -v "$name"
    return 0
  fi
  for candidate in \
    "/opt/homebrew/opt/postgresql@17/bin/$name" \
    "/opt/homebrew/opt/postgresql@16/bin/$name" \
    "/opt/homebrew/opt/postgresql@15/bin/$name" \
    "/usr/local/opt/postgresql@17/bin/$name" \
    "/usr/local/opt/postgresql@16/bin/$name" \
    "/usr/local/opt/postgresql@15/bin/$name"; do
    if [ -x "$candidate" ]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

parse_postgres_endpoint() {
  local database_url="$1"
  local default_port="$2"
  local rest authority hostport port_part

  POSTGRES_RUNTIME_HOST="localhost"
  POSTGRES_RUNTIME_PORT="$default_port"
  if [ -z "$database_url" ]; then
    return
  fi

  rest="${database_url#*://}"
  rest="${rest%%\?*}"
  authority="${rest%%/*}"
  hostport="${authority##*@}"
  if [[ "$hostport" == \[* ]]; then
    POSTGRES_RUNTIME_HOST="${hostport#\[}"
    POSTGRES_RUNTIME_HOST="${POSTGRES_RUNTIME_HOST%%]*}"
    port_part="${hostport#*\]}"
    if [[ "$port_part" == :* ]] && [ -n "${port_part#:}" ]; then
      POSTGRES_RUNTIME_PORT="${port_part#:}"
    fi
  else
    POSTGRES_RUNTIME_HOST="${hostport%%:*}"
    if [[ "$hostport" == *:* ]] && [ -n "${hostport##*:}" ]; then
      POSTGRES_RUNTIME_PORT="${hostport##*:}"
    fi
  fi
  export POSTGRES_RUNTIME_HOST POSTGRES_RUNTIME_PORT
}

postgres_host_is_local() {
  case "$1" in
    localhost | 127.0.0.1 | ::1) return 0 ;;
    *) return 1 ;;
  esac
}

postgres_quote_identifier() {
  local identifier="$1"
  identifier="${identifier//\"/\"\"}"
  printf '"%s"' "$identifier"
}

postgres_clean_libpq_routing() {
  # libpq reads connection routing from the environment even when psql has no
  # -h flag, and PGHOSTADDR can override the actual network address even when
  # -h is explicit. Strip every ambient selector; callers provide the intended
  # host, port, database and user as command arguments.
  env \
    -u PGHOST \
    -u PGHOSTADDR \
    -u PGPORT \
    -u PGDATABASE \
    -u PGUSER \
    -u PGSERVICE \
    -u PGSERVICEFILE \
    -u PGSYSCONFDIR \
    "$@"
}

postgres_local_socket_psql() {
  postgres_clean_libpq_routing env -u PGPASSWORD "$@"
}

postgres_validate_local_reset() {
  local database_url="$1"
  local default_port="$2"
  local postgres_database="$3"

  if [[ ! "$postgres_database" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
    echo "Unsafe local database name: $postgres_database" >&2
    return 1
  fi

  case "$postgres_database" in
    postgres | template0 | template1)
      echo "Refusing to reset protected PostgreSQL database '$postgres_database'." >&2
      return 1
      ;;
  esac

  parse_postgres_endpoint "$database_url" "$default_port"
  if ! postgres_host_is_local "$POSTGRES_RUNTIME_HOST"; then
    echo "Refusing to reset database '$postgres_database': DATABASE_URL points at a remote host." >&2
    return 1
  fi
}

postgres_runtime_provider() {
  local database_url="$1"
  local default_port="$2"
  local mode="${PATCHBAY_POSTGRES_RUNTIME:-auto}"

  parse_postgres_endpoint "$database_url" "$default_port"
  case "$mode" in
    auto)
      if postgres_host_is_local "$POSTGRES_RUNTIME_HOST" &&
        [ "$POSTGRES_RUNTIME_PORT" = "5432" ] && postgres_docker_available; then
        printf 'docker\n'
      else
        printf 'native\n'
      fi
      ;;
    docker)
      if ! postgres_host_is_local "$POSTGRES_RUNTIME_HOST" ||
        [ "$POSTGRES_RUNTIME_PORT" != "5432" ]; then
        echo "PATCHBAY_POSTGRES_RUNTIME=docker requires localhost:5432; configured endpoint is ${POSTGRES_RUNTIME_HOST}:${POSTGRES_RUNTIME_PORT}." >&2
        return 1
      fi
      if ! postgres_docker_available; then
        echo "PATCHBAY_POSTGRES_RUNTIME=docker but Docker Compose or its daemon is unavailable." >&2
        return 1
      fi
      printf 'docker\n'
      ;;
    native)
      printf 'native\n'
      ;;
    *)
      echo "PATCHBAY_POSTGRES_RUNTIME must be auto, docker, or native (received '$mode')." >&2
      return 1
      ;;
  esac
}

postgres_reset_database() {
  local database_url="$1"
  local default_port="$2"
  local postgres_user="$3"
  local postgres_database="$4"
  local postgres_provider psql_command app_can_create admin_is_superuser
  local database_identifier owner_identifier

  postgres_validate_local_reset "$database_url" "$default_port" "$postgres_database"

  postgres_provider="$(postgres_runtime_provider "$database_url" "$default_port")"
  if [ "$postgres_provider" = "docker" ]; then
    docker compose exec -T postgres \
      psql -U "$postgres_user" -d postgres -v ON_ERROR_STOP=1 \
      -c "DROP DATABASE IF EXISTS \"$postgres_database\" WITH (FORCE);" \
      -c "CREATE DATABASE \"$postgres_database\";"
    return
  fi

  psql_command="$(find_postgres_tool psql || true)"
  if [ -z "$psql_command" ]; then
    echo "Cannot reset '$postgres_database': native psql is unavailable." >&2
    return 1
  fi
  database_identifier="$(postgres_quote_identifier "$postgres_database")"
  owner_identifier="$(postgres_quote_identifier "$postgres_user")"

  # A database owner may DROP its database without having CREATEDB. Prove the
  # same connection can recreate it before executing either statement.
  if ! app_can_create="$(
    postgres_clean_libpq_routing "$psql_command" \
      -X \
      -h "$POSTGRES_RUNTIME_HOST" \
      -p "$POSTGRES_RUNTIME_PORT" \
      -U "$postgres_user" \
      -d postgres \
      -Atqc "SELECT 1 FROM pg_roles WHERE rolname = current_user AND (rolcreatedb OR rolsuper);"
  )"; then
    echo "Cannot reset '$postgres_database': failed to verify CREATEDB for PostgreSQL role '$postgres_user'." >&2
    return 1
  fi

  if [ "$app_can_create" = "1" ]; then
    postgres_clean_libpq_routing "$psql_command" \
      -X \
      -h "$POSTGRES_RUNTIME_HOST" \
      -p "$POSTGRES_RUNTIME_PORT" \
      -U "$postgres_user" \
      -d postgres \
      -v ON_ERROR_STOP=1 \
      -c "DROP DATABASE IF EXISTS $database_identifier WITH (FORCE);" \
      -c "CREATE DATABASE $database_identifier OWNER $owner_identifier;"
    return
  fi

  # Homebrew commonly gives the macOS account a local socket superuser while
  # the application role only owns its database. Verify that administrator
  # before DROP, then use the same proven connection for DROP and CREATE.
  admin_is_superuser="$(
    postgres_local_socket_psql "$psql_command" \
      -X \
      -p "$POSTGRES_RUNTIME_PORT" \
      -d postgres \
      -Atqc "SELECT 1 FROM pg_roles WHERE rolname = current_user AND rolsuper;" \
      2>/dev/null || true
  )"
  if [ "$admin_is_superuser" != "1" ]; then
    echo "Cannot reset '$postgres_database': role '$postgres_user' lacks CREATEDB and no local socket superuser is available." >&2
    return 1
  fi

  postgres_local_socket_psql "$psql_command" \
    -X \
    -p "$POSTGRES_RUNTIME_PORT" \
    -d postgres \
    -v ON_ERROR_STOP=1 \
    -c "DROP DATABASE IF EXISTS $database_identifier WITH (FORCE);" \
    -c "CREATE DATABASE $database_identifier OWNER $owner_identifier;"
}
