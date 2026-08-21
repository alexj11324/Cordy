# Shared local development env derivation. Source this after loading .env.

POSTGRES_DB="${POSTGRES_DB:-cordy}"
POSTGRES_USER="${POSTGRES_USER:-cordy}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"

PORT="${BACKEND_PORT:-${API_PORT:-${SERVER_PORT:-${PORT:-8080}}}}"
FRONTEND_PORT="${FRONTEND_PORT:-3000}"
FRONTEND_ORIGIN="${FRONTEND_ORIGIN:-http://localhost:${FRONTEND_PORT}}"

# Older generated worktree env files predate CORDY_PUBLIC_URL. Derive it
# only when the variable is absent; an explicitly configured value, including
# an intentionally empty one for same-origin proxying, must be preserved.
if [ "${CORDY_PUBLIC_URL+x}" != "x" ]; then
  CORDY_PUBLIC_URL="http://localhost:${PORT}"
fi
CORDY_APP_URL="${CORDY_APP_URL:-${FRONTEND_ORIGIN}}"
GOOGLE_REDIRECT_URI="${GOOGLE_REDIRECT_URI:-${FRONTEND_ORIGIN}/auth/callback}"
CORDY_SERVER_URL="${CORDY_SERVER_URL:-ws://localhost:${PORT}/ws}"
LOCAL_UPLOAD_BASE_URL="${LOCAL_UPLOAD_BASE_URL:-http://localhost:${PORT}}"
PLAYWRIGHT_BASE_URL="${PLAYWRIGHT_BASE_URL:-${FRONTEND_ORIGIN}}"

export POSTGRES_DB POSTGRES_USER POSTGRES_PORT
export PORT FRONTEND_PORT FRONTEND_ORIGIN
export CORDY_PUBLIC_URL CORDY_APP_URL GOOGLE_REDIRECT_URI CORDY_SERVER_URL LOCAL_UPLOAD_BASE_URL
export PLAYWRIGHT_BASE_URL
