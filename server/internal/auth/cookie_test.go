package auth

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestIsSecureCookie(t *testing.T) {
	cases := []struct {
		name           string
		frontendOrigin string
		want           bool
	}{
		{"https origin → Secure", "https://app.example.com", true},
		{"https with port", "https://app.example.com:8443", true},
		{"http origin → not Secure", "http://192.168.5.5:13000", false},
		{"http localhost → not Secure", "http://localhost:3000", false},
		{"empty → not Secure", "", false},
		{"malformed → not Secure", "::not-a-url", false},
		{"uppercase scheme still matches", "HTTPS://app.example.com", true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Setenv("FRONTEND_ORIGIN", tc.frontendOrigin)
			if got := isSecureCookie(); got != tc.want {
				t.Errorf("isSecureCookie() = %v, want %v (FRONTEND_ORIGIN=%q)", got, tc.want, tc.frontendOrigin)
			}
		})
	}
}

func TestCookieDomain(t *testing.T) {
	cases := []struct {
		name string
		env  string
		want string
	}{
		{"empty", "", ""},
		{"whitespace only", "   ", ""},
		{"real domain", ".example.com", ".example.com"},
		{"bare domain", "example.com", "example.com"},
		{"IPv4 rejected", "192.168.5.5", ""},
		{"IPv4 with leading dot rejected", ".192.168.5.5", ""},
		{"IPv6 rejected", "::1", ""},
		{"IPv6 bracketed is not a valid IP literal → passthrough", "[::1]", "[::1]"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Setenv("COOKIE_DOMAIN", tc.env)
			if got := cookieDomain(); got != tc.want {
				t.Errorf("cookieDomain() = %q, want %q (COOKIE_DOMAIN=%q)", got, tc.want, tc.env)
			}
		})
	}
}

// TestSetAuthCookies_HTTPSelfHost covers the exact misconfiguration that
// shipped to users on LAN self-host: COOKIE_DOMAIN=<ip> + HTTP FRONTEND_ORIGIN.
// The cookie must land with no Domain attribute and Secure=false so browsers
// actually store it.
func TestSetAuthCookies_HTTPSelfHost(t *testing.T) {
	t.Setenv("FRONTEND_ORIGIN", "http://192.168.5.5:13000")
	t.Setenv("COOKIE_DOMAIN", "192.168.5.5")

	rec := httptest.NewRecorder()
	if err := SetAuthCookies(rec, "test-token"); err != nil {
		t.Fatalf("SetAuthCookies: %v", err)
	}

	cookies := rec.Result().Cookies()
	if len(cookies) != 2 {
		t.Fatalf("expected 2 cookies (auth + csrf), got %d", len(cookies))
	}
	for _, c := range cookies {
		if c.Secure {
			t.Errorf("cookie %q has Secure=true on HTTP origin; browser would reject it", c.Name)
		}
		if c.Domain != "" {
			t.Errorf("cookie %q has Domain=%q; IP-address Domain would be rejected by the browser (RFC 6265)", c.Name, c.Domain)
		}
	}
}

func TestParseAuthTokenTTL(t *testing.T) {
	cases := []struct {
		name    string
		raw     string
		wantDur time.Duration
		wantOK  bool
	}{
		{"empty string", "", 0, false},
		{"valid 3600", "3600", time.Hour, true},
		{"valid 86400", "86400", 24 * time.Hour, true},
		{"negative", "-100", 0, false},
		{"zero", "0", 0, false},
		{"non-numeric", "abc", 0, false},
		{"whitespace trimmed", " 7200 ", 2 * time.Hour, true},
		{"duration hours", "8760h", 8760 * time.Hour, true},
		{"duration compound", "720h30m", 720*time.Hour + 30*time.Minute, true},
		{"duration minutes", "90m", 90 * time.Minute, true},
		{"duration negative", "-1h", 0, false},
		{"duration zero", "0s", 0, false},
		{"integer overflow", "9999999999", 0, false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got, ok := parseAuthTokenTTL(tc.raw)
			if ok != tc.wantOK || got != tc.wantDur {
				t.Errorf("parseAuthTokenTTL(%q) = (%v, %v), want (%v, %v)", tc.raw, got, ok, tc.wantDur, tc.wantOK)
			}
		})
	}
}

