#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Build a self-contained sandbox with stub `curl` and a tarball that the
# release-binary installation path will download.
_setup_sandbox() {
  local tmp="$1"
  local stub_bin="$tmp/stub-bin"
  local install_bin="$tmp/install-bin"
  local payload_dir="$tmp/payload"
  mkdir -p "$stub_bin" "$install_bin" "$payload_dir"

  cat >"$payload_dir/patchbay" <<'STUB'
#!/usr/bin/env bash
echo "patchbay v0.3.2 (commit: test)"
STUB
  chmod +x "$payload_dir/patchbay"
  tar -czf "$tmp/patchbay.tar.gz" -C "$payload_dir" patchbay

  cat >"$stub_bin/curl" <<'STUB'
#!/usr/bin/env bash
if [[ "$*" == *"-sI"* ]]; then
  printf 'HTTP/2 302\r\nlocation: https://github.com/alexj11324/Cordy/releases/tag/v0.3.2\r\n'
  exit 0
fi

out=""
url=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -o)
      out="$2"
      shift 2
      ;;
    http*)
      url="$1"
      shift
      ;;
    *)
      shift
      ;;
  esac
done

if [[ -z "$out" ]]; then
  echo "stub curl expected -o" >&2
  exit 2
fi
if [[ "$url" == */checksums.txt ]]; then
  asset="$(cat "$PATCHBAY_TEST_ASSET_FILE")"
  if command -v sha256sum >/dev/null 2>&1; then
    checksum="$(sha256sum "$PATCHBAY_TEST_ARCHIVE" | awk '{print $1}')"
  else
    checksum="$(shasum -a 256 "$PATCHBAY_TEST_ARCHIVE" | awk '{print $1}')"
  fi
  printf '%s  %s\n' "$checksum" "$asset" >"$out"
else
  printf '%s' "${url##*/}" >"$PATCHBAY_TEST_ASSET_FILE"
  cp "$PATCHBAY_TEST_ARCHIVE" "$out"
fi
STUB
  chmod +x "$stub_bin/curl"
}

_run_installer() {
  local tmp="$1"
  local out="$tmp/install.out"
  local err="$tmp/install.err"
  if ! PATH="$tmp/stub-bin:$tmp/install-bin:/usr/bin:/bin" \
    PATCHBAY_BIN_DIR="$tmp/install-bin" \
    PATCHBAY_TEST_ARCHIVE="$tmp/patchbay.tar.gz" \
    PATCHBAY_TEST_ASSET_FILE="$tmp/release-asset" \
    bash "$ROOT_DIR/scripts/install.sh" >"$out" 2>"$err"; then
    echo "install.sh exited non-zero" >&2
    cat "$out" >&2 || true
    cat "$err" >&2 || true
    return 1
  fi

  if [[ ! -x "$tmp/install-bin/patchbay" ]]; then
    echo "expected fallback binary at $tmp/install-bin/patchbay" >&2
    cat "$out" >&2 || true
    cat "$err" >&2 || true
    return 1
  fi

}

test_release_binary_install() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  _setup_sandbox "$tmp"
  _run_installer "$tmp"
}

test_remote_ssh_install_prints_token_login_hint() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  _setup_sandbox "$tmp"
  cat >"$tmp/stub-bin/brew" <<'STUB'
#!/usr/bin/env bash
case "${1:-}" in
  tap)
    exit 0
    ;;
  install)
    echo "simulated brew install failure" >&2
    exit 42
    ;;
  list)
    exit 1
    ;;
  *)
    exit 0
    ;;
esac
STUB
  chmod +x "$tmp/stub-bin/brew"

  (
    export SSH_CONNECTION="192.0.2.10 54321 198.51.100.20 22"
    _run_installer "$tmp"
  )

  if ! grep -q "Looks like a remote/SSH session" "$tmp/install.out"; then
    echo "expected remote/SSH token-login hint in installer output" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
  if ! grep -q "https://aspectlylabs.com/settings?tab=tokens" "$tmp/install.out"; then
    echo "expected direct API Tokens settings URL in installer output" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
  if ! grep -q "Settings > API Tokens" "$tmp/install.out"; then
    echo "expected API Tokens tab name in installer output" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
  if ! grep -q "patchbay login --token <YOUR_TOKEN>" "$tmp/install.out"; then
    echo "expected token login command in installer output" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
  if grep -q "patchbay config set server_url" "$tmp/install.out"; then
    echo "did not expect default cloud server config command in installer output" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
  if grep -q "patchbay config set app_url" "$tmp/install.out"; then
    echo "did not expect default cloud app config command in installer output" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
}

