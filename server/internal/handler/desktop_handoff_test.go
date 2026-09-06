package handler

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
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

func TestDesktopHandoffInitiateAcceptsWorktreeCallbackProtocols(t *testing.T) {
	binding := desktopAuthHandoffRequest{
		State:         strings.Repeat("s", 43),
		CodeChallenge: strings.Repeat("c", 43),
	}
	for _, protocol := range []string{"patchbay", "patchbay-canary-5718c47b86bf9ece"} {
		binding.CallbackProtocol = protocol
		if !validateDesktopHandoffInitiate(binding) {
			t.Fatalf("owned protocol %q rejected", protocol)
		}
	}
	for _, protocol := range []string{"evil-app", "patchbay-canary", "patchbay-canary-01zp-25"} {
		binding.CallbackProtocol = protocol
		if validateDesktopHandoffInitiate(binding) {
			t.Fatalf("unowned protocol %q accepted", protocol)
		}
	}
}

func TestDesktopHandoffCompleteIgnoresBrowserSelectedProtocol(t *testing.T) {
	req := desktopAuthHandoffRequest{
		State:            strings.Repeat("s", 43),
		CodeChallenge:    strings.Repeat("c", 43),
		CallbackProtocol: "evil-app",
	}
	if !validateDesktopHandoffComplete(req) {
		t.Fatal("complete rejected a valid PKCE binding because of callback_protocol")
	}
	req.State = "short"
	if validateDesktopHandoffComplete(req) {
		t.Fatal("complete accepted a malformed PKCE binding")
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
