#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
env_file="${1:-$root_dir/.env}"
[ ! -e "$env_file" ] || exit 0
umask 077
db_password="$(openssl rand -hex 24)"
cp "$root_dir/.env.example" "$env_file"
if [ "$(uname)" = Darwin ]; then
  sed -i '' "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$db_password/" "$env_file"
else
  sed -i "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$db_password/" "$env_file"
fi
