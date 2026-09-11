#!/usr/bin/env bash
# Load REUI_LICENSE_KEY from a machine-local file so every worktree can
# call the @reui registry without copying secrets into git.
#
# Precedence: already-exported REUI_LICENSE_KEY, then
# ${XDG_CONFIG_HOME:-$HOME/.config}/orvilo/reui.env, then ~/.orvilo/reui.env.
#
# shadcn interpolates ${REUI_LICENSE_KEY} from packages/ui/.env.local, not
# from the process environment, so this script materializes that gitignored
# file when a key is available.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UI_DIR="$ROOT/packages/ui"
SHARED_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/orvilo/reui.env"
SHARED_ORVILO="$HOME/.orvilo/reui.env"

load_env_file() {
  local file="$1"
  if [ -f "$file" ]; then
    set -a
    # shellcheck disable=SC1090
    source "$file"
    set +a
  fi
}

if [ -z "${REUI_LICENSE_KEY:-}" ]; then
  load_env_file "$SHARED_CONFIG"
fi
if [ -z "${REUI_LICENSE_KEY:-}" ]; then
  load_env_file "$SHARED_ORVILO"
fi

if [ -z "${REUI_LICENSE_KEY:-}" ]; then
  echo "REUI_LICENSE_KEY is not set." >&2
  echo "Write it once to $SHARED_ORVILO (gitignored, outside the repo):" >&2
  echo "  mkdir -p \"$HOME/.orvilo\"" >&2
  echo "  printf 'REUI_LICENSE_KEY=your-key\\n' > \"$SHARED_ORVILO\"" >&2
  echo "  chmod 600 \"$SHARED_ORVILO\"" >&2
  exit 1
fi

umask 077
printf 'REUI_LICENSE_KEY=%s\n' "$REUI_LICENSE_KEY" > "$UI_DIR/.env.local"

if [ "$#" -eq 0 ]; then
  echo "Wrote $UI_DIR/.env.local from the shared ReUI key."
  exit 0
fi

cd "$UI_DIR"
exec "$@"