test_local_install_does_not_print_token_login_hint() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  _setup_sandbox "$tmp"
  cat >"$tmp/stub-bin/brew" <<'STUB'
#!/usr/bin/env bash
case "${1:-}" in
  tap)
    exit 0
    ;;
  install)
    echo "simulated brew install failure" >&2
    exit 42
    ;;
  list)
    exit 1
    ;;
  *)
    exit 0
    ;;
esac
STUB
  chmod +x "$tmp/stub-bin/brew"

  (
    unset SSH_CONNECTION SSH_CLIENT SSH_TTY
    _run_installer "$tmp"
  )

  if grep -q "Looks like a remote/SSH session" "$tmp/install.out"; then
    echo "did not expect remote/SSH token-login hint in local installer output" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
  if grep -q "patchbay login --token <YOUR_TOKEN>" "$tmp/install.out"; then
    echo "did not expect token login command in local installer output" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
}

# ---------------------------------------------------------------------------
# --with-server: the probed port and the printed port must both be the port
# Docker Compose reported (#6145)
#
# The installer used to derive the port from .env with its own copy of the alias
# chain. Compose gives the *calling environment* precedence over .env, so any
# ambient PORT / BACKEND_PORT / API_PORT / SERVER_PORT / FRONTEND_PORT moved the
# published port while the installer kept probing and printing the file value.
#
# Here the docker stub plays Compose: it answers `port` from the same resolution
# Compose performs, environment first, then .env. The installer must take that
# answer as given for both the health check and the summary — which is exactly
# what a .env-only derivation cannot do, because the two disagree in every case
# below. That real Compose resolves this way is proven separately, against real
# `docker compose config`, in scripts/selfhost-config.test.sh; that test needs a
# Docker CLI, which this job deliberately does not require.
# ---------------------------------------------------------------------------
_setup_server_sandbox() {
  local tmp="$1"
  local stub_bin="$tmp/stub-bin"
  local server_dir="$tmp/server"
  mkdir -p "$stub_bin" "$server_dir/.git"

  # Minimal self-host assets: only the port mapping matters here.
  cat >"$server_dir/.env.example" <<'ENVFILE'
PORT=8080
# BACKEND_PORT=8080
# API_PORT=8080
# SERVER_PORT=8080
FRONTEND_PORT=3000
JWT_SECRET=change-me-in-production
POSTGRES_PASSWORD=patchbay
DATABASE_URL=postgres://patchbay:patchbay@localhost:5432/patchbay?sslmode=disable
ENVFILE
  touch "$server_dir/docker-compose.selfhost.yml"

  # Compose stand-in. Resolves the published host port the way Compose does:
  # the process environment wins over .env, then the alias chain decides.
  cat >"$stub_bin/docker" <<'STUB'
#!/usr/bin/env bash
set -uo pipefail

_env_file_value() {
  local key="$1" line
  line="$(grep -E "^${key}=" .env 2>/dev/null | tail -n 1 || true)"
  [ -n "$line" ] || return 1
  printf '%s' "${line#*=}"
}

# Environment first (Compose interpolation), then the env file.
_resolve() {
  local key="$1" from_env
  eval "from_env=\${$key-__unset__}"
  if [ "$from_env" != "__unset__" ]; then
    printf '%s' "$from_env"
    return 0
  fi
  _env_file_value "$key"
}

_published_backend_port() {
  local value
  for key in BACKEND_PORT API_PORT SERVER_PORT PORT; do
    if value="$(_resolve "$key")" && [ -n "$value" ]; then
      printf '%s' "$value"
      return
    fi
  done
  printf '8080'
}

_published_frontend_port() {
  local value
  if value="$(_resolve FRONTEND_PORT)" && [ -n "$value" ]; then
    printf '%s' "$value"
    return
  fi
  printf '3000'
}

case "${1:-}" in
  info) exit 0 ;;
  compose)
    shift
    subcommand=""
    for arg in "$@"; do
      case "$arg" in
        pull | up | port | version | ps | logs | down | config) subcommand="$arg"; break ;;
      esac
    done
    case "$subcommand" in
      port)
        service=""
        for arg in "$@"; do
          case "$arg" in
            backend | frontend) service="$arg"; break ;;
          esac
        done
        case "$service" in
          backend) printf '127.0.0.1:%s\n' "$(_published_backend_port)" ;;
          frontend) printf '127.0.0.1:%s\n' "$(_published_frontend_port)" ;;
          *) exit 1 ;;
        esac
        ;;
      config)
        printed_environment=false
        for arg in "$@"; do
          if [ "$arg" = "--environment" ]; then
            printed_environment=true
            for key in PORT BACKEND_PORT API_PORT SERVER_PORT FRONTEND_PORT PATCHBAY_IMAGE_TAG; do
              if value="$(_resolve "$key")"; then
                printf '%s=%s\n' "$key" "$value"
              fi
            done
            break
          fi
        done
        if [ "$printed_environment" = false ]; then
          printf 'services:\n  backend:\n    ports: ["127.0.0.1:%s:8080"]\n  frontend:\n    ports: ["127.0.0.1:%s:3000"]\n' \
            "$(_published_backend_port)" "$(_published_frontend_port)"
        fi
        ;;
      version) echo "2.30.0" ;;
    esac
    exit 0
    ;;
