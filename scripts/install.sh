#!/usr/bin/env bash
# Patchbay installer — installs the CLI and optionally provisions a self-host server.
#
# Install / upgrade CLI only:
#   curl -fsSL https://raw.githubusercontent.com/patchbay-ai/patchbay/main/scripts/install.sh | bash
#
# Install CLI + provision self-host server:
#   curl -fsSL https://raw.githubusercontent.com/patchbay-ai/patchbay/main/scripts/install.sh | bash -s -- --with-server
#
# After installation, run `patchbay setup` to configure your environment.
#
set -euo pipefail

if [ -z "${PATCHBAY_INSTALL_DIR+x}" ] && [ -n "${CORDY_INSTALL_DIR+x}" ]; then # legacy-brand-compat
  export PATCHBAY_INSTALL_DIR="$CORDY_INSTALL_DIR" # legacy-brand-compat
fi

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
REPO_URL="https://github.com/patchbay-ai/patchbay.git"
REPO_WEB_URL="https://github.com/patchbay-ai/patchbay"  # without .git, for GitHub web APIs
INSTALL_DIR="${PATCHBAY_INSTALL_DIR:-$HOME/.patchbay/server}"
LEGACY_PATCHBAY_HOME="$HOME/.cordy" # legacy-brand-compat
BREW_PACKAGE="patchbay-ai/tap/patchbay"

# Host ports Compose reported after `up -d`; set by setup_server and reused by
# the summary so the health check and the printed URLs cannot diverge.
SELFHOST_BACKEND_PORT=""
SELFHOST_FRONTEND_PORT=""
INSTALL_SYSTEMD=false
SELFHOST_ENV_EXISTED=false

# Colors (disabled when not a terminal)
if [ -t 1 ] || [ -t 2 ]; then
  BOLD='\033[1m'
  GREEN='\033[0;32m'
  YELLOW='\033[0;33m'
  RED='\033[0;31m'
  CYAN='\033[0;36m'
  RESET='\033[0m'
else
  BOLD='' GREEN='' YELLOW='' RED='' CYAN='' RESET=''
fi

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
info()  { printf "${BOLD}${CYAN}==> %s${RESET}\n" "$*"; }
ok()    { printf "${BOLD}${GREEN}✓ %s${RESET}\n" "$*"; }
warn()  { printf "${BOLD}${YELLOW}⚠ %s${RESET}\n" "$*" >&2; }
fail()  { printf "${BOLD}${RED}✗ %s${RESET}\n" "$*" >&2; exit 1; }

command_exists() { command -v "$1" >/dev/null 2>&1; }