func TestSetAuthCookies_HTTPSProduction(t *testing.T) {
	t.Setenv("FRONTEND_ORIGIN", "https://app.example.com")
	t.Setenv("COOKIE_DOMAIN", "app.example.com")
	t.Setenv("AUTH_COOKIE_NAME", "")
	t.Setenv("CSRF_COOKIE_NAME", "")

	rec := httptest.NewRecorder()
	if err := SetAuthCookies(rec, "test-token"); err != nil {
		t.Fatalf("SetAuthCookies: %v", err)
	}

	var sawAuth, sawCSRF bool
	for _, c := range rec.Result().Cookies() {
		if !c.Secure {
			t.Errorf("cookie %q missing Secure flag on HTTPS origin", c.Name)
		}
		if c.Domain != "app.example.com" {
			t.Errorf("cookie %q Domain = %q, want %q", c.Name, c.Domain, "app.example.com")
		}
		switch c.Name {
		case AuthCookieName:
			sawAuth = true
		case CSRFCookieName:
			sawCSRF = true
		default:
			t.Errorf("unexpected cookie name %q", c.Name)
		}
	}
	if !sawAuth || !sawCSRF {
		t.Fatalf("production defaults missing: auth=%v csrf=%v", sawAuth, sawCSRF)
	}
}

func TestCookieNamesFromEnv(t *testing.T) {
	cases := []struct {
		name     string
		authEnv  string
		csrfEnv  string
		wantAuth string
		wantCSRF string
	}{
		{"unset", "", "", AuthCookieName, CSRFCookieName},
		{"whitespace", "  ", "\t", AuthCookieName, CSRFCookieName},
		{"unsupported custom names", "custom_auth", "custom_csrf", AuthCookieName, CSRFCookieName},
		{"production explicit", AuthCookieName, CSRFCookieName, AuthCookieName, CSRFCookieName},
		{"staging", "patchbay_staging_auth", "patchbay_staging_csrf", "patchbay_staging_auth", "patchbay_staging_csrf"},
		{"hyphen rejected", "patchbay-staging-auth", "patchbay-staging-csrf", AuthCookieName, CSRFCookieName},
		{"semicolon rejected", "patchbay_auth;evil", "", AuthCookieName, CSRFCookieName},
		{"too long rejected", strings.Repeat("a", 65), strings.Repeat("b", 65), AuthCookieName, CSRFCookieName},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Setenv("AUTH_COOKIE_NAME", tc.authEnv)
			t.Setenv("CSRF_COOKIE_NAME", tc.csrfEnv)
			if got := AuthCookie(); got != tc.wantAuth {
				t.Errorf("AuthCookie() = %q, want %q", got, tc.wantAuth)
			}
			if got := CSRFCookie(); got != tc.wantCSRF {
				t.Errorf("CSRFCookie() = %q, want %q", got, tc.wantCSRF)
			}
		})
	}
}

func TestSetAuthCookies_StagingNamesIgnoreProductionCookie(t *testing.T) {
	t.Setenv("FRONTEND_ORIGIN", "https://staging.aspectlylabs.com")
	t.Setenv("COOKIE_DOMAIN", ".staging.aspectlylabs.com")
	t.Setenv("AUTH_COOKIE_NAME", "patchbay_staging_auth")
	t.Setenv("CSRF_COOKIE_NAME", "patchbay_staging_csrf")

	rec := httptest.NewRecorder()
	if err := SetAuthCookies(rec, "staging-token"); err != nil {
		t.Fatalf("SetAuthCookies: %v", err)
	}

	var authCookie, csrfCookie *http.Cookie
	for _, c := range rec.Result().Cookies() {
		switch c.Name {
		case "patchbay_staging_auth":
			authCookie = c
		case "patchbay_staging_csrf":
			csrfCookie = c
		case AuthCookieName, CSRFCookieName:
			t.Errorf("SetAuthCookies wrote production cookie %q", c.Name)
		}
	}
	if authCookie == nil || csrfCookie == nil {
		t.Fatalf("expected staging auth+csrf cookies, got %+v", rec.Result().Cookies())
	}

	req := httptest.NewRequest(http.MethodPost, "/api/issues", nil)
	req.AddCookie(&http.Cookie{Name: AuthCookieName, Value: "production-token"})
	req.AddCookie(authCookie)
	req.Header.Set("X-CSRF-Token", csrfCookie.Value)
	if !ValidateCSRF(req) {
		t.Fatal("ValidateCSRF rejected the staging cookie pair while a production cookie was also present")
	}

	clearRec := httptest.NewRecorder()
	ClearAuthCookies(clearRec)
	for _, c := range clearRec.Result().Cookies() {
		if c.Name == AuthCookieName || c.Name == CSRFCookieName {
			t.Errorf("ClearAuthCookies expired production cookie %q", c.Name)
		}
	}
}