esac
exit 0
STUB
  chmod +x "$stub_bin/docker"

  # git: the installer takes the existing-installation path and now requires
  # both fetch and detached checkout to succeed before changing image tags.
  cat >"$stub_bin/git" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${PATCHBAY_TEST_GIT_LOG:-/dev/null}"
exit 0
STUB
  chmod +x "$stub_bin/git"

  # brew: pretend the CLI installs cleanly so the run reaches the summary.
  printf '#!/usr/bin/env bash\nexit 0\n' >"$stub_bin/brew"
  chmod +x "$stub_bin/brew"

  printf '#!/usr/bin/env bash\necho "patchbay v0.3.2 (commit: test)"\n' >"$stub_bin/patchbay"
  chmod +x "$stub_bin/patchbay"

  # curl records every probed URL so the health-check port can be asserted.
  cat >"$stub_bin/curl" <<'STUB'
#!/usr/bin/env bash
set -uo pipefail
latest_request=false
for arg in "$@"; do
  case "$arg" in
    */releases/latest) latest_request=true ;;
    http*) printf '%s\n' "$arg" >>"$PATCHBAY_TEST_CURL_LOG" ;;
  esac
done
if [ "$latest_request" = true ] && [ -n "${PATCHBAY_TEST_LATEST_TAG:-}" ]; then
  printf 'HTTP/2 302\nlocation: https://github.com/alexj11324/Cordy/releases/tag/%s\n' "$PATCHBAY_TEST_LATEST_TAG"
fi
exit 0
STUB
  chmod +x "$stub_bin/curl"

  printf '#!/usr/bin/env bash\nhead -c 32 /dev/zero | od -An -tx1 | tr -d " \\n"\n' >"$stub_bin/openssl"
  chmod +x "$stub_bin/openssl"
}

# Runs `install.sh --with-server` with the sandbox stubs. Remaining arguments are
# ambient environment assignments, so each case controls the environment
# explicitly instead of inheriting a CI runner's PORT.
_run_with_server() {
  local tmp="$1"
  shift

  : >"$tmp/curl.log"
  : >"$tmp/git.log"
  if ! env -i \
    PATH="$tmp/stub-bin:/usr/bin:/bin" \
    HOME="$tmp" \
    PATCHBAY_INSTALL_DIR="$tmp/server" \
    PATCHBAY_SELFHOST_REF="main" \
    PATCHBAY_TEST_CURL_LOG="$tmp/curl.log" \
    PATCHBAY_TEST_GIT_LOG="$tmp/git.log" \
    "$@" \
    bash "$ROOT_DIR/scripts/install.sh" --with-server \
    >"$tmp/install.out" 2>"$tmp/install.err"; then
    echo "install.sh --with-server exited non-zero" >&2
    cat "$tmp/install.out" >&2 || true
    cat "$tmp/install.err" >&2 || true
    return 1
  fi
}