normalize_install_dir() {
  case "$INSTALL_DIR" in
    /*) ;;
    *) INSTALL_DIR="$PWD/$INSTALL_DIR" ;;
  esac
}

migrate_legacy_patchbay_home() {
  local patchbay_home="$HOME/.patchbay"
  if [ -z "${PATCHBAY_INSTALL_DIR:-}" ] && [ ! -e "$patchbay_home" ] && [ -d "$LEGACY_PATCHBAY_HOME" ]; then
    mv -- "$LEGACY_PATCHBAY_HOME" "$patchbay_home"
    ok "Migrated the existing Patchbay home directory to $patchbay_home"
  fi
}

sha256_file() {
  local path="$1"
  if command_exists sha256sum; then
    sha256sum "$path" | awk '{print tolower($1)}'
  elif command_exists shasum; then
    shasum -a 256 "$path" | awk '{print tolower($1)}'
  else
    fail "Neither sha256sum nor shasum is available; refusing to install an unverified CLI."
  fi
}

running_in_ssh_session() {
  [ -n "${SSH_CONNECTION:-}" ] || [ -n "${SSH_CLIENT:-}" ] || [ -n "${SSH_TTY:-}" ]
}

print_remote_server_token_hint() {
  if ! running_in_ssh_session; then
    return
  fi

  printf "  ${BOLD}Looks like a remote/SSH session.${RESET} Browser login may not be able to call back to this machine's localhost.\n"
  printf "  Token login is usually simpler here:\n"
  printf "     1. On your local computer, open ${CYAN}https://patchbay.ai/settings?tab=tokens${RESET}\n"
  printf "        and create a token under ${BOLD}Settings > API Tokens${RESET}.\n"
  printf "     2. On this server, run:\n"
  printf "        ${CYAN}patchbay login --token <YOUR_TOKEN>${RESET}\n"
  printf "        ${CYAN}patchbay daemon start${RESET}\n"
  printf "\n"
}

# Host port Docker Compose actually published for a service.
#
# This is the only authority. Compose's interpolation gives the calling process
# environment precedence over .env, so an ambient PORT / BACKEND_PORT / API_PORT
# / SERVER_PORT / FRONTEND_PORT moves the published port without touching the
# file. Re-deriving the port from .env alone made the installer probe and print
# a port the stack was never published on (#6145). Must be called from the
# installation directory, after `up -d`.
compose_published_port() {
  local service=$1 container_port=$2 published

  published="$(docker compose -f docker-compose.selfhost.yml port "$service" "$container_port" 2>/dev/null | tail -n 1)"
  published="${published##*:}"
  published="${published%$'\r'}"

  case "$published" in
    "" | *[!0-9]*) return 1 ;;
  esac

  printf "%s" "$published"
}

detect_os() {
  case "$(uname -s)" in
    Darwin) OS="darwin" ;;
    Linux)  OS="linux" ;;
    MINGW*|MSYS*|CYGWIN*)
            fail "This script does not support Windows. Use the PowerShell installer instead:
  irm https://raw.githubusercontent.com/patchbay-ai/patchbay/main/scripts/install.ps1 | iex" ;;
    *)      fail "Unsupported operating system: $(uname -s). Patchbay supports macOS, Linux, and Windows." ;;
  esac

  ARCH="$(uname -m)"
  case "$ARCH" in
    x86_64)  ARCH="amd64" ;;
    aarch64) ARCH="arm64" ;;
    arm64)   ARCH="arm64" ;;
    *)       fail "Unsupported architecture: $ARCH" ;;
  esac
}

# ---------------------------------------------------------------------------
# CLI Installation
# ---------------------------------------------------------------------------
_dump_brew_log() {
  local log="$1"
  if [ -s "$log" ]; then
    warn "Homebrew output (last 80 lines):"
    tail -n 80 "$log" | sed 's/^/  /' >&2
  fi
}

install_cli_brew() {
  info "Installing Patchbay CLI via Homebrew..."
  local brew_log
  brew_log=$(mktemp)
  if ! brew tap patchbay-ai/tap >"$brew_log" 2>&1; then
    warn "Failed to add Homebrew tap. Falling back to GitHub Releases binary install."
    _dump_brew_log "$brew_log"
    rm -f "$brew_log"
    return 1
  fi
  # brew install exits non-zero if already installed on older Homebrew versions
  if ! brew install "$BREW_PACKAGE" >"$brew_log" 2>&1; then
    if brew list "$BREW_PACKAGE" >/dev/null 2>&1; then
      rm -f "$brew_log"
      ok "Patchbay CLI already installed via Homebrew"
    else
      warn "Failed to install patchbay via Homebrew. Falling back to GitHub Releases binary install."
      _dump_brew_log "$brew_log"
      rm -f "$brew_log"
      return 1
    fi
  else
    rm -f "$brew_log"
    ok "Patchbay CLI installed via Homebrew"
  fi
}

install_cli_binary() {
  info "Installing Patchbay CLI from GitHub Releases..."

  # Get latest release tag
  local latest
  latest=$(curl -sI "$REPO_WEB_URL/releases/latest" 2>/dev/null | grep -i '^location:' | sed 's/.*tag\///' | tr -d '\r\n' || true)
  if [ -z "$latest" ]; then
    fail "Could not determine latest release. Check your network connection."
  fi

  local version="${latest#v}"
  local url="https://github.com/patchbay-ai/patchbay/releases/download/${latest}/patchbay-cli-${version}-${OS}-${ARCH}.tar.gz"
  local tmp_dir
  tmp_dir=$(mktemp -d)

  info "Downloading $url ..."
  if ! curl -fsSL "$url" -o "$tmp_dir/patchbay.tar.gz"; then
    rm -rf "$tmp_dir"
    fail "Failed to download CLI binary."
  fi

  local checksum_file="$tmp_dir/checksums.txt"
  local asset_name="patchbay-cli-${version}-${OS}-${ARCH}.tar.gz"
  if ! curl -fsSL "https://github.com/patchbay-ai/patchbay/releases/download/${latest}/checksums.txt" -o "$checksum_file"; then
    rm -rf "$tmp_dir"
    fail "Failed to download the CLI checksum manifest; refusing to install an unverified binary."
  fi
  local expected_checksum
  if ! expected_checksum=$(awk -v asset="$asset_name" '
    $2 == asset || $2 == "*" asset {
      count++
      checksum=tolower($1)
    }
    END {
      if (count != 1 || length(checksum) != 64 || checksum !~ /^[[:xdigit:]]+$/) exit 1
      print checksum
    }
  ' "$checksum_file"); then
    rm -rf "$tmp_dir"
    fail "CLI checksum manifest has no unique valid entry for ${asset_name}."
  fi
  local actual_checksum
  actual_checksum=$(sha256_file "$tmp_dir/patchbay.tar.gz")
  if [ "$actual_checksum" != "$expected_checksum" ]; then
    rm -rf "$tmp_dir"
    fail "CLI checksum verification failed for ${asset_name}."
  fi

  tar -xzf "$tmp_dir/patchbay.tar.gz" -C "$tmp_dir" patchbay

  # Try /usr/local/bin first, fall back to ~/.local/bin. Tests and scripted
  # installs can override the first choice with PATCHBAY_BIN_DIR.
  local bin_dir="${PATCHBAY_BIN_DIR:-/usr/local/bin}"
  if [ -w "$bin_dir" ]; then
    mv "$tmp_dir/patchbay" "$bin_dir/patchbay"
  elif command_exists sudo; then
    sudo mv "$tmp_dir/patchbay" "$bin_dir/patchbay"
  else
    bin_dir="$HOME/.local/bin"
    mkdir -p "$bin_dir"
    mv "$tmp_dir/patchbay" "$bin_dir/patchbay"
    chmod +x "$bin_dir/patchbay"
    # Add to PATH if not already there
    if ! echo "$PATH" | tr ':' '\n' | grep -q "^$bin_dir$"; then
      export PATH="$bin_dir:$PATH"
      add_to_path "$bin_dir"
    fi
  fi

  rm -rf "$tmp_dir"
  ok "Patchbay CLI installed to $bin_dir/patchbay"
}

add_to_path() {
  local dir="$1"
  local line="export PATH=\"$dir:\$PATH\""
  for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
    if [ -f "$rc" ] && ! grep -qF "$dir" "$rc"; then
      printf '\n# Added by Patchbay installer\n%s\n' "$line" >> "$rc"
    fi
  done
}

get_latest_version() {
  # grep exits 1 when no match; use `|| true` to avoid triggering pipefail
  curl -sI "$REPO_WEB_URL/releases/latest" 2>/dev/null | grep -i '^location:' | sed 's/.*tag\///' | tr -d '\r\n' || true
}

existing_selfhost_image_pin() {
  [ "$SELFHOST_ENV_EXISTED" = true ] || return 1
  [ -f "$INSTALL_DIR/.env" ] || return 1

  local image_tag
  image_tag="$(sed -n 's/^PATCHBAY_IMAGE_TAG=//p' "$INSTALL_DIR/.env" | tail -n 1)"
  [ -n "$image_tag" ] || return 1
  printf '%s' "$image_tag"
}

validate_selfhost_image_tag() {
  local image_tag="$1"
  case "$image_tag" in
    "" | [.-]* | *[!A-Za-z0-9_.-]*)
      fail "Self-host image tag '$image_tag' is invalid. Use a release tag such as v0.4.10 or main."
      ;;
  esac
  if [ "${#image_tag}" -gt 128 ]; then
    fail "Self-host image tag '$image_tag' is too long."
  fi
}

get_selfhost_ref() {
  if [ -n "${PATCHBAY_SELFHOST_REF:-}" ]; then
    printf '%s' "$PATCHBAY_SELFHOST_REF"
    return
  fi

  # Keep deployment assets and images on one version boundary. A rerun of an
  # existing install must check out the ref represented by its durable image
  # pin instead of combining that old image with the newest Compose/config.
  local existing_pin latest
  if existing_pin="$(existing_selfhost_image_pin)"; then
    validate_selfhost_image_tag "$existing_pin"
    case "$existing_pin" in
      latest)
        latest="$(get_latest_version)"
        if [ -z "$latest" ]; then
          fail "Existing self-host image pin 'latest' cannot be mapped to a published release. Set PATCHBAY_SELFHOST_REF explicitly."
        fi
        validate_selfhost_image_tag "$latest"
        printf '%s' "$latest"
        ;;
      sha-*)
        local commit_ref="${existing_pin#sha-}"
        if ! printf '%s' "$commit_ref" | grep -Eq '^[0-9A-Fa-f]{40}$'; then
          fail "Existing self-host image pin '$existing_pin' cannot be mapped reliably to a full Git commit. Set PATCHBAY_SELFHOST_REF explicitly."
        fi
        printf '%s' "$commit_ref"
        ;;
      *)
        printf '%s' "$existing_pin"
        ;;
    esac
    return
  fi

  latest=$(get_latest_version)
  if [ -n "$latest" ]; then
    printf '%s' "$latest"
    return
  fi

  printf '%s' "main"
}

checkout_server_ref() {
  local ref="$1"
  git fetch origin "$ref" --depth 1 || fail "Could not fetch self-host ref '$ref'."
  git checkout --force --detach FETCH_HEAD || fail "Could not check out self-host ref '$ref'."
}

migrate_legacy_selfhost_branding() {
  local migrated=false
  local canonical_backend="ghcr.io/patchbay-ai/patchbay-backend"
  local canonical_web="ghcr.io/patchbay-ai/patchbay-web"
  local legacy_backend legacy_web
  local legacy_backends=(
    "ghcr.io/cordy-ai/cordy-backend" # legacy-brand-compat
    "ghcr.io/alexj11324/cordy-backend" # legacy-brand-compat
  )
  local legacy_webs=(
    "ghcr.io/cordy-ai/cordy-web" # legacy-brand-compat
    "ghcr.io/alexj11324/cordy-web" # legacy-brand-compat
  )

  if grep -q '^CORDY_' .env; then # legacy-brand-compat
    if [ "$(uname -s)" = "Darwin" ]; then
      sed -i '' 's/^CORDY_/PATCHBAY_/' .env # legacy-brand-compat
    else
      sed -i 's/^CORDY_/PATCHBAY_/' .env # legacy-brand-compat
    fi
    migrated=true
  fi

  for legacy_backend in "${legacy_backends[@]}"; do
    if grep -Fxq "PATCHBAY_BACKEND_IMAGE=$legacy_backend" .env; then
      if [ "$(uname -s)" = "Darwin" ]; then
        sed -i '' "s#^PATCHBAY_BACKEND_IMAGE=$legacy_backend\$#PATCHBAY_BACKEND_IMAGE=$canonical_backend#" .env
      else
        sed -i "s#^PATCHBAY_BACKEND_IMAGE=$legacy_backend\$#PATCHBAY_BACKEND_IMAGE=$canonical_backend#" .env
      fi
      migrated=true
    fi
  done
  for legacy_web in "${legacy_webs[@]}"; do
    if grep -Fxq "PATCHBAY_WEB_IMAGE=$legacy_web" .env; then
      if [ "$(uname -s)" = "Darwin" ]; then
        sed -i '' "s#^PATCHBAY_WEB_IMAGE=$legacy_web\$#PATCHBAY_WEB_IMAGE=$canonical_web#" .env
      else
        sed -i "s#^PATCHBAY_WEB_IMAGE=$legacy_web\$#PATCHBAY_WEB_IMAGE=$canonical_web#" .env
      fi
      migrated=true
    fi
  done

  if [ "$migrated" = true ]; then
    ok "Migrated the existing self-host configuration to Patchbay identifiers"
  fi
}

pin_selfhost_image_tag() {
  local ref="$1" image_tag preserve_existing=false

  # A durable pin in an existing installation is an operator choice. Only an
  # explicit PATCHBAY_SELFHOST_REF is allowed to replace it; otherwise rerunning
  # the installer could unexpectedly start a newer image and its migrations.
  if [ "$SELFHOST_ENV_EXISTED" = true ] && [ -z "${PATCHBAY_SELFHOST_REF:-}" ] && grep -q '^PATCHBAY_IMAGE_TAG=.' .env; then
    image_tag="$(sed -n 's/^PATCHBAY_IMAGE_TAG=//p' .env | tail -n 1)"
    if [ "$image_tag" = "latest" ]; then
      # `latest` is a moving channel rather than a durable version boundary.
      # Resolve it to the same stable release ref selected for the deployment
      # assets, then persist that exact version below.
      image_tag="$ref"
    else
      preserve_existing=true
    fi
  elif [ "$ref" = "main" ]; then
    image_tag="latest"
  else
    image_tag="$ref"
  fi

  validate_selfhost_image_tag "$image_tag"

  if [ "$preserve_existing" = true ]; then
    export PATCHBAY_IMAGE_TAG="$image_tag"
    ok "Preserved existing backend and web image pin $image_tag"
    return
  elif grep -q '^PATCHBAY_IMAGE_TAG=' .env; then
    if [ "$(uname -s)" = "Darwin" ]; then
      sed -i '' "s/^PATCHBAY_IMAGE_TAG=.*/PATCHBAY_IMAGE_TAG=$image_tag/" .env
    else
      sed -i "s/^PATCHBAY_IMAGE_TAG=.*/PATCHBAY_IMAGE_TAG=$image_tag/" .env
    fi
  else
    printf '\nPATCHBAY_IMAGE_TAG=%s\n' "$image_tag" >>.env
  fi

  # Compose gives the calling environment precedence over .env. Export the
  # selected ref so an ambient PATCHBAY_IMAGE_TAG cannot silently defeat rollback.
  export PATCHBAY_IMAGE_TAG="$image_tag"
  ok "Pinned backend and web images to $image_tag"
}

