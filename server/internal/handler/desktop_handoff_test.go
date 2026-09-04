package handler

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestRequireDesktopBrokerAuthFailsClosedAndStripsCredential(t *testing.T) {
	secret := strings.Repeat("a", 64)
	tests := []struct {
		name     string
		expected string
		provided []string
		status   int
		called   bool
	}{
		{name: "valid", expected: secret, provided: []string{secret}, status: http.StatusNoContent, called: true},
		{name: "missing", expected: secret, status: http.StatusForbidden},
		{name: "duplicate", expected: secret, provided: []string{secret, secret}, status: http.StatusForbidden},
		{name: "malformed configuration", expected: "short", provided: []string{"short"}, status: http.StatusServiceUnavailable},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req := httptest.NewRequest(http.MethodPost, "/api/desktop-google/attempt", nil)
			for _, value := range tt.provided {
				req.Header.Add(desktopBrokerAuthHeader, value)
			}
			called := false
			next := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				called = true
				if r.Header.Get(desktopBrokerAuthHeader) != "" {
					t.Fatal("broker credential reached handler")
				}
				w.WriteHeader(http.StatusNoContent)
			})
			recorder := httptest.NewRecorder()
			RequireDesktopBrokerAuth(tt.expected)(next).ServeHTTP(recorder, req)
			if called != tt.called {
				t.Fatalf("called = %v, want %v", called, tt.called)
			}
			if recorder.Code != tt.status {
				body, _ := io.ReadAll(recorder.Body)
				t.Fatalf("status = %d, want %d: %s", recorder.Code, tt.status, body)
			}
		})
	}
}

func TestValidateDesktopGoogleAttemptMatchesBrokerPKCERange(t *testing.T) {
	for _, length := range []int{43, 128} {
		if !validDesktopGoogleAttempt(desktopGoogleAttemptRequest{State: strings.Repeat("s", length), CodeChallenge: strings.Repeat("c", length)}) {
			t.Fatalf("valid length %d rejected", length)
		}
	}
	if validDesktopGoogleAttempt(desktopGoogleAttemptRequest{State: strings.Repeat("s", 42), CodeChallenge: strings.Repeat("c", 43)}) {
		t.Fatal("short state accepted")
	}
}

func TestRequireFormalDesktopAuthActorRejectsGuestAndMachineCredentials(t *testing.T) {
	tests := []struct {
		name   string
		header string
		value  string
	}{
		{name: "guest bearer", header: "X-Guest-User", value: "true"},
		{name: "task token", header: "X-Actor-Source", value: "task_token"},
		{name: "cloud node", header: "X-Actor-Source", value: "cloud_pat"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req := httptest.NewRequest(http.MethodPost, "/api/desktop-handoff/complete", nil)
			req.Header.Set(tt.header, tt.value)
			called := false
			next := http.HandlerFunc(func(http.ResponseWriter, *http.Request) {
				called = true
			})

			recorder := httptest.NewRecorder()
			if requireFormalDesktopAuthActor(recorder, req) {
				next.ServeHTTP(recorder, req)
			}

			if called {
				t.Fatal("non-formal actor reached desktop handoff completion")
			}
			if recorder.Code != http.StatusForbidden {
				t.Fatalf("status = %d, want %d", recorder.Code, http.StatusForbidden)
			}
		})
	}
}

func TestParseDesktopLoopbackSessionAcceptsFormFields(t *testing.T) {
	state := strings.Repeat("s", 43)
	challenge := strings.Repeat("c", 43)
	req := httptest.NewRequest(http.MethodPost, "/auth/desktop-session/complete", strings.NewReader("session=clerk-token&state="+state+"&code_challenge="+challenge))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")

	token, attempt, ok := parseDesktopLoopbackSession(req)
	if !ok || token != "clerk-token" || attempt.State != state || attempt.CodeChallenge != challenge {
		t.Fatalf("parseDesktopLoopbackSession = (%q, %+v, %v)", token, attempt, ok)
	}
}