# Asserts the probed port and the printed ports all match the stub's answer.
_require_server_ports() {
  local tmp="$1" label="$2" expected_backend="$3" expected_frontend="$4"
  local probed printed_backend printed_frontend

  probed="$(sed -n '1s#.*localhost:\([0-9]*\)/health#\1#p' "$tmp/curl.log")"
  printed_backend="$(sed -n 's#.*Backend:[^0-9]*http://localhost:\([0-9]*\).*#\1#p' "$tmp/install.out" | head -n 1)"
  printed_frontend="$(sed -n 's#.*Frontend:[^0-9]*http://localhost:\([0-9]*\).*#\1#p' "$tmp/install.out" | head -n 1)"

  if [ "$probed" != "$expected_backend" ] ||
    [ "$printed_backend" != "$expected_backend" ] ||
    [ "$printed_frontend" != "$expected_frontend" ]; then
    echo "[$label] installer ports disagree with the port Compose published" >&2
    echo "  compose published:  backend=$expected_backend frontend=$expected_frontend" >&2
    echo "  health check probed: ${probed:-<none>}" >&2
    echo "  printed:            backend=${printed_backend:-<none>} frontend=${printed_frontend:-<none>}" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
}

test_with_server_uses_compose_published_ports() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  # label | .env mutation (sed) | ambient env | expected backend | expected frontend
  local cases='defaults|||8080|3000
env-file PORT|s/^PORT=8080/PORT=9100/||9100|3000
env-file BACKEND_PORT|s/^# BACKEND_PORT=8080/BACKEND_PORT=9200/||9200|3000
env-file API_PORT|s/^# API_PORT=8080/API_PORT=9300/||9300|3000
env-file SERVER_PORT|s/^# SERVER_PORT=8080/SERVER_PORT=9400/||9400|3000
env-file FRONTEND_PORT|s/^FRONTEND_PORT=3000/FRONTEND_PORT=3100/||8080|3100
ambient PORT beats .env|s/^PORT=8080/PORT=9100/|PORT=9500|9500|3000
ambient BACKEND_PORT beats .env|s/^PORT=8080/PORT=9100/|BACKEND_PORT=9600|9600|3000
ambient API_PORT beats .env|s/^PORT=8080/PORT=9100/|API_PORT=9700|9700|3000
ambient SERVER_PORT beats .env|s/^PORT=8080/PORT=9100/|SERVER_PORT=9800|9800|3000
ambient FRONTEND_PORT beats .env|s/^FRONTEND_PORT=3000/FRONTEND_PORT=3100/|FRONTEND_PORT=3200|8080|3200
empty ambient BACKEND_PORT falls through|s/^PORT=8080/PORT=9100/|BACKEND_PORT=|9100|3000
empty env-file BACKEND_PORT falls through|s/^PORT=8080/PORT=9100/;s/^# BACKEND_PORT=8080/BACKEND_PORT=/||9100|3000'

  local label mutation ambient expect_backend expect_frontend
  while IFS='|' read -r label mutation ambient expect_backend expect_frontend; do
    [ -n "$label" ] || continue

    rm -rf "$tmp/server" "$tmp/stub-bin"
    _setup_server_sandbox "$tmp"
    cp "$tmp/server/.env.example" "$tmp/server/.env"
    if [ -n "$mutation" ]; then
      sed "$mutation" "$tmp/server/.env" >"$tmp/server/.env.new"
      mv "$tmp/server/.env.new" "$tmp/server/.env"
    fi

    if [ -n "$ambient" ]; then
      _run_with_server "$tmp" "$ambient" || return 1
    else
      _run_with_server "$tmp" || return 1
    fi
    _require_server_ports "$tmp" "$label" "$expect_backend" "$expect_frontend" || return 1
  done <<<"$cases"
}

test_with_server_fails_when_compose_port_is_unavailable() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  _setup_server_sandbox "$tmp"
  cp "$tmp/server/.env.example" "$tmp/server/.env"

  # Compose cannot report a port, e.g. the container never came up.
  cat >"$tmp/stub-bin/docker" <<'STUB'
#!/usr/bin/env bash
set -uo pipefail
case "${1:-}" in
  info) exit 0 ;;
  compose)
    for arg in "$@"; do
      case "$arg" in
        port) exit 1 ;;
        version) echo "2.30.0"; exit 0 ;;
      esac
    done
    exit 0
    ;;
