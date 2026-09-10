package devseed

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"strings"
	"testing"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"
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

func TestSeedRuntimeAgentNameUsesProviderDisplayWithoutMachineSuffix(t *testing.T) {
	for provider, want := range map[string]string{
		"codex":      "Codex",
		"claude":     "Claude",
		"omp":        "Oh-My-Pi",
		"qoderclicn": "Qoder CN",
	} {
		if got := seedRuntimeAgentName(provider); got != want {
			t.Errorf("seedRuntimeAgentName(%q) = %q, want %q", provider, got, want)
		}
	}
	if got := seedRuntimeAgentName("Codex (Mac)"); got != "Codex (Mac)" {
		t.Fatalf("unknown provider should remain readable, got %q", got)
	}
}

func TestSeedRuntimeAgentsCleansLegacyFieldsIndependently(t *testing.T) {
	databaseURL := os.Getenv("DATABASE_URL")
	if databaseURL == "" {
		t.Skip("integration test requires DATABASE_URL")
	}
	ctx := context.Background()
	parsed, err := url.Parse(databaseURL)
	if err != nil {
		t.Skipf("skip unsafe database URL: %v", err)
	}
	if err := ValidateTarget(databaseURL, true); err != nil {
		t.Skipf("skip unsafe database URL: %v", err)
	}
	if !strings.Contains(strings.TrimPrefix(parsed.Path, "/"), "_test") {
		t.Skipf("integration test requires an isolated local *_test database")
	}
	adminPool, err := pgxpool.New(ctx, databaseURL)
	if err != nil {
		t.Fatalf("open admin pool: %v", err)
	}
	defer adminPool.Close()

	schema := "devseed_test_" + strings.ReplaceAll(uuid.NewString(), "-", "")
	if _, err := adminPool.Exec(ctx, "CREATE SCHEMA "+schema); err != nil {
		t.Fatalf("create test schema: %v", err)
	}
	defer adminPool.Exec(ctx, "DROP SCHEMA "+schema+" CASCADE")

	query := parsed.Query()
	query.Set("options", "-csearch_path="+schema)
	parsed.RawQuery = query.Encode()
	testPool, err := pgxpool.New(ctx, parsed.String())
	if err != nil {
		t.Fatalf("open schema pool: %v", err)
	}
	defer testPool.Close()
	if err := testPool.Ping(ctx); err != nil {
		t.Fatalf("ping schema pool: %v", err)
	}
	if _, err := testPool.Exec(ctx, `
		CREATE TABLE "user" (id uuid PRIMARY KEY, email text NOT NULL);
		CREATE TABLE workspace (id uuid PRIMARY KEY);
		CREATE TABLE agent_runtime (
			id uuid PRIMARY KEY, workspace_id uuid NOT NULL, provider text NOT NULL,
			name text NOT NULL, runtime_mode text NOT NULL, status text NOT NULL,
			daemon_id text, owner_id uuid NOT NULL
		);
		CREATE TABLE agent (
			id uuid PRIMARY KEY, workspace_id uuid NOT NULL, name text NOT NULL,
			description text NOT NULL, runtime_mode text NOT NULL, runtime_config jsonb NOT NULL,
			runtime_id uuid NOT NULL, visibility text NOT NULL, permission_mode text NOT NULL,
			max_concurrent_tasks integer NOT NULL, owner_id uuid NOT NULL, status text NOT NULL
		)
	`); err != nil {
		t.Fatalf("create fixture tables: %v", err)
	}

	workspaceID := fixtureID("workspace")
	ownerID := uuid.NewString()
	email := "seed-fields-" + uuid.NewString() + "@localhost"
	if _, err := testPool.Exec(ctx, `INSERT INTO "user" (id, email) VALUES ($1, $2)`, ownerID, email); err != nil {
		t.Fatalf("insert owner: %v", err)
	}
	if _, err := testPool.Exec(ctx, `INSERT INTO workspace (id) VALUES ($1)`, workspaceID); err != nil {
		t.Fatalf("insert workspace: %v", err)
	}

	type runtimeFixture struct {
		id, provider, runtimeName, agentName, description string
	}
	runtimes := []runtimeFixture{
		{provider: "codex", runtimeName: "Codex (Mac)", agentName: "Codex", description: legacyRuntimeAgentDescription},
		{provider: "claude", runtimeName: "Claude (Mac)", agentName: "Claude (Mac)", description: "User description"},
		{provider: "qwen", runtimeName: "QwenPaw (Mac)", agentName: "QwenPaw (Mac)", description: legacyRuntimeAgentDescription},
		{provider: "grok", runtimeName: "Grok (Mac)", agentName: "User agent", description: "User description"},
	}
	for i := range runtimes {
		runtimes[i].id = uuid.NewString()
		fixture := runtimes[i]
		if _, err := testPool.Exec(ctx, `
			INSERT INTO agent_runtime (id, workspace_id, provider, name, runtime_mode, status, daemon_id, owner_id)
			VALUES ($1, $2, $3, $4, 'local', 'online', $5, $6)
		`, fixture.id, workspaceID, fixture.provider, fixture.runtimeName, fmt.Sprintf("daemon-%d", i), ownerID); err != nil {
			t.Fatalf("insert runtime fixture %d: %v", i, err)
		}
		if _, err := testPool.Exec(ctx, `
			INSERT INTO agent (id, workspace_id, name, description, runtime_mode, runtime_config, runtime_id, visibility, permission_mode, max_concurrent_tasks, owner_id, status)
			VALUES ($1, $2, $3, $4, 'local', '{}'::jsonb, $5, 'private', 'private', 1, $6, 'idle')
		`, fixtureID("agent/runtime/"+fixture.id), workspaceID, fixture.agentName, fixture.description, fixture.id, ownerID); err != nil {
			t.Fatalf("insert agent fixture %d: %v", i, err)
		}
	}

	if inserted, err := SeedRuntimeAgents(ctx, testPool, email); err != nil {
		t.Fatalf("seed runtime agents: %v", err)
	} else if inserted != 0 {
		t.Fatalf("seed inserted %d existing agents", inserted)
	}

	want := []struct {
		provider, name, description string
	}{
		{provider: "codex", name: "Codex", description: ""},
		{provider: "claude", name: "Claude", description: "User description"},
		{provider: "qwen", name: "Qwen Code", description: ""},
		{provider: "grok", name: "User agent", description: "User description"},
	}
	for i, fixture := range runtimes {
		var gotName, gotDescription string
		if err := testPool.QueryRow(ctx, `SELECT name, description FROM agent WHERE id = $1`, fixtureID("agent/runtime/"+fixture.id)).Scan(&gotName, &gotDescription); err != nil {
			t.Fatalf("load agent %d: %v", i, err)
		}
		if gotName != want[i].name || gotDescription != want[i].description {
			t.Fatalf("runtime agent %d = %q/%q, want %q/%q", i, gotName, gotDescription, want[i].name, want[i].description)
		}
	}
}