systemd_quote() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  value="${value//%/%%}"
  printf '"%s"' "$value"
}

preflight_selfhost_systemd() {
  [ "$OS" = "linux" ] || fail "--systemd is supported only on Linux."
  command_exists systemctl || fail "--systemd requires systemctl."
  command_exists loginctl || fail "--systemd requires loginctl to enable user lingering."
  systemctl --user show-environment >/dev/null 2>&1 ||
    fail "No systemd user manager is available. Enable systemd for this login and retry."
}

persist_systemd_compose_configuration() {
  local configuration_path="$INSTALL_DIR/.patchbay-systemd.compose.yml" temporary_path
  temporary_path="$(mktemp "$INSTALL_DIR/.patchbay-systemd.compose.yml.XXXXXX")" ||
    fail "Could not create the resolved systemd Compose file."

  if ! docker compose -f docker-compose.selfhost.yml config >"$temporary_path"; then
    rm -f "$temporary_path"
    fail "Could not resolve the current Docker Compose configuration for systemd."
  fi
  chmod 0600 "$temporary_path"
  mv "$temporary_path" "$configuration_path"
}

install_selfhost_systemd() {
  preflight_selfhost_systemd

  local account docker_path unit_dir unit_path install_dir_q docker_q configuration_q
  account="${USER:-$(id -un)}"
  docker_path="$(command -v docker)"
  unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
  unit_path="$unit_dir/patchbay-selfhost.service"
  install_dir_q="$(systemd_quote "$INSTALL_DIR")"
  docker_q="$(systemd_quote "$docker_path")"
  configuration_q="$(systemd_quote "$INSTALL_DIR/.patchbay-systemd.compose.yml")"

  persist_systemd_compose_configuration

  loginctl enable-linger "$account" ||
    fail "Could not enable systemd lingering for '$account'. Run 'sudo loginctl enable-linger $account' and retry."

  mkdir -p "$unit_dir"
  {
    printf '%s\n' '[Unit]'
    printf '%s\n' 'Description=Patchbay self-hosted Rust services'
    printf '%s\n' 'Wants=network-online.target'
    printf '%s\n' 'After=network-online.target'
    printf '\n%s\n' '[Service]'
    printf '%s\n' 'Type=oneshot'
    printf '%s\n' 'RemainAfterExit=yes'
    printf 'WorkingDirectory=%s\n' "$install_dir_q"
    printf 'ExecStartPre=%s compose -f %s config --quiet\n' "$docker_q" "$configuration_q"
    printf 'ExecStart=%s compose -f %s up -d --remove-orphans\n' "$docker_q" "$configuration_q"
    printf 'ExecStop=%s compose -f %s down\n' "$docker_q" "$configuration_q"
    printf '%s\n' 'Restart=on-failure'
    printf '%s\n' 'RestartSec=10s'
    printf '%s\n' 'TimeoutStartSec=5min'
    printf '%s\n' 'TimeoutStopSec=2min'
    printf '\n%s\n' '[Install]'
    printf '%s\n' 'WantedBy=default.target'
  } >"$unit_path"
  chmod 0644 "$unit_path"

  systemctl --user daemon-reload || fail "Could not reload the systemd user manager."
  systemctl --user enable --now patchbay-selfhost.service ||
    fail "Could not enable and start patchbay-selfhost.service."
  ok "Enabled patchbay-selfhost.service for boot and login-independent operation"
}

