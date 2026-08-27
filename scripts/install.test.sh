#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Build a self-contained sandbox with stub `curl` and a tarball that the
# release-binary fallback path will download. Each test supplies its own
# `brew` stub to model a specific Homebrew failure mode.
_setup_sandbox() {
  local tmp="$1"
  local stub_bin="$tmp/stub-bin"
  local install_bin="$tmp/install-bin"
  local payload_dir="$tmp/payload"
  mkdir -p "$stub_bin" "$install_bin" "$payload_dir"

  cat >"$payload_dir/cordy" <<'STUB'
#!/usr/bin/env bash
echo "cordy v0.3.2 (commit: test)"
STUB
  chmod +x "$payload_dir/cordy"
  tar -czf "$tmp/cordy.tar.gz" -C "$payload_dir" cordy

  cat >"$stub_bin/curl" <<'STUB'
#!/usr/bin/env bash
if [[ "$*" == *"-sI"* ]]; then
  printf 'HTTP/2 302\r\nlocation: https://github.com/cordy-ai/cordy/releases/tag/v0.3.2\r\n'
  exit 0
fi

out=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -o)
      out="$2"
      shift 2
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
cp "$CORDY_TEST_ARCHIVE" "$out"
STUB
  chmod +x "$stub_bin/curl"
}

_run_installer() {
  local tmp="$1"
  local out="$tmp/install.out"
  local err="$tmp/install.err"
  if ! PATH="$tmp/stub-bin:$tmp/install-bin:/usr/bin:/bin" \
    CORDY_BIN_DIR="$tmp/install-bin" \
    CORDY_TEST_ARCHIVE="$tmp/cordy.tar.gz" \
    bash "$ROOT_DIR/scripts/install.sh" >"$out" 2>"$err"; then
    echo "install.sh exited non-zero" >&2
    cat "$out" >&2 || true
    cat "$err" >&2 || true
    return 1
  fi

  if [[ ! -x "$tmp/install-bin/cordy" ]]; then
    echo "expected fallback binary at $tmp/install-bin/cordy" >&2
    cat "$out" >&2 || true
    cat "$err" >&2 || true
    return 1
  fi

  if ! grep -q "Homebrew output (last 80 lines):" "$err"; then
    echo "expected diagnostic tail in stderr" >&2
    cat "$err" >&2 || true
    return 1
  fi
}

test_brew_install_failure_falls_back_to_release_binary() {
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

  _run_installer "$tmp"
}

test_brew_tap_failure_falls_back_to_release_binary() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  _setup_sandbox "$tmp"
  cat >"$tmp/stub-bin/brew" <<'STUB'
#!/usr/bin/env bash
case "${1:-}" in
  tap)
    echo "simulated brew tap failure" >&2
    exit 17
    ;;
  *)
    echo "brew $* should not be reached after tap failure" >&2
    exit 99
    ;;
esac
STUB
  chmod +x "$tmp/stub-bin/brew"

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
  if ! grep -q "https://cordy.ai/settings?tab=tokens" "$tmp/install.out"; then
    echo "expected direct API Tokens settings URL in installer output" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
  if ! grep -q "Settings > API Tokens" "$tmp/install.out"; then
    echo "expected API Tokens tab name in installer output" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
  if ! grep -q "cordy login --token <YOUR_TOKEN>" "$tmp/install.out"; then
    echo "expected token login command in installer output" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
  if grep -q "cordy config set server_url" "$tmp/install.out"; then
    echo "did not expect default cloud server config command in installer output" >&2
    cat "$tmp/install.out" >&2 || true
    return 1
  fi
  if grep -q "cordy config set app_url" "$tmp/install.out"; then
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
  if grep -q "cordy login --token <YOUR_TOKEN>" "$tmp/install.out"; then
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
POSTGRES_PASSWORD=cordy
DATABASE_URL=postgres://cordy:cordy@localhost:5432/cordy?sslmode=disable
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
        pull | up | port | version | ps | logs | down) subcommand="$arg"; break ;;
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
  printf '#!/usr/bin/env bash\nexit 0\n' >"$stub_bin/git"
  chmod +x "$stub_bin/git"

  # brew: pretend the CLI installs cleanly so the run reaches the summary.
  printf '#!/usr/bin/env bash\nexit 0\n' >"$stub_bin/brew"
  chmod +x "$stub_bin/brew"

  printf '#!/usr/bin/env bash\necho "cordy v0.3.2 (commit: test)"\n' >"$stub_bin/cordy"
  chmod +x "$stub_bin/cordy"

  # curl records every probed URL so the health-check port can be asserted.
  cat >"$stub_bin/curl" <<'STUB'