esac
exit 0
STUB
  chmod +x "$tmp/stub-bin/docker"

  if _run_with_server "$tmp" >/dev/null 2>&1; then
    echo "installer must not report success when Compose cannot report the port" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
  if ! grep -q "could not read the backend host port" "$tmp/install.err"; then
    echo "expected an explicit failure about the backend host port" >&2
    cat "$tmp/install.err" >&2 || true
    return 1
  fi
  if grep -q "server is running and CLI is ready" "$tmp/install.out"; then
    echo "installer claimed success despite an unresolved port" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
}

test_with_server_pins_selected_release_images() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  _setup_server_sandbox "$tmp"
  cp "$tmp/server/.env.example" "$tmp/server/.env"

  _run_with_server "$tmp" PATCHBAY_SELFHOST_REF=v0.3.2 PATCHBAY_IMAGE_TAG=ambient || return 1

  if [ "$(grep '^PATCHBAY_IMAGE_TAG=' "$tmp/server/.env")" != "PATCHBAY_IMAGE_TAG=v0.3.2" ]; then
    echo "installer did not pin Compose images to the selected release ref" >&2
    cat "$tmp/server/.env" >&2 || true
    return 1
  fi
  if ! grep -q "Pinned backend and web images to v0.3.2" "$tmp/install.out"; then
    echo "installer did not report the selected production image tag" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
}

test_with_server_preserves_existing_image_pin_without_explicit_ref() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  _setup_server_sandbox "$tmp"
  cp "$tmp/server/.env.example" "$tmp/server/.env"
  printf '\nPATCHBAY_IMAGE_TAG=v0.2.9\n' >>"$tmp/server/.env"

  _run_with_server "$tmp" PATCHBAY_SELFHOST_REF= || return 1

  if [ "$(grep '^PATCHBAY_IMAGE_TAG=' "$tmp/server/.env")" != "PATCHBAY_IMAGE_TAG=v0.2.9" ]; then
    echo "installer replaced an existing image pin without an explicit self-host ref" >&2
    cat "$tmp/server/.env" >&2 || true
    return 1
  fi
  if ! grep -q "Preserved existing backend and web image pin v0.2.9" "$tmp/install.out"; then
    echo "installer did not report the preserved image pin" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
  if ! grep -Fq "fetch origin v0.2.9 --depth 1" "$tmp/git.log"; then
    echo "installer preserved the image pin but did not check out its matching assets" >&2
    cat "$tmp/git.log" >&2 || true
    return 1
  fi
}

test_with_server_preserves_legacy_image_pin_before_brand_migration() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  _setup_server_sandbox "$tmp"
  cp "$tmp/server/.env.example" "$tmp/server/.env"
  printf '\nCORDY_IMAGE_TAG=v0.2.8\n' >>"$tmp/server/.env" # legacy-brand-compat

  _run_with_server "$tmp" PATCHBAY_SELFHOST_REF= || return 1

  if [ "$(grep '^PATCHBAY_IMAGE_TAG=' "$tmp/server/.env")" != "PATCHBAY_IMAGE_TAG=v0.2.8" ]; then
    echo "installer did not preserve and migrate the legacy image pin" >&2
    cat "$tmp/server/.env" >&2 || true
    return 1
  fi
  if ! grep -Fq "fetch origin v0.2.8 --depth 1" "$tmp/git.log"; then
    echo "installer selected assets before reading the legacy image pin" >&2
    cat "$tmp/git.log" >&2 || true
    return 1
  fi
}

test_with_server_resolves_latest_pin_to_matching_release_assets() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  _setup_server_sandbox "$tmp"
  cp "$tmp/server/.env.example" "$tmp/server/.env"
  printf '\nPATCHBAY_IMAGE_TAG=latest\n' >>"$tmp/server/.env"

  _run_with_server "$tmp" PATCHBAY_SELFHOST_REF= PATCHBAY_TEST_LATEST_TAG=v0.3.2 || return 1

  if [ "$(grep '^PATCHBAY_IMAGE_TAG=' "$tmp/server/.env")" != "PATCHBAY_IMAGE_TAG=v0.3.2" ]; then
    echo "installer did not replace the moving latest pin with the resolved release" >&2
    cat "$tmp/server/.env" >&2 || true
    return 1
  fi
  if ! grep -Fq "fetch origin v0.3.2 --depth 1" "$tmp/git.log"; then
    echo "installer did not check out the release assets matching the resolved latest image" >&2
    cat "$tmp/git.log" >&2 || true
    return 1
  fi
}

