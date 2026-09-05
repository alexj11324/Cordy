package handler

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/auth"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
)

func enableDevLogin(t *testing.T) {
	t.Helper()
	t.Setenv("APP_ENV", "development")
	t.Setenv(devLoginEnv, "1")
}

func deleteUserAfterTest(t *testing.T, email string) {
	t.Helper()
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM "user" WHERE email = $1`, email)
	})
}

func authCookie(t *testing.T, res *testutil.Response) *http.Cookie {
	t.Helper()
	for _, cookie := range res.Result().Cookies() {
		if cookie.Name == auth.AuthCookieName {
			return cookie
		}
	}
	return nil
}

func TestDevLoginDisabledByDefault(t *testing.T) {
	t.Setenv("APP_ENV", "")
	t.Setenv(devLoginEnv, "")

	res := testutil.Call(t, testHandler.DevLogin,
		testutil.JSONRequest(http.MethodPost, "/auth/dev-login", map[string]string{"email": "dev-login-off@patchbay.ai"}))
	res.Want(http.StatusNotFound)
}

func TestDevLoginIgnoredInProduction(t *testing.T) {
	t.Setenv("APP_ENV", "production")
	t.Setenv(devLoginEnv, "1")

	res := testutil.Call(t, testHandler.DevLogin,
		testutil.JSONRequest(http.MethodPost, "/auth/dev-login", map[string]string{"email": "dev-login-prod@patchbay.ai"}))
	res.Want(http.StatusNotFound)
}

func TestDevLoginCreatesUserAndIssuesSession(t *testing.T) {
	enableDevLogin(t)
	const email = "dev-login-new@patchbay.ai"
	deleteUserAfterTest(t, email)

	var body LoginResponse
	res := testutil.Call(t, testHandler.DevLogin,
		testutil.JSONRequest(http.MethodPost, "/auth/dev-login", map[string]string{"email": email}))
	res.Want(http.StatusOK).JSON(&body)

	if body.Token == "" {
		t.Fatal("dev login: expected a token in the response")
	}
	if body.User.Email != email {
		t.Fatalf("dev login: expected user %q, got %q", email, body.User.Email)
	}
	cookie := authCookie(t, res)
	if cookie == nil || cookie.Value != body.Token {
		// Without the cookie the browser one-click URL signs nobody in: the
		// web app authenticates from the cookie, not the JSON token.
		t.Fatalf("dev login: expected %s cookie carrying the token, got %+v", auth.AuthCookieName, cookie)
	}

	user, err := testHandler.Queries.GetUserByEmail(context.Background(), email)
	if err != nil {
		t.Fatalf("dev login: user was not created: %v", err)
	}
	if !user.OnboardedAt.Valid {
		t.Fatal("dev login: expected onboarding to be completed so the redirect is not bounced to /onboarding")
	}
}

func TestDevLoginKeepsOnboardingWhenAsked(t *testing.T) {
	enableDevLogin(t)
	const email = "dev-login-onboarding@patchbay.ai"
	deleteUserAfterTest(t, email)

	res := testutil.Call(t, testHandler.DevLogin,
		testutil.JSONRequest(http.MethodPost, "/auth/dev-login", map[string]string{"email": email, "onboarding": "keep"}))
	res.Want(http.StatusOK)

	user, err := testHandler.Queries.GetUserByEmail(context.Background(), email)
	if err != nil {
		t.Fatalf("dev login: user was not created: %v", err)
	}
	if user.OnboardedAt.Valid {
		t.Fatal("dev login: onboarding=keep must leave onboarded_at NULL so the onboarding flow stays testable")
	}
}

func TestDevLoginReusesExistingUser(t *testing.T) {
	enableDevLogin(t)
	const email = "dev-login-existing@patchbay.ai"
	deleteUserAfterTest(t, email)

	var first, second LoginResponse
	testutil.Call(t, testHandler.DevLogin,
		testutil.JSONRequest(http.MethodPost, "/auth/dev-login", map[string]string{"email": email})).
		Want(http.StatusOK).JSON(&first)
	testutil.Call(t, testHandler.DevLogin,
		testutil.JSONRequest(http.MethodPost, "/auth/dev-login", map[string]string{"email": email})).
		Want(http.StatusOK).JSON(&second)

	if first.User.ID != second.User.ID {
		t.Fatalf("dev login: expected the same user across calls, got %s then %s", first.User.ID, second.User.ID)
	}
}

func TestDevLoginDefaultsToConfiguredEmail(t *testing.T) {
	enableDevLogin(t)
	const email = "dev-login-default@patchbay.ai"
	t.Setenv(devLoginEmailEnv, email)
	deleteUserAfterTest(t, email)

	var body LoginResponse
	testutil.Call(t, testHandler.DevLogin, httptest.NewRequest(http.MethodPost, "/auth/dev-login", nil)).
		Want(http.StatusOK).JSON(&body)

	if body.User.Email != email {
		t.Fatalf("dev login: expected the %s user %q, got %q", devLoginEmailEnv, email, body.User.Email)
	}
}

func TestDevLoginGetRedirectsIntoTheApp(t *testing.T) {
	enableDevLogin(t)
	t.Setenv("FRONTEND_ORIGIN", "http://localhost:13000")
	const email = "dev-login-redirect@patchbay.ai"
	deleteUserAfterTest(t, email)

	res := testutil.Call(t, testHandler.DevLogin,
		httptest.NewRequest(http.MethodGet, "/auth/dev-login?email="+email+"&redirect=/dev/issues", nil))
	res.Want(http.StatusFound)

	if got := res.Header().Get("Location"); got != "http://localhost:13000/dev/issues" {
		t.Fatalf("dev login: expected a redirect into the web app, got %q", got)
	}
	if authCookie(t, res) == nil {
		t.Fatalf("dev login: browser redirect did not set the %s cookie", auth.AuthCookieName)
	}
}

// A body sent with chunked transfer encoding reports ContentLength -1. Gating
// the decode on a positive length silently dropped the caller's email and
// signed them in as the default user instead.
func TestDevLoginReadsBodyWithUnknownContentLength(t *testing.T) {
	enableDevLogin(t)
	const email = "dev-login-chunked@patchbay.ai"
	deleteUserAfterTest(t, email)

	req := testutil.JSONRequest(http.MethodPost, "/auth/dev-login", map[string]string{"email": email})
	req.ContentLength = -1

	var body LoginResponse
	testutil.Call(t, testHandler.DevLogin, req).Want(http.StatusOK).JSON(&body)

	if body.User.Email != email {
		t.Fatalf("dev login: streamed body was ignored — signed in as %q, want %q", body.User.Email, email)
	}
}

// PATCHBAY_APP_URL is the repository's user-facing app URL; a deployment that
// sets it away from FRONTEND_ORIGIN means the browser to land on the former.
func TestDevLoginRedirectPrefersConfiguredAppURL(t *testing.T) {
	enableDevLogin(t)
	t.Setenv("FRONTEND_ORIGIN", "http://localhost:13000")
	t.Setenv("PATCHBAY_APP_URL", "https://app.example.com")
	const email = "dev-login-appurl@patchbay.ai"
	deleteUserAfterTest(t, email)

	res := testutil.Call(t, testHandler.DevLogin,
		httptest.NewRequest(http.MethodGet, "/auth/dev-login?email="+email+"&redirect=/dev/issues", nil))
	res.Want(http.StatusFound)

	if got := res.Header().Get("Location"); got != "https://app.example.com/dev/issues" {
		t.Fatalf("dev login: expected the redirect to use PATCHBAY_APP_URL, got %q", got)
	}
}

// The redirect target is attacker-controllable input on a request that sets a
// session cookie, so the matrix for what counts as a same-origin path lives
// here rather than being re-run through the handler.
func TestDevLoginRedirectRejectsOffOriginTargets(t *testing.T) {
	cases := []struct {
		raw  string
		want string
	}{
		{"", "/"},
		{"/", "/"},
		{"/dev/issues", "/dev/issues"},
		{"/dev/issues?tab=all#top", "/dev/issues?tab=all#top"},
		{"dev/issues", "/"},
		{"//evil.example.com/path", "/"},
		{"/\\evil.example.com/path", "/"},
		{"https://evil.example.com", "/"},
		{"javascript:alert(1)", "/"},
		{"  /dev/issues  ", "/dev/issues"},
	}
	for _, tc := range cases {
		if got := devLoginRedirect(tc.raw); got != tc.want {
			t.Errorf("devLoginRedirect(%q) = %q, want %q", tc.raw, got, tc.want)
		}
	}
}

func TestDevLoginEmailFallbackOrder(t *testing.T) {
	t.Setenv(devLoginEmailEnv, "configured@example.com")
	if got := devLoginEmail("  Requested@Example.com "); got != "requested@example.com" {
		t.Errorf("devLoginEmail(requested) = %q, want the normalised request value", got)
	}
	if got := devLoginEmail(""); got != "configured@example.com" {
		t.Errorf("devLoginEmail(\"\") = %q, want the %s value", got, devLoginEmailEnv)
	}
	t.Setenv(devLoginEmailEnv, "")
	if got := devLoginEmail(""); got != devLoginDefaultEmail {
		t.Errorf("devLoginEmail(\"\") = %q, want %q", got, devLoginDefaultEmail)
	}
}
