package slack

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"

	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

type fakeManagedTokenQueries struct {
	mu           sync.Mutex
	rows         []db.ChannelInstallation
	rotations    []db.RotateManagedSlackTokensParams
	observations []db.ObserveManagedSlackRuntimeParams
	loseRotation bool
}

func (q *fakeManagedTokenQueries) ListActiveManagedSlackInstallations(context.Context) ([]db.ChannelInstallation, error) {
	return q.rows, nil
}
func (q *fakeManagedTokenQueries) RotateManagedSlackTokens(_ context.Context, p db.RotateManagedSlackTokensParams) (int64, error) {
	q.mu.Lock()
	defer q.mu.Unlock()
	q.rotations = append(q.rotations, p)
	if q.loseRotation {
		return 0, nil
	}
	return 1, nil
}
func (q *fakeManagedTokenQueries) ObserveManagedSlackRuntime(_ context.Context, p db.ObserveManagedSlackRuntimeParams) (int64, error) {
	q.mu.Lock()
	defer q.mu.Unlock()
	q.observations = append(q.observations, p)
	return 1, nil
}

func tokenWorkerFixture(t *testing.T, q *fakeManagedTokenQueries, server *httptest.Server) (*ManagedTokenWorker, installConfig) {
	t.Helper()
	box := testBox(t)
	seal := func(value string) string {
		sealed, err := box.Seal([]byte(value))
		if err != nil {
			t.Fatal(err)
		}
		return base64.StdEncoding.EncodeToString(sealed)
	}
	now := time.Date(2026, time.September, 3, 12, 0, 0, 0, time.UTC)
	expiry := now.Add(29 * time.Minute)
	cfg := installConfig{AppID: "APP:TEAM", ApiAppID: "APP", TeamID: "TEAM", Transport: ManagedTransportWebhook,
		BotTokenEncrypted: seal("old-access"), RefreshTokenEncrypted: seal("old-refresh"), TokenExpiresAt: &expiry}
	oauth := &ManagedOAuthService{clientID: "client-id", clientSecret: "client-secret", httpClient: server.Client(), tokenURL: server.URL + "/oauth", now: func() time.Time { return now }}
	worker := newManagedTokenWorker(q, box, oauth, nil)
	worker.authTestURL = server.URL + "/auth"
	worker.now = oauth.now
	encoded, err := json.Marshal(cfg)
	if err != nil {
		t.Fatal(err)
	}
	q.rows = []db.ChannelInstallation{{ID: testUUID(31), Config: encoded, Status: "active"}}
	return worker, cfg
}

func TestManagedTokenWorkerRotatesThenProbesTheNewCredential(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		if r.URL.Path == "/oauth" {
			clientID, secret, ok := r.BasicAuth()
			if !ok || clientID != "client-id" || secret != "client-secret" {
				t.Error("refresh request must authenticate the managed client")
			}
			if err := r.ParseForm(); err != nil || r.Form.Get("grant_type") != "refresh_token" || r.Form.Get("refresh_token") != "old-refresh" {
				t.Error("refresh request has the wrong OAuth grant")
			}
			_, _ = w.Write([]byte(`{"ok":true,"access_token":"new-access","refresh_token":"new-refresh","expires_in":43200}`))
			return
		}
		if r.Header.Get("Authorization") != "Bearer new-access" {
			t.Error("health check must use the committed new access token")
		}
		_, _ = w.Write([]byte(`{"ok":true}`))
	}))
	defer server.Close()
	q := &fakeManagedTokenQueries{}
	worker, original := tokenWorkerFixture(t, q, server)
	if err := worker.Sweep(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(q.rotations) != 1 || len(q.observations) != 1 || q.observations[0].State != "healthy" {
		t.Fatalf("rotation/observation counts = %d/%d", len(q.rotations), len(q.observations))
	}
	rotation := q.rotations[0]
	access, err := decryptToken(rotation.BotTokenEncrypted, worker.box.Open)
	if err != nil || access != "new-access" {
		t.Fatal("new access token was not encrypted with the installation key")
	}
	refresh, err := decryptToken(rotation.RefreshTokenEncrypted, worker.box.Open)
	if err != nil || refresh != "new-refresh" {
		t.Fatal("new refresh token was not encrypted with the installation key")
	}
	if rotation.PreviousRefreshToken != original.RefreshTokenEncrypted || q.observations[0].ExpectedBotToken != rotation.BotTokenEncrypted {
		t.Fatal("rotation and health must be fenced by the credential generation they used")
	}
}

func TestManagedTokenWorkerSkipsHealthAfterLosingTheCredentialFence(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/oauth" {
			t.Error("a stale worker must not probe its old credential after a concurrent reconnect")
		}
		_, _ = w.Write([]byte(`{"ok":true,"access_token":"new-access","refresh_token":"new-refresh","expires_in":43200}`))
	}))
	defer server.Close()
	q := &fakeManagedTokenQueries{loseRotation: true}
	worker, _ := tokenWorkerFixture(t, q, server)
	if err := worker.Sweep(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(q.rotations) != 1 || len(q.observations) != 0 {
		t.Fatal("a stale rotation wrote a health observation")
	}
}