test_with_server_migrates_legacy_branding_without_overwriting_custom_repositories() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  _setup_server_sandbox "$tmp"
  cp "$tmp/server/.env.example" "$tmp/server/.env"
  printf '\nCORDY_BACKEND_IMAGE=ghcr.io/alexj11324/cordy-backend\nCORDY_WEB_IMAGE=ghcr.io/alexj11324/cordy-web\n' >>"$tmp/server/.env" # legacy-brand-compat

  _run_with_server "$tmp" PATCHBAY_SELFHOST_REF=v0.3.2 || return 1

  grep -Fxq 'PATCHBAY_BACKEND_IMAGE=ghcr.io/alexj11324/patchbay-backend' "$tmp/server/.env" || return 1
  grep -Fxq 'PATCHBAY_WEB_IMAGE=ghcr.io/alexj11324/patchbay-web' "$tmp/server/.env" || return 1

  cp "$tmp/server/.env.example" "$tmp/server/.env"
  printf '\nCORDY_BACKEND_IMAGE=registry.example/custom-backend\nCORDY_WEB_IMAGE=registry.example/custom-web\n' >>"$tmp/server/.env" # legacy-brand-compat

  _run_with_server "$tmp" PATCHBAY_SELFHOST_REF=v0.3.2 || return 1

  grep -Fxq 'PATCHBAY_BACKEND_IMAGE=registry.example/custom-backend' "$tmp/server/.env" || return 1
  grep -Fxq 'PATCHBAY_WEB_IMAGE=registry.example/custom-web' "$tmp/server/.env" || return 1
}

test_default_home_migrates_legacy_systemd_unit() {
  if [[ "$(uname -s)" != "Linux" ]]; then
    echo "skipping Linux-only legacy systemd migration test on $(uname -s)"
    return 0
  fi

  local tmp legacy_home unit_dir legacy_unit legacy_compose legacy_description
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  _setup_server_sandbox "$tmp"
  legacy_home="$tmp/.cordy" # legacy-brand-compat
  mv "$tmp/server" "$legacy_home"
  unit_dir="$tmp/.config/systemd/user"
  legacy_unit="$unit_dir/cordy-selfhost.service" # legacy-brand-compat
  legacy_compose="$legacy_home/server/.cordy-systemd.compose.yml" # legacy-brand-compat
  legacy_description="Cordy self-hosted Rust services" # legacy-brand-compat
  mkdir -p "$unit_dir" "$legacy_home/server"
  touch "$legacy_compose"
  {
    printf '%s\n' '[Unit]'
    printf 'Description=%s\n' "$legacy_description"
    printf '%s\n' '[Service]'
    printf 'WorkingDirectory=%s\n' "$legacy_home/server"
    printf 'ExecStart=docker compose -f %s up -d\n' "$legacy_compose"
  } >"$legacy_unit"
  : >"$tmp/systemctl.log"

  cat >"$tmp/stub-bin/systemctl" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$PATCHBAY_TEST_SYSTEMCTL_LOG"
exit 0
STUB
  chmod +x "$tmp/stub-bin/systemctl"

  if ! env -i \
    PATH="$tmp/stub-bin:/usr/bin:/bin" \
    HOME="$tmp" \
    PATCHBAY_TEST_SYSTEMCTL_LOG="$tmp/systemctl.log" \
    bash "$ROOT_DIR/scripts/install.sh" \
    >"$tmp/legacy-systemd.out" 2>"$tmp/legacy-systemd.err"; then
    cat "$tmp/legacy-systemd.out" >&2 || true
    cat "$tmp/legacy-systemd.err" >&2 || true
    return 1
  fi

  [ ! -e "$legacy_home" ] || return 1
  [ -f "$tmp/.patchbay/server/.patchbay-systemd.compose.yml" ] || return 1
  [ -f "$unit_dir/patchbay-selfhost.service" ] || return 1
  grep -Fq "WorkingDirectory=$tmp/.patchbay/server" "$unit_dir/patchbay-selfhost.service" || return 1
  grep -Fq 'Description=Patchbay self-hosted Rust services' "$unit_dir/patchbay-selfhost.service" || return 1
  grep -Fq -- '--user disable --now cordy-selfhost.service' "$tmp/systemctl.log" || return 1 # legacy-brand-compat
  grep -Fq -- '--user enable --now patchbay-selfhost.service' "$tmp/systemctl.log" || return 1
}