pull_official_selfhost_images() {
  if docker compose -f docker-compose.selfhost.yml pull; then
    return
  fi

  echo ""
  warn "Official images for the selected self-host channel are not published yet."
  echo "This can happen before the first GHCR release is available."
  echo "From $INSTALL_DIR, build from source instead:"
  echo "  docker compose -f docker-compose.selfhost.yml -f docker-compose.selfhost.build.yml up -d --build"
  exit 1
}

upgrade_cli_brew() {
  info "Upgrading Patchbay CLI via Homebrew..."
  brew update 2>/dev/null || true
  if brew upgrade "$BREW_PACKAGE" 2>/dev/null; then
    ok "Patchbay CLI upgraded via Homebrew"
  else
    # brew upgrade exits non-zero if already up to date
    ok "Patchbay CLI is already the latest version"
  fi
}

install_cli() {
  if command_exists patchbay; then
    local current_ver
    # `patchbay version` outputs "patchbay 0.3.23 (commit: f46b929eb, built: 2026-06-16T10:11:56Z)" — extract just the version
    current_ver=$(patchbay version 2>/dev/null | awk 'NR==1{print $2}' || echo "unknown")

    local latest_ver
    latest_ver=$(get_latest_version)

    # Normalize: strip leading 'v' for comparison
    local current_cmp="${current_ver#v}"
    local latest_cmp="${latest_ver#v}"

    if [ -z "$latest_ver" ] || [ "$current_cmp" = "$latest_cmp" ]; then
      ok "Patchbay CLI is up to date ($current_ver)"
      return 0
    fi

    info "Patchbay CLI $current_ver installed, latest is $latest_ver — upgrading..."
    if command_exists brew && brew list "$BREW_PACKAGE" >/dev/null 2>&1; then
      upgrade_cli_brew
    else
      install_cli_binary
    fi

    local new_ver
    new_ver=$(patchbay version 2>/dev/null | awk 'NR==1{print $2}' || echo "unknown")
    ok "Patchbay CLI upgraded ($current_ver → $new_ver)"
    return 0
  fi

  if command_exists brew; then
    install_cli_brew || install_cli_binary
  else
    install_cli_binary
  fi

  # Verify
  if ! command_exists patchbay; then
    fail "CLI installed but 'patchbay' not found on PATH. You may need to restart your shell."
  fi
}