func TestManagedTokenWorkerRetainsCredentialOnRefreshFailure(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/oauth" {
			w.WriteHeader(http.StatusServiceUnavailable)
			_, _ = w.Write([]byte(`{"ok":true,"access_token":"bad","refresh_token":"bad","expires_in":1}`))
			return
		}
		if r.Header.Get("Authorization") != "Bearer old-access" {
			t.Error("failed refresh must leave the existing token intact")
		}
		_, _ = w.Write([]byte(`{"ok":false,"error":"token_revoked"}`))
	}))
	defer server.Close()
	q := &fakeManagedTokenQueries{}
	worker, _ := tokenWorkerFixture(t, q, server)
	if err := worker.Sweep(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(q.rotations) != 0 || len(q.observations) != 1 || q.observations[0].ErrorCode != "authentication_failed" {
		t.Fatal("failed rotation must preserve credentials and report the actual connection failure")
	}
}

func TestManagedTokenWorkerCancellationJoinsInFlightProbe(t *testing.T) {
	started := make(chan struct{})
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if err := r.ParseForm(); err != nil {
			t.Error("probe fixture could not read the request body")
		}
		close(started)
		<-r.Context().Done()
	}))
	defer server.Close()
	q := &fakeManagedTokenQueries{}
	worker, _ := tokenWorkerFixture(t, q, server)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go worker.Run(ctx)
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("worker did not start its initial sweep")
	}
	cancel()
	if !worker.WaitWithTimeout(3 * time.Second) {
		t.Fatal("cancelled worker left a provider request running")
	}
}

func TestManagedTokenRotationTimingAndSweepBudget(t *testing.T) {
	now := time.Date(2026, time.September, 3, 12, 0, 0, 0, time.UTC)
	for _, minutes := range []int{-1, 29, 30, 31} {
		expiry := now.Add(time.Duration(minutes) * time.Minute)
		if got := needsManagedTokenRotation(&expiry, now); got != (minutes <= 30) {
			t.Fatalf("expiry in %d minutes: rotate=%v", minutes, got)
		}
	}
	if needsManagedTokenRotation(nil, now) {
		t.Fatal("non-expiring installations must not rotate")
	}
	concurrency := managedHealthConcurrency(10_000)
	batches := (10_000 + concurrency - 1) / concurrency
	if time.Duration(batches)*managedInstallationTimeout > managedHealthSweepBudget {
		t.Fatal("large installation sweeps can exceed the runtime freshness budget")
	}
}

func TestManagedTokenWorkerRequiresManagedCredentials(t *testing.T) {
	q := &fakeManagedTokenQueries{}
	box := testBox(t)
	ready := &ManagedOAuthService{clientID: "client", clientSecret: "secret", httpClient: http.DefaultClient}
	if NewManagedTokenWorker(nil, box, ready, nil) != nil || newManagedTokenWorker(q, nil, ready, nil) != nil {
		t.Fatal("worker must be disabled without database or encryption key")
	}
	for _, oauth := range []*ManagedOAuthService{nil, {}, {clientID: "client"}, {clientSecret: "secret"}} {
		if newManagedTokenWorker(q, box, oauth, nil) != nil {
			t.Fatal("BYO-only deployments must not start a managed token worker")
		}
	}
}

func TestManagedTokenWorkerClassifiesProviderHealth(t *testing.T) {
	for _, tc := range []struct {
		name   string
		status int
		body   string
		state  string
		code   string
	}{
		{"healthy", 200, `{"ok":true}`, "healthy", ""},
		{"rejected", 200, `{"ok":false,"error":"invalid_auth"}`, "error", "authentication_failed"},
		{"unreadable", 200, `not json`, "degraded", "health_probe_invalid_response"},
		{"unavailable", 503, `{"ok":true}`, "degraded", "health_probe_failed"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.URL.Path != "/auth" || r.Header.Get("Authorization") != "Bearer old-access" {
					t.Error("non-expiring installation must only probe its current access token")
				}
				w.WriteHeader(tc.status)
				_, _ = w.Write([]byte(tc.body))
			}))
			defer server.Close()
			q := &fakeManagedTokenQueries{}
			worker, cfg := tokenWorkerFixture(t, q, server)
			cfg.RefreshTokenEncrypted, cfg.TokenExpiresAt = "", nil
			encoded, err := json.Marshal(cfg)
			if err != nil {
				t.Fatal(err)
			}
			q.rows[0].Config = encoded
			if err := worker.Sweep(context.Background()); err != nil {
				t.Fatal(err)
			}
			if len(q.rotations) != 0 || len(q.observations) != 1 || q.observations[0].State != tc.state || q.observations[0].ErrorCode != tc.code {
				t.Fatal("provider health did not preserve the expected state and error contract")
			}
		})
	}
}

func TestManagedTokenWorkerReportsMissingAndMalformedCredentials(t *testing.T) {
	for _, encrypted := range []string{"", "invalid-ciphertext"} {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			t.Error("unreadable credentials must not reach the provider")
		}))
		t.Cleanup(server.Close)
		q := &fakeManagedTokenQueries{}
		worker, cfg := tokenWorkerFixture(t, q, server)
		cfg.RefreshTokenEncrypted, cfg.BotTokenEncrypted, cfg.TokenExpiresAt = "", encrypted, nil
		encoded, err := json.Marshal(cfg)
		if err != nil {
			t.Fatal(err)
		}
		q.rows[0].Config = encoded
		if err := worker.Sweep(context.Background()); err != nil {
			t.Fatal(err)
		}
		server.Close()
		want := "credential_decryption_failed"
		if encrypted == "" {
			want = "credential_missing"
		}
		if len(q.observations) != 1 || q.observations[0].ErrorCode != want {
			t.Fatalf("credential failure was not classified as %s", want)
		}
	}
}