test_systemd_preflight_fails_before_server_mutation() {
  if [[ "$(uname -s)" != "Linux" ]]; then
    echo "skipping Linux-only systemd preflight test on $(uname -s)"
    return 0
  fi

  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  _setup_server_sandbox "$tmp"
  cp "$tmp/server/.env.example" "$tmp/server/.env"
  cat >"$tmp/stub-bin/systemctl" <<'STUB'
#!/usr/bin/env bash
exit 1
STUB
  printf '#!/usr/bin/env bash\nexit 0\n' >"$tmp/stub-bin/loginctl"
  chmod +x "$tmp/stub-bin/systemctl" "$tmp/stub-bin/loginctl"

  if env -i \
    PATH="$tmp/stub-bin:/usr/bin:/bin" \
    HOME="$tmp" \
    PATCHBAY_INSTALL_DIR="$tmp/server" \
    PATCHBAY_SELFHOST_REF="v0.3.2" \
    bash "$ROOT_DIR/scripts/install.sh" --with-server --systemd \
    >"$tmp/preflight.out" 2>"$tmp/preflight.err"; then
    echo "systemd installation unexpectedly passed a failed preflight" >&2
    return 1
  fi
  if grep -q '^PATCHBAY_IMAGE_TAG=' "$tmp/server/.env"; then
    echo "systemd preflight ran after mutating the server environment" >&2
    cat "$tmp/server/.env" >&2 || true
    return 1
  fi
  grep -q "No systemd user manager is available" "$tmp/preflight.err" || return 1
}

test_with_server_systemd_owns_compose_lifecycle() {
  if [[ "$(uname -s)" != "Linux" ]]; then
    echo "skipping Linux-only systemd lifecycle test on $(uname -s)"
    return 0
  fi

  local tmp unit
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  _setup_server_sandbox "$tmp"
  cp "$tmp/server/.env.example" "$tmp/server/.env"
  : >"$tmp/systemctl.log"
  : >"$tmp/loginctl.log"

  cat >"$tmp/stub-bin/systemctl" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$PATCHBAY_TEST_SYSTEMCTL_LOG"
if [ "${PATCHBAY_TEST_SYSTEMCTL_FAIL_DISABLE:-}" = "1" ] && [[ "$*" == *"disable --now"* ]]; then
  exit 1
fi
exit 0
STUB
  cat >"$tmp/stub-bin/loginctl" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$PATCHBAY_TEST_LOGINCTL_LOG"
exit 0
STUB
  chmod +x "$tmp/stub-bin/systemctl" "$tmp/stub-bin/loginctl"

  if ! (
    cd "$tmp"
    env -i \
      PATH="$tmp/stub-bin:/usr/bin:/bin" \
      HOME="$tmp" \
      USER="patchbay-test" \
      PATCHBAY_INSTALL_DIR="server" \
      PATCHBAY_SELFHOST_REF="v0.3.2" \
      BACKEND_PORT="9000" \
      FRONTEND_PORT="4000" \
      PATCHBAY_TEST_CURL_LOG="$tmp/curl.log" \
      PATCHBAY_TEST_SYSTEMCTL_LOG="$tmp/systemctl.log" \
      PATCHBAY_TEST_LOGINCTL_LOG="$tmp/loginctl.log" \
      bash "$ROOT_DIR/scripts/install.sh" --with-server --systemd \
      >"$tmp/systemd-install.out" 2>"$tmp/systemd-install.err"
  ); then
    cat "$tmp/systemd-install.out" >&2 || true
    cat "$tmp/systemd-install.err" >&2 || true
    return 1
  fi

  unit="$tmp/.config/systemd/user/patchbay-selfhost.service"
  [ -f "$unit" ] || { echo "expected generated systemd user unit" >&2; return 1; }
  grep -Fq "WorkingDirectory=\"$tmp/server\"" "$unit" || return 1
  grep -Fq "ExecStart=\"$tmp/stub-bin/docker\" compose -f \"$tmp/server/.patchbay-systemd.compose.yml\" up -d --remove-orphans" "$unit" || return 1
  grep -Fq '127.0.0.1:9000:8080' "$tmp/server/.patchbay-systemd.compose.yml" || return 1
  grep -Fq '127.0.0.1:4000:3000' "$tmp/server/.patchbay-systemd.compose.yml" || return 1
  grep -Fq -- '--user enable --now patchbay-selfhost.service' "$tmp/systemctl.log" || return 1
  grep -Fq 'enable-linger patchbay-test' "$tmp/loginctl.log" || return 1

  if ! env -i \
    PATH="$tmp/stub-bin:/usr/bin:/bin" \
    HOME="$tmp" \
    PATCHBAY_INSTALL_DIR="$tmp/server" \
    PATCHBAY_TEST_SYSTEMCTL_LOG="$tmp/systemctl.log" \
    bash "$ROOT_DIR/scripts/install.sh" --stop \
    >"$tmp/systemd-stop.out" 2>"$tmp/systemd-stop.err"; then
    cat "$tmp/systemd-stop.out" >&2 || true
    cat "$tmp/systemd-stop.err" >&2 || true
    return 1
  fi
  grep -Fq -- '--user disable --now patchbay-selfhost.service' "$tmp/systemctl.log" || return 1

  if env -i \
    PATH="$tmp/stub-bin:/usr/bin:/bin" \
    HOME="$tmp" \
    PATCHBAY_INSTALL_DIR="$tmp/server" \
    PATCHBAY_TEST_SYSTEMCTL_LOG="$tmp/systemctl.log" \
    PATCHBAY_TEST_SYSTEMCTL_FAIL_DISABLE=1 \
    bash "$ROOT_DIR/scripts/install.sh" --stop \
    >"$tmp/systemd-stop-failure.out" 2>"$tmp/systemd-stop-failure.err"; then
    echo "stop unexpectedly succeeded when systemd could not disable the unit" >&2
    return 1
  fi
  grep -q "Could not stop and disable patchbay-selfhost.service" "$tmp/systemd-stop-failure.err" || return 1
}