# ---------------------------------------------------------------------------
# Docker check
# ---------------------------------------------------------------------------
check_docker() {
  if ! command_exists docker; then
    printf "\n"
    fail "Docker is not installed. Patchbay self-hosting requires Docker and Docker Compose.

Install Docker:
  macOS:  https://docs.docker.com/desktop/install/mac-install/
  Linux:  https://docs.docker.com/engine/install/

After installing Docker, re-run this script with --with-server."
  fi

  if ! docker info >/dev/null 2>&1; then
    fail "Docker is installed but not running. Please start Docker and re-run this script."
  fi

  ok "Docker is available"
}

# ---------------------------------------------------------------------------
# Server setup (self-host / --with-server)
# ---------------------------------------------------------------------------
setup_server() {
  info "Setting up Patchbay server..."
  local server_ref
  if [ -d "$INSTALL_DIR/.git" ] && [ -f "$INSTALL_DIR/.env" ]; then
    SELFHOST_ENV_EXISTED=true
  fi
  server_ref=$(get_selfhost_ref)
  info "Using self-host assets from ${server_ref}..."

  if [ -d "$INSTALL_DIR/.git" ]; then
    info "Updating existing installation at $INSTALL_DIR..."
    cd "$INSTALL_DIR"
  else
    info "Cloning Patchbay repository..."
    if ! command_exists git; then
      fail "Git is not installed. Please install git and re-run."
    fi
    # Remove leftover directory from a previously interrupted clone
    if [ -d "$INSTALL_DIR" ]; then
      warn "Removing incomplete installation at $INSTALL_DIR..."
      rm -rf "$INSTALL_DIR"
    fi
    mkdir -p "$(dirname "$INSTALL_DIR")"
    git clone --depth 1 "$REPO_URL" "$INSTALL_DIR"
    cd "$INSTALL_DIR"
  fi

  checkout_server_ref "$server_ref"

  ok "Repository ready at $INSTALL_DIR ($server_ref)"

  # Generate .env if needed
  if [ ! -f .env ]; then
    info "Creating .env with random secrets..."
    cp .env.example .env
    local jwt pgpass
    jwt=$(openssl rand -hex 32)
    pgpass=$(openssl rand -hex 24)
    if [ "$(uname -s)" = "Darwin" ]; then
      sed -i '' "s/^JWT_SECRET=.*/JWT_SECRET=$jwt/" .env
      sed -i '' "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$pgpass/" .env
      sed -i '' -E "s#^(DATABASE_URL=postgres://[^:]+:)[^@]*(@.*)#\1$pgpass\2#" .env
    else
      sed -i "s/^JWT_SECRET=.*/JWT_SECRET=$jwt/" .env
      sed -i "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$pgpass/" .env
      sed -i -E "s#^(DATABASE_URL=postgres://[^:]+:)[^@]*(@.*)#\1$pgpass\2#" .env
    fi
    ok "Generated .env with random JWT_SECRET and POSTGRES_PASSWORD"
  else
    SELFHOST_ENV_EXISTED=true
    ok "Using existing .env"
  fi

  migrate_legacy_selfhost_branding
  pin_selfhost_image_tag "$server_ref"

  # Start Docker Compose
  info "Pulling official Patchbay images..."
  pull_official_selfhost_images
  info "Starting Patchbay services (this may take a few minutes on first run)..."
  docker compose -f docker-compose.selfhost.yml up -d

  # Read the ports Compose actually published, once, and reuse them for both the
  # health check and the summary so the two can never disagree.
  if ! SELFHOST_BACKEND_PORT="$(compose_published_port backend 8080)"; then
    fail "Started the stack but could not read the backend host port from Docker Compose.
  Check it with: cd $INSTALL_DIR && docker compose -f docker-compose.selfhost.yml ps"
  fi
  if ! SELFHOST_FRONTEND_PORT="$(compose_published_port frontend 3000)"; then
    fail "Started the stack but could not read the frontend host port from Docker Compose.
  Check it with: cd $INSTALL_DIR && docker compose -f docker-compose.selfhost.yml ps"
  fi

  # Wait for health check
  info "Waiting for backend to be ready..."
  local ready=false
  for i in $(seq 1 45); do
    if curl -sf "http://localhost:${SELFHOST_BACKEND_PORT}/health" >/dev/null 2>&1; then
      ready=true
      break
    fi
    sleep 2
  done

  if [ "$ready" = true ]; then
    ok "Patchbay server is running"
  else
    warn "Server is still starting. You can check logs with:"
    echo "  cd $INSTALL_DIR && docker compose -f docker-compose.selfhost.yml logs"
    echo ""
  fi
}


