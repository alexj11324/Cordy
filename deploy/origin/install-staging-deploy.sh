#!/usr/bin/env bash
set -Eeuo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "install-staging-deploy.sh must run as root" >&2
  exit 1
fi

if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
  echo "usage: install-staging-deploy.sh <github-actions-public-key-file>" >&2
  exit 1
fi

deploy_user="${PATCHBAY_DEPLOY_USER:-ubuntu}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
state_dir="/var/lib/patchbay-staging"
static_dir="/usr/local/share/patchbay-staging"
production_state_dir="/var/lib/patchbay-production"

if [ "$state_dir" = "$production_state_dir" ]; then
  echo "refusing to install staging into the production state directory" >&2
  exit 1
fi

if ! id "$deploy_user" >/dev/null 2>&1; then
  echo "deployment user does not exist: $deploy_user" >&2
  exit 1
fi
deploy_home="$(getent passwd "$deploy_user" | cut -d: -f6)"
deploy_group="$(id -gn "$deploy_user")"
ssh_dir="$deploy_home/.ssh"
authorized_keys="$ssh_dir/authorized_keys"
if [ -z "$deploy_home" ] || [ ! -d "$deploy_home" ]; then
  echo "deployment user has no usable home directory: $deploy_user" >&2
  exit 1
fi
for protected_path in "$state_dir" "$static_dir" "$ssh_dir" "$authorized_keys"; do
  if [ -L "$protected_path" ]; then
    echo "refusing symlinked staging deployment path: $protected_path" >&2
    exit 1
  fi
done
for dependency in docker git runuser; do
  if ! command -v "$dependency" >/dev/null 2>&1; then
    echo "missing staging deployment dependency: $dependency" >&2
    exit 1
  fi
done
docker compose version >/dev/null

install -o root -g root -m 0755 \
  "$script_dir/staging_deploy.py" \
  /usr/local/bin/patchbay-staging-deploy
install -d -o root -g root -m 0755 "$static_dir"
install -o root -g root -m 0644 \
  "$script_dir/staging-product.override.yml" \
  "$static_dir/staging-product.override.yml"
install -o root -g root -m 0644 \
  "$script_dir/staging-docs.compose.yml" \
  "$static_dir/staging-docs.compose.yml"
install -o root -g root -m 0644 \
  "$script_dir/staging-auth-broker.compose.yml" \
  "$static_dir/staging-auth-broker.compose.yml"

install -d -o "$deploy_user" -g "$deploy_group" -m 0700 "$state_dir"
install -d -o "$deploy_user" -g "$deploy_group" -m 0700 "$state_dir/secrets"
install -d -o "$deploy_user" -g "$deploy_group" -m 0700 "$ssh_dir"
touch "$authorized_keys"
chown "$deploy_user:$deploy_group" "$authorized_keys"
chmod 0600 "$authorized_keys"

if [ ! -f "$state_dir/secrets/product-env.json" ] || [ ! -f "$state_dir/secrets/auth-broker-env.json" ]; then
  echo "place staging product-env.json and auth-broker-env.json in $state_dir/secrets before bootstrap" >&2
  exit 1
fi

public_key="$(awk 'NF >= 2 { print $1 " " $2; exit }' "$1")"
if [[ ! "$public_key" =~ ^(ssh-ed25519|sk-ssh-ed25519@openssh.com)[[:space:]][A-Za-z0-9+/=]+$ ]]; then
  echo "deployment key must be an Ed25519 public key" >&2
  exit 1
fi
forced_entry="restrict,command=\"/usr/local/bin/patchbay-staging-deploy\" $public_key patchbay-staging-github-actions"

if [ -f "$state_dir/bootstrapped.json" ]; then
  runuser -u "$deploy_user" -- /usr/local/bin/patchbay-staging-deploy --check
else
  runuser -u "$deploy_user" -- /usr/local/bin/patchbay-staging-deploy --bootstrap
fi

temporary_keys="$(mktemp "$deploy_home/.ssh/.authorized_keys.XXXXXX")"
trap 'rm -f "$temporary_keys"' EXIT
awk '$NF != "patchbay-staging-github-actions"' "$authorized_keys" > "$temporary_keys"
printf '%s\n' "$forced_entry" >> "$temporary_keys"
chown "$deploy_user:$deploy_group" "$temporary_keys"
chmod 0600 "$temporary_keys"
mv -f "$temporary_keys" "$authorized_keys"
trap - EXIT
