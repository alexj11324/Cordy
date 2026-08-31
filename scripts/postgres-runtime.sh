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

postgres_runtime_provider() {
  local database_url="$1"
  local default_port="$2"
  local mode="${PATCHBAY_POSTGRES_RUNTIME:-auto}"

  parse_postgres_endpoint "$database_url" "$default_port"
  case "$mode" in
    auto)
      if { [ "$POSTGRES_RUNTIME_HOST" = "localhost" ] || [ "$POSTGRES_RUNTIME_HOST" = "127.0.0.1" ] || [ "$POSTGRES_RUNTIME_HOST" = "::1" ]; } &&
        [ "$POSTGRES_RUNTIME_PORT" = "5432" ] && postgres_docker_available; then
        printf 'docker\n'
      else
        printf 'native\n'
      fi
      ;;
    docker)
      if ! { [ "$POSTGRES_RUNTIME_HOST" = "localhost" ] || [ "$POSTGRES_RUNTIME_HOST" = "127.0.0.1" ] || [ "$POSTGRES_RUNTIME_HOST" = "::1" ]; } ||
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