# ---------------------------------------------------------------------------
# Main: Default mode (install / upgrade CLI only)
# ---------------------------------------------------------------------------
run_default() {
  printf "\n"
  printf "${BOLD}  Patchbay — Installer${RESET}\n"
  printf "\n"

  detect_os
  install_cli

  printf "\n"
  printf "${BOLD}${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}\n"
  printf "${BOLD}${GREEN}  ✓ Patchbay CLI is ready!${RESET}\n"
  printf "${BOLD}${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}\n"
  printf "\n"
  printf "  ${BOLD}Next: configure your environment${RESET}\n"
  printf "\n"
  printf "     ${CYAN}patchbay setup${RESET}                # Connect to Patchbay Cloud (patchbay.ai)\n"
  printf "     ${CYAN}patchbay setup self-host${RESET}       # Connect to a self-hosted server\n"
  printf "\n"
  print_remote_server_token_hint
  printf "  ${BOLD}Self-hosting?${RESET} Install the server first:\n"
  printf "     curl -fsSL https://raw.githubusercontent.com/patchbay-ai/patchbay/main/scripts/install.sh | bash -s -- --with-server\n"
  printf "\n"
}

# ---------------------------------------------------------------------------
# Main: With-server mode (provision self-host infrastructure + install CLI)
# ---------------------------------------------------------------------------
run_with_server() {
  printf "\n"
  printf "${BOLD}  Patchbay — Self-Host Installer${RESET}\n"
  printf "  Provisioning server infrastructure + installing CLI\n"
  printf "\n"

  detect_os
  check_docker
  if [ "$INSTALL_SYSTEMD" = true ]; then
    preflight_selfhost_systemd
  fi
  setup_server
  install_cli
  if [ "$INSTALL_SYSTEMD" = true ]; then
    install_selfhost_systemd
  fi

  printf "\n"
  printf "${BOLD}${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}\n"
  printf "${BOLD}${GREEN}  ✓ Patchbay server is running and CLI is ready!${RESET}\n"
  printf "${BOLD}${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}\n"
  printf "\n"
  printf "  ${BOLD}Frontend:${RESET}  http://localhost:%s\n" "$SELFHOST_FRONTEND_PORT"
  printf "  ${BOLD}Backend:${RESET}   http://localhost:%s\n" "$SELFHOST_BACKEND_PORT"
  printf "  ${BOLD}Server at:${RESET} %s\n" "$INSTALL_DIR"
  printf "\n"
  printf "  ${BOLD}Next: configure your CLI to connect${RESET}\n"
  printf "\n"
  printf "     ${CYAN}patchbay setup self-host${RESET}   # Configure + authenticate + start daemon\n"
  printf "\n"
  printf "  ${BOLD}Login:${RESET} configure ${CYAN}RESEND_API_KEY${RESET} in .env for email codes,\n"
  printf "  or read the generated code from backend logs when Resend is unset.\n"
  printf "\n"
  printf "  ${BOLD}To stop all services:${RESET}\n"
  printf "     curl -fsSL https://raw.githubusercontent.com/patchbay-ai/patchbay/main/scripts/install.sh | bash -s -- --stop\n"
  printf "\n"
}

