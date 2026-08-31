#!/usr/bin/env bash

postgres_docker_available() {
  command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1
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