test_container_entrypoint_forwards_migration_signal() {
  local tmp entrypoint_pid status=0
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  cp "$ROOT_DIR/docker/entrypoint.sh" "$tmp/entrypoint.sh"
  cat >"$tmp/migrate" <<'STUB'
#!/bin/sh
trap 'touch migration-terminated; exit 143' TERM
touch migration-ready
while :; do sleep 1; done
STUB
  cat >"$tmp/server" <<'STUB'
#!/bin/sh
touch server-started
STUB
  chmod +x "$tmp/entrypoint.sh" "$tmp/migrate" "$tmp/server"

  (cd "$tmp" && exec ./entrypoint.sh >entrypoint.out 2>entrypoint.err) &
  entrypoint_pid=$!
  for _ in $(seq 1 50); do
    [ -f "$tmp/migration-ready" ] && break
    sleep 0.1
  done
  [ -f "$tmp/migration-ready" ] || { echo "migration child did not start" >&2; return 1; }

  kill -TERM "$entrypoint_pid"
  wait "$entrypoint_pid" || status=$?
  [ "$status" -ne 0 ] || { echo "signalled entrypoint unexpectedly succeeded" >&2; return 1; }
  [ -f "$tmp/migration-terminated" ] || { echo "entrypoint did not forward TERM to migration" >&2; return 1; }
  [ ! -f "$tmp/server-started" ] || { echo "entrypoint started server after interrupted migration" >&2; return 1; }
}

test_release_binary_install
test_remote_ssh_install_prints_token_login_hint
test_local_install_does_not_print_token_login_hint
test_with_server_uses_compose_published_ports
test_with_server_fails_when_compose_port_is_unavailable
test_with_server_pins_selected_release_images
test_with_server_preserves_existing_image_pin_without_explicit_ref
test_with_server_preserves_legacy_image_pin_before_brand_migration
test_with_server_resolves_latest_pin_to_matching_release_assets
test_with_server_migrates_legacy_branding_without_overwriting_custom_repositories
test_default_home_migrates_legacy_systemd_unit
test_systemd_preflight_fails_before_server_mutation
test_with_server_systemd_owns_compose_lifecycle
test_container_entrypoint_forwards_migration_signal
echo "install.sh tests passed"
