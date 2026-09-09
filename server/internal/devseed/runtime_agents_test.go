package devseed

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestWakeRuntimeRegistrationPreservesWorkspaceName(t *testing.T) {
	var patches int
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/auth/dev-login":
			if r.Method != http.MethodPost {
				t.Error("login must POST")
			}
			var body map[string]string
			_ = json.NewDecoder(r.Body).Decode(&body)
			if body["email"] != "person@localhost" || body["onboarding"] != "keep" {
				t.Errorf("login input %v", body)
			}
			_, _ = w.Write([]byte(`{"token":"test-only-token"}`))
		case "/api/workspaces/" + fixtureID("workspace"):
			patches++
			if r.Method != http.MethodPatch || r.Header.Get("Authorization") != "Bearer test-only-token" {
				t.Error("missing authenticated patch")
			}
			var body map[string]string
			_ = json.NewDecoder(r.Body).Decode(&body)
			if len(body) != 1 || body["name"] != "User renamed workspace" {
				t.Errorf("must preserve current name, got %v", body)
			}
			w.WriteHeader(http.StatusOK)
		default:
			t.Errorf("unexpected path %s", r.URL.Path)
			w.WriteHeader(http.StatusNotFound)
		}
	}))
	defer server.Close()
	if err := wakeRuntimeRegistration(context.Background(), server.URL, "person@localhost", "User renamed workspace"); err != nil {
		t.Fatal(err)
	}
	if patches != 1 {
		t.Fatalf("patches = %d", patches)
	}
}

func TestWakeRuntimeRegistrationRejectsRemoteAPI(t *testing.T) {
	if err := wakeRuntimeRegistration(context.Background(), "https://example.com", "dev@localhost", "Fixture"); err == nil {
		t.Fatal("remote API accepted")
	}
}

func TestRuntimeRegistrationWaitsForEveryInstalledHarness(t *testing.T) {
	expected := []RuntimeIdentity{{"device-a", "codex", ""}, {"device-a", "claude", ""}, {"device-b", "codex", ""}}
	for _, partial := range [][]RuntimeIdentity{nil, expected[:1], expected[:2], {{"device-a", "codex", ""}, {"device-a", "claude", ""}, {"device-c", "codex", ""}}} {
		if RuntimeRegistrationComplete(expected, partial) {
			t.Fatalf("partial registration accepted: %v", partial)
		}
	}
	if !RuntimeRegistrationComplete(expected, expected) {
		t.Fatal("complete registration rejected")
	}
	if RuntimeRegistrationComplete(nil, expected) {
		t.Fatal("unknown installed set accepted")
	}
}

func TestRuntimeRegistrationKeepsProfilesDistinct(t *testing.T) {
	expected := []RuntimeIdentity{{"device-a", "codex", "profile-1"}, {"device-a", "codex", "profile-2"}}
	if RuntimeRegistrationComplete(expected, expected[:1]) {
		t.Fatal("second profile missing but registration accepted")
	}
	if !RuntimeRegistrationComplete(expected, expected) {
		t.Fatal("both profiles rejected")
	}
}
