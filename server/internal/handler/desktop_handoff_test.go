package handler

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

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