# ---------------------------------------------------------------------------
# Stop: shut down a self-hosted installation
# ---------------------------------------------------------------------------
run_stop() {
  printf "\n"
  info "Stopping Patchbay services..."

  local unit_path="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/patchbay-selfhost.service"
  if [ -f "$unit_path" ]; then
    command_exists systemctl ||
      fail "patchbay-selfhost.service exists, but systemctl is unavailable; refusing to report the service stopped."
    systemctl --user disable --now patchbay-selfhost.service ||
      fail "Could not stop and disable patchbay-selfhost.service; it may restart the stack on the next login or boot."
    ok "Systemd service stopped and disabled"
  fi

  if [ -d "$INSTALL_DIR" ]; then
    cd "$INSTALL_DIR"
    if [ -f docker-compose.selfhost.yml ]; then
      docker compose -f docker-compose.selfhost.yml down
      ok "Docker services stopped"
    else
      warn "No docker-compose.selfhost.yml found at $INSTALL_DIR"
    fi
  else
    warn "No Patchbay installation found at $INSTALL_DIR"
  fi

  if command_exists patchbay; then
    patchbay daemon stop 2>/dev/null && ok "Daemon stopped" || true
  fi

  printf "\n"
}

# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------
main() {
  local mode="default"

  while [ $# -gt 0 ]; do
    case "$1" in
      --with-server) mode="with-server" ;;
      --local)       mode="with-server" ;;  # backwards compat alias
      --systemd)     INSTALL_SYSTEMD=true ;;
      --stop)        mode="stop" ;;
      --help|-h)
        echo "Usage: install.sh [--with-server [--systemd] | --stop]"
        echo ""
        echo "  (default)       Install / upgrade the Patchbay CLI"
        echo "  --with-server   Install CLI + provision a self-host server (Docker)"
        echo "  --systemd       With --with-server, enable the Linux user service"
        echo "  --stop          Stop a self-hosted installation"
        echo ""
        echo "Environment variables:"
        echo "  PATCHBAY_INSTALL_DIR   Self-host server install directory"
        echo "                        (default: \$HOME/.patchbay/server)"
        echo "  PATCHBAY_BIN_DIR       Target directory for the CLI binary when"
        echo "                        installing from GitHub Releases"
        echo "                        (default: /usr/local/bin, then \$HOME/.local/bin)"
        echo "  PATCHBAY_SELFHOST_REF  Git ref to check out for self-host assets"
        echo "                        (default: latest release tag, falling back to main)"
        echo ""
        echo "After installation, run 'patchbay setup' to configure your environment."
        exit 0
        ;;
      *) warn "Unknown option: $1" ;;
    esac
    shift
  done

  if [ "$INSTALL_SYSTEMD" = true ] && [ "$mode" != "with-server" ]; then
    fail "--systemd requires --with-server."
  fi

  migrate_legacy_patchbay_home
  normalize_install_dir

  case "$mode" in
    default)     run_default ;;
    with-server) run_with_server ;;
    stop)        run_stop ;;
  esac
}

main "$@"
