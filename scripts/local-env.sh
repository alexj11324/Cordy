# Shared local development env derivation. Source this after loading .env.

POSTGRES_DB="${POSTGRES_DB:-patchbay}"
POSTGRES_USER="${POSTGRES_USER:-patchbay}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"

PORT="${BACKEND_PORT:-${API_PORT:-${SERVER_PORT:-${PORT:-8080}}}}"
FRONTEND_PORT="${FRONTEND_PORT:-3000}"
FRONTEND_ORIGIN="${FRONTEND_ORIGIN:-http://localhost:${FRONTEND_PORT}}"

# Older generated worktree env files predate PATCHBAY_PUBLIC_URL. Derive it
# only when the variable is absent; an explicitly configured value, including
# an intentionally empty one for same-origin proxying, must be preserved.
if [ "${PATCHBAY_PUBLIC_URL+x}" != "x" ]; then
  PATCHBAY_PUBLIC_URL="http://localhost:${PORT}"
fi
PATCHBAY_APP_URL="${PATCHBAY_APP_URL:-${FRONTEND_ORIGIN}}"
GOOGLE_REDIRECT_URI="${GOOGLE_REDIRECT_URI:-${FRONTEND_ORIGIN}/auth/callback}"
PATCHBAY_SERVER_URL="${PATCHBAY_SERVER_URL:-ws://localhost:${PORT}/ws}"
LOCAL_UPLOAD_BASE_URL="${LOCAL_UPLOAD_BASE_URL:-http://localhost:${PORT}}"
PLAYWRIGHT_BASE_URL="${PLAYWRIGHT_BASE_URL:-${FRONTEND_ORIGIN}}"

export POSTGRES_DB POSTGRES_USER POSTGRES_PORT
export PORT FRONTEND_PORT FRONTEND_ORIGIN
export PATCHBAY_PUBLIC_URL PATCHBAY_APP_URL GOOGLE_REDIRECT_URI PATCHBAY_SERVER_URL LOCAL_UPLOAD_BASE_URL
export PLAYWRIGHT_BASE_URL
