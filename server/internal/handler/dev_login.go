// dev_login.go — one-request local sign-in for development environments.
//
// The email verification flow always costs two round trips and an out-of-band
// code, which is friction a local run does not need and automation cannot
// spend: `send-code` is rate limited to one code per email per minute, and a
// second `verify-code` attempt on the same code locks it out. This endpoint
// exchanges an email for the same JWT and cookies that VerifyCode issues, so a
// browser can be signed in by opening one URL and a script can get a bearer
// token in one call.
//
// It exists only when APP_ENV is non-production AND PATCHBAY_DEV_LOGIN=1. Both
// halves are deliberate: the variable is the explicit opt-in, and the APP_ENV
// check makes a production deployment unable to honour the opt-in even if the
// variable leaks into its environment.
package handler

import (
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"

	"github.com/patchbay-ai/patchbay/server/internal/auth"
	"github.com/patchbay-ai/patchbay/server/internal/logger"
)

const (
	devLoginEnv      = "PATCHBAY_DEV_LOGIN"
	devLoginEmailEnv = "PATCHBAY_DEV_EMAIL"
	// Mirrors devseed.DefaultDeveloperEmail: `make seed-dev` attaches its
	// fixtures to this user, so signing in as the same address is what makes
	// the seeded workspace visible without naming an email anywhere.
	devLoginDefaultEmail = "dev@localhost"
)

type devLoginRequest struct {
	Email string `json:"email"`
	// "keep" leaves onboarded_at untouched so the onboarding flow itself can
	// be tested; any other value (including empty) skips it.
	Onboarding string `json:"onboarding"`
	Redirect   string `json:"redirect"`
}

// DevLoginEnabled reports whether the development sign-in endpoint is served.
// The router asks before registering the route, so a normal deployment does
// not expose the path at all; the handler asks again so it is inert if it is
// ever mounted by something else.
func DevLoginEnabled() bool {
	if isProductionEnv() {
		return false
	}
	return strings.TrimSpace(os.Getenv(devLoginEnv)) == "1"
}

func devLoginEmail(requested string) string {
	if email := strings.ToLower(strings.TrimSpace(requested)); email != "" {
		return email
	}
	if email := strings.ToLower(strings.TrimSpace(os.Getenv(devLoginEmailEnv))); email != "" {
		return email
	}
	return devLoginDefaultEmail
}

// devLoginRedirect resolves the post-login location. Only a same-origin path is
// accepted: this endpoint sets a session cookie, so honouring an arbitrary
// target would hand a freshly authenticated browser to whoever wrote the URL.
// "//host" and "/\host" are rejected for the same reason as "https://host" —
// browsers read both as an authority, not a path.
func devLoginRedirect(raw string) string {
	path := strings.TrimSpace(raw)
	if path == "" {
		return "/"
	}
	if !strings.HasPrefix(path, "/") || strings.HasPrefix(path, "//") || strings.HasPrefix(path, "/\\") {
		return "/"
	}
	if parsed, err := url.Parse(path); err != nil || parsed.Scheme != "" || parsed.Host != "" {
		return "/"
	}
	return path
}

// devLoginAppOrigin is where a browser sign-in lands. FRONTEND_ORIGIN is what
// the cookie flags are already derived from, so the redirect follows the same
// value rather than a second, possibly divergent, notion of "the app".
func devLoginAppOrigin() string {
	for _, candidate := range []string{os.Getenv("FRONTEND_ORIGIN"), os.Getenv("PATCHBAY_APP_URL")} {
		if origin := strings.TrimRight(strings.TrimSpace(candidate), "/"); origin != "" {
			return origin
		}
	}
	return ""
}

// DevLogin signs in as an email without a verification code.
//
// GET redirects to the web app with the session cookies set — that is the
// "open one URL and you are inside" path. POST answers with the same
// LoginResponse as VerifyCode, for curl and test setup.
func (h *Handler) DevLogin(w http.ResponseWriter, r *http.Request) {
	if !DevLoginEnabled() {
		writeError(w, http.StatusNotFound, "not found")
		return
	}

	var req devLoginRequest
	if r.Method == http.MethodPost && r.ContentLength > 0 {
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			writeError(w, http.StatusBadRequest, "invalid request body")
			return
		}
	}
	query := r.URL.Query()
	if req.Email == "" {
		req.Email = query.Get("email")
	}
	if req.Onboarding == "" {
		req.Onboarding = query.Get("onboarding")
	}
	if req.Redirect == "" {
		req.Redirect = query.Get("redirect")
	}

	email := devLoginEmail(req.Email)
	user, _, err := h.findOrCreateUser(r.Context(), email)
	if err != nil {
		if errors.Is(err, auth.ErrTemporarilyDisabledUser) {
			writeError(w, http.StatusForbidden, auth.TemporarilyDisabledUserError)
			return
		}
		var signupErr SignupError
		if errors.As(err, &signupErr) {
			writeError(w, http.StatusForbidden, signupErr.Error())
			return
		}
		slog.Warn("dev login: find or create user failed", append(logger.RequestAttrs(r), "error", err, "email", email)...)
		writeError(w, http.StatusInternalServerError, "failed to create user")
		return
	}

	// A user with onboarded_at = NULL is bounced to /onboarding, which would
	// make the redirect land somewhere other than the page the caller asked
	// for. Skipping it is the default because "skip the login" is what this
	// endpoint is for; `onboarding=keep` is how the onboarding flow itself
	// stays testable.
	if !strings.EqualFold(strings.TrimSpace(req.Onboarding), "keep") && !user.OnboardedAt.Valid {
		onboarded, err := h.Queries.MarkUserOnboarded(r.Context(), user.ID)
		if err != nil {
			slog.Warn("dev login: mark user onboarded failed", append(logger.RequestAttrs(r), "error", err)...)
			writeError(w, http.StatusInternalServerError, "failed to complete onboarding")
			return
		}
		user = onboarded
	}

	// No signup analytics event here, unlike VerifyCode: a developer signing
	// themselves in is not a signup, and counting it would put local noise in
	// the same funnel the product is measured by.
	token, err := h.issueJWT(user)
	if err != nil {
		if errors.Is(err, auth.ErrTemporarilyDisabledUser) {
			writeError(w, http.StatusForbidden, auth.TemporarilyDisabledUserError)
			return
		}
		slog.Warn("dev login failed", append(logger.RequestAttrs(r), "error", err, "email", email)...)
		writeError(w, http.StatusInternalServerError, "failed to generate token")
		return
	}

	if err := auth.SetAuthCookies(w, token); err != nil {
		slog.Warn("dev login: failed to set auth cookies", "error", err)
	}
	if h.CFSigner != nil {
		for _, cookie := range h.CFSigner.SignedCookies(time.Now().Add(auth.AuthTokenTTL())) {
			http.SetCookie(w, cookie)
		}
	}

	slog.Info("dev login", append(logger.RequestAttrs(r), "user_id", uuidToString(user.ID), "email", user.Email)...)

	if r.Method == http.MethodGet {
		http.Redirect(w, r, devLoginAppOrigin()+devLoginRedirect(req.Redirect), http.StatusFound)
		return
	}
	writeJSON(w, http.StatusOK, LoginResponse{Token: token, User: h.userToResponse(user)})
}
