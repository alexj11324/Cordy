#!/usr/bin/env bash
set -Eeuo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "install-production-deploy.sh must run as root" >&2
  exit 1
fi

if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
  echo "usage: install-production-deploy.sh <github-actions-public-key-file>" >&2
  exit 1
fi

deploy_user="${PATCHBAY_DEPLOY_USER:-ubuntu}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
state_dir="/var/lib/patchbay-production"
static_dir="/usr/local/share/patchbay-production"

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
    echo "refusing symlinked production deployment path: $protected_path" >&2
    exit 1
  fi
done
for dependency in docker git runuser; do
  if ! command -v "$dependency" >/dev/null 2>&1; then
    echo "missing production deployment dependency: $dependency" >&2
    exit 1
  fi
done
docker compose version >/dev/null

install -o root -g root -m 0755 \
  "$script_dir/production_deploy.py" \
  /usr/local/bin/patchbay-production-deploy
install -d -o root -g root -m 0755 "$static_dir"
install -o root -g root -m 0644 \
  "$script_dir/production-product.override.yml" \
  "$static_dir/production-product.override.yml"
install -o root -g root -m 0644 \
  "$script_dir/production-docs.compose.yml" \
  "$static_dir/production-docs.compose.yml"

install -d -o "$deploy_user" -g "$deploy_group" -m 0700 "$state_dir"
install -d -o "$deploy_user" -g "$deploy_group" -m 0700 "$ssh_dir"
touch "$authorized_keys"
chown "$deploy_user:$deploy_group" "$authorized_keys"
chmod 0600 "$authorized_keys"

public_key="$(awk 'NF >= 2 { print $1 " " $2; exit }' "$1")"
if [[ ! "$public_key" =~ ^(ssh-ed25519|sk-ssh-ed25519@openssh.com)[[:space:]][A-Za-z0-9+/=]+$ ]]; then
  echo "deployment key must be an Ed25519 public key" >&2
  exit 1
fi
forced_entry="restrict,command=\"/usr/local/bin/patchbay-production-deploy\" $public_key patchbay-production-github-actions"

# Do not authorize remote deployment until the local baseline has been
# captured or the existing rollback state has passed validation. Reinstalling
# the gateway must not overwrite deployment history.
if [ -f "$state_dir/current.json" ]; then
  runuser -u "$deploy_user" -- /usr/local/bin/patchbay-production-deploy --check
else
  runuser -u "$deploy_user" -- /usr/local/bin/patchbay-production-deploy --bootstrap
fi

temporary_keys="$(mktemp "$deploy_home/.ssh/.authorized_keys.XXXXXX")"
trap 'rm -f "$temporary_keys"' EXIT
awk '$NF != "patchbay-production-github-actions"' "$authorized_keys" > "$temporary_keys"
printf '%s\n' "$forced_entry" >> "$temporary_keys"
chown "$deploy_user:$deploy_group" "$temporary_keys"
chmod 0600 "$temporary_keys"
mv -f "$temporary_keys" "$authorized_keys"
trap - EXIT
