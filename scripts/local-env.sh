# Shared local development env derivation. Source this after loading .env.

POSTGRES_DB="${POSTGRES_DB:-orvilo}"
POSTGRES_USER="${POSTGRES_USER:-orvilo}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"

PORT="${BACKEND_PORT:-${API_PORT:-${SERVER_PORT:-${PORT:-8080}}}}"
FRONTEND_PORT="${FRONTEND_PORT:-3000}"
FRONTEND_ORIGIN="${FRONTEND_ORIGIN:-http://localhost:${FRONTEND_PORT}}"

# Older generated worktree env files predate ORVILO_PUBLIC_URL. Derive it
# only when the variable is absent; an explicitly configured value, including
# an intentionally empty one for same-origin proxying, must be preserved.
if [ "${ORVILO_PUBLIC_URL+x}" != "x" ]; then
  ORVILO_PUBLIC_URL="http://localhost:${PORT}"
fi
ORVILO_APP_URL="${ORVILO_APP_URL:-${FRONTEND_ORIGIN}}"
GOOGLE_REDIRECT_URI="${GOOGLE_REDIRECT_URI:-${FRONTEND_ORIGIN}/auth/callback}"
ORVILO_SERVER_URL="${ORVILO_SERVER_URL:-ws://localhost:${PORT}/ws}"
LOCAL_UPLOAD_BASE_URL="${LOCAL_UPLOAD_BASE_URL:-http://localhost:${PORT}}"
PLAYWRIGHT_BASE_URL="${PLAYWRIGHT_BASE_URL:-${FRONTEND_ORIGIN}}"

export POSTGRES_DB POSTGRES_USER POSTGRES_PORT
export PORT FRONTEND_PORT FRONTEND_ORIGIN
export ORVILO_PUBLIC_URL ORVILO_APP_URL GOOGLE_REDIRECT_URI ORVILO_SERVER_URL LOCAL_UPLOAD_BASE_URL
export PLAYWRIGHT_BASE_URL

# Local Desktop trusts hosted Accounts for identity, but mints its own session.
# The browser carries only a PKCE-bound code; no production bearer reaches it.
ORVILO_HOSTED_DESKTOP_IDENTITY="${ORVILO_HOSTED_DESKTOP_IDENTITY:-1}"
export ORVILO_HOSTED_DESKTOP_IDENTITY