func TestParseDesktopLoopbackSessionRejectsRemoteOrEmptyBindings(t *testing.T) {
	req := httptest.NewRequest(http.MethodPost, "/auth/desktop-session/complete", strings.NewReader("session=clerk-token&state=short&code_challenge="+strings.Repeat("c", 43)))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	if _, _, ok := parseDesktopLoopbackSession(req); ok {
		t.Fatal("short state accepted")
	}
}

func TestDesktopNativeCallbackURLStaysOnCustomProtocol(t *testing.T) {
	code := "pbd_" + strings.Repeat("a", 43)
	state := strings.Repeat("s", 43)
	got, err := desktopNativeCallbackURL("patchbay", code, state)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(got, "patchbay://auth/callback?") || !strings.Contains(got, "code="+code) {
		t.Fatalf("callback = %s", got)
	}
	if _, err := desktopNativeCallbackURL("https", code, state); err == nil {
		t.Fatal("https callback accepted")
	}
}

func TestCompleteDesktopLoopbackSessionRejectsInvalidForm(t *testing.T) {
	h := &Handler{}
	req := httptest.NewRequest(http.MethodPost, "/auth/desktop-session/complete", strings.NewReader("session=&state=x"))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	rec := httptest.NewRecorder()
	h.CompleteDesktopLoopbackSession(rec, req)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want 400: %s", rec.Code, rec.Body.String())
	}
}

func TestCompleteDesktopLoopbackSessionRequiresClerk(t *testing.T) {
	h := &Handler{}
	state := strings.Repeat("s", 43)
	challenge := strings.Repeat("c", 43)
	req := httptest.NewRequest(http.MethodPost, "/auth/desktop-session/complete", strings.NewReader("session=clerk-token&state="+state+"&code_challenge="+challenge))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	rec := httptest.NewRecorder()
	h.CompleteDesktopLoopbackSession(rec, req)
	if rec.Code != http.StatusServiceUnavailable {
		t.Fatalf("status = %d, want 503: %s", rec.Code, rec.Body.String())
	}
}

func TestCompleteDesktopLoopbackSessionMintsFromInitiateRow(t *testing.T) {
	if testHandler == nil {
		t.Skip("no database")
	}
	state := strings.Repeat("s", 43)
	challenge := strings.Repeat("c", 43)
	if err := testHandler.Queries.CreateDesktopAuthHandoff(context.Background(), db.CreateDesktopAuthHandoffParams{
		State:            state,
		CodeChallenge:    challenge,
		CallbackProtocol: desktopAuthCallbackProtocol,
	}); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_, _ = testPool.Exec(context.Background(), "DELETE FROM desktop_auth_handoff WHERE state = $1", state)
	})

	previous := testHandler.ClerkAuth
	t.Cleanup(func() { testHandler.ClerkAuth = previous })
	testHandler.ClerkAuth = clerkVerifierFunc(func(context.Context, string, time.Time) (ClerkIdentity, error) {
		return ClerkIdentity{Email: handlerTestEmail, Name: handlerTestName}, nil
	})

	req := httptest.NewRequest(http.MethodPost, "/auth/desktop-session/complete", strings.NewReader("session=clerk-token&state="+state+"&code_challenge="+challenge))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	rec := httptest.NewRecorder()
	testHandler.CompleteDesktopLoopbackSession(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200: %s", rec.Code, rec.Body.String())
	}
	if !strings.Contains(rec.Body.String(), "patchbay://auth/callback") {
		t.Fatalf("body = %s", rec.Body.String())
	}
}

func TestRequireFormalDesktopAuthActorAllowsFormalCredential(t *testing.T) {
	req := httptest.NewRequest(http.MethodPost, "/api/desktop-handoff/complete", nil)
	recorder := httptest.NewRecorder()

	if !requireFormalDesktopAuthActor(recorder, req) {
		t.Fatal("formal credential was rejected")
	}
	if recorder.Code != http.StatusOK {
		t.Fatalf("status = %d, want no response status", recorder.Code)
	}
}