#!/usr/bin/env bash
set -uo pipefail
for arg in "$@"; do
  case "$arg" in
    http*) printf '%s\n' "$arg" >>"$CORDY_TEST_CURL_LOG" ;;
  esac
done
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
  if ! env -i \
    PATH="$tmp/stub-bin:/usr/bin:/bin" \
    HOME="$tmp" \
    CORDY_INSTALL_DIR="$tmp/server" \
    CORDY_SELFHOST_REF="main" \
    CORDY_TEST_CURL_LOG="$tmp/curl.log" \
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

  _run_with_server "$tmp" CORDY_SELFHOST_REF=v0.3.2 CORDY_IMAGE_TAG=ambient || return 1

  if [ "$(grep '^CORDY_IMAGE_TAG=' "$tmp/server/.env")" != "CORDY_IMAGE_TAG=v0.3.2" ]; then
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

test_with_server_systemd_owns_compose_lifecycle() {
  local tmp unit
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  _setup_server_sandbox "$tmp"
  cp "$tmp/server/.env.example" "$tmp/server/.env"
  : >"$tmp/systemctl.log"
  : >"$tmp/loginctl.log"

  cat >"$tmp/stub-bin/systemctl" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$CORDY_TEST_SYSTEMCTL_LOG"
exit 0
STUB
  cat >"$tmp/stub-bin/loginctl" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$CORDY_TEST_LOGINCTL_LOG"
exit 0
STUB
  chmod +x "$tmp/stub-bin/systemctl" "$tmp/stub-bin/loginctl"

  if ! env -i \
    PATH="$tmp/stub-bin:/usr/bin:/bin" \
    HOME="$tmp" \
    USER="cordy-test" \
    CORDY_INSTALL_DIR="$tmp/server" \
    CORDY_SELFHOST_REF="v0.3.2" \
    CORDY_TEST_CURL_LOG="$tmp/curl.log" \
    CORDY_TEST_SYSTEMCTL_LOG="$tmp/systemctl.log" \
    CORDY_TEST_LOGINCTL_LOG="$tmp/loginctl.log" \
    bash "$ROOT_DIR/scripts/install.sh" --with-server --systemd \
    >"$tmp/systemd-install.out" 2>"$tmp/systemd-install.err"; then
    cat "$tmp/systemd-install.out" >&2 || true
    cat "$tmp/systemd-install.err" >&2 || true
    return 1
  fi

  unit="$tmp/.config/systemd/user/cordy-selfhost.service"
  [ -f "$unit" ] || { echo "expected generated systemd user unit" >&2; return 1; }
  grep -Fq "WorkingDirectory=\"$tmp/server\"" "$unit" || return 1
  grep -Fq "ExecStart=\"$tmp/stub-bin/docker\" compose -f docker-compose.selfhost.yml up -d --remove-orphans" "$unit" || return 1
  grep -Fq -- '--user enable --now cordy-selfhost.service' "$tmp/systemctl.log" || return 1
  grep -Fq 'enable-linger cordy-test' "$tmp/loginctl.log" || return 1

  if ! env -i \
    PATH="$tmp/stub-bin:/usr/bin:/bin" \
    HOME="$tmp" \
    CORDY_INSTALL_DIR="$tmp/server" \
    CORDY_TEST_SYSTEMCTL_LOG="$tmp/systemctl.log" \
    bash "$ROOT_DIR/scripts/install.sh" --stop \
    >"$tmp/systemd-stop.out" 2>"$tmp/systemd-stop.err"; then
    cat "$tmp/systemd-stop.out" >&2 || true
    cat "$tmp/systemd-stop.err" >&2 || true
    return 1
  fi
  grep -Fq -- '--user disable --now cordy-selfhost.service' "$tmp/systemctl.log" || return 1
}

test_brew_install_failure_falls_back_to_release_binary
test_brew_tap_failure_falls_back_to_release_binary
test_remote_ssh_install_prints_token_login_hint
test_local_install_does_not_print_token_login_hint
test_with_server_uses_compose_published_ports
test_with_server_fails_when_compose_port_is_unavailable
test_with_server_pins_selected_release_images
test_with_server_systemd_owns_compose_lifecycle
echo "install.sh tests passed"
