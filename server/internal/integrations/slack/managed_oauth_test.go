package slack

import (
	"context"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"

	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// memOAuthStore is an in-memory managedOAuthQueries fake with the same
// single-use + expiry semantics as the SQL: Consume deletes only unexpired
// rows, Purge drops expired ones.
type memOAuthStore struct {
	mu   sync.Mutex
	rows map[string]db.SlackOauthState
	now  func() time.Time
}

func newMemOAuthStore(now func() time.Time) *memOAuthStore {
	return &memOAuthStore{rows: map[string]db.SlackOauthState{}, now: now}
}

func (m *memOAuthStore) CreateSlackOAuthState(_ context.Context, arg db.CreateSlackOAuthStateParams) (db.SlackOauthState, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	row := db.SlackOauthState{
		StateHash:       arg.StateHash,
		WorkspaceID:     arg.WorkspaceID,
		InstallerUserID: arg.InstallerUserID,
		RedirectUrl:     arg.RedirectUrl,
		ExpiresAt:       arg.ExpiresAt,
	}
	m.rows[string(arg.StateHash)] = row
	return row, nil
}

func (m *memOAuthStore) ConsumeSlackOAuthState(_ context.Context, stateHash []byte) (db.SlackOauthState, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	row, ok := m.rows[string(stateHash)]
	if !ok {
		return db.SlackOauthState{}, pgx.ErrNoRows
	}
	if !row.ExpiresAt.Valid || !row.ExpiresAt.Time.After(m.now()) {
		return db.SlackOauthState{}, pgx.ErrNoRows
	}
	delete(m.rows, string(stateHash))
	return row, nil
}

func (m *memOAuthStore) PurgeExpiredSlackOAuthStates(_ context.Context, _ pgtype.Timestamptz) error {
	return nil
}

func testUUID(b byte) pgtype.UUID {
	var u pgtype.UUID
	u.Bytes[0] = b
	u.Valid = true
	return u
}

func TestManagedOAuth_BeginConsumeRoundtrip(t *testing.T) {
	now := time.Now()
	store := newMemOAuthStore(func() time.Time { return now })
	svc := &ManagedOAuthService{q: store, httpClient: http.DefaultClient, tokenURL: DefaultSlackOAuthTokenURL, now: func() time.Time { return now }}
	wsID, userID := testUUID(1), testUUID(2)
	state, expires, err := svc.BeginInstall(context.Background(), wsID, userID, "https://app.example.test/installed")
	if err != nil {
		t.Fatalf("begin: %v", err)
	}
	if state == "" || !expires.After(now) {
		t.Fatalf("begin returned no usable state (state=%q expires=%v)", state, expires)
	}
	row, err := svc.ConsumeState(context.Background(), state)
	if err != nil {
		t.Fatalf("consume: %v", err)
	}
	if row.WorkspaceID != wsID || row.InstallerUserID != userID {
		t.Fatalf("consumed wrong authorization: %+v", row)
	}
	// Single-use: the replay finds nothing and renders the restart answer.
	if _, err := svc.ConsumeState(context.Background(), state); err == nil {
		t.Fatal("replayed state must be refused")
	} else if err != ErrInvalidOAuthState {
		t.Fatalf("replay error = %v, want ErrInvalidOAuthState", err)
	}
}

func TestManagedOAuth_UnknownAndEmptyStateRefused(t *testing.T) {
	now := time.Now()
	store := newMemOAuthStore(func() time.Time { return now })
	svc := &ManagedOAuthService{q: store, now: func() time.Time { return now }}
	for _, raw := range []string{"", "   ", "not-a-state"} {
		if _, err := svc.ConsumeState(context.Background(), raw); err != ErrInvalidOAuthState {
			t.Fatalf("state %q: err = %v, want ErrInvalidOAuthState", raw, err)
		}
	}
}

func TestManagedOAuth_ExpiredStateRefused(t *testing.T) {
	now := time.Now()
	store := newMemOAuthStore(func() time.Time { return now })
	svc := &ManagedOAuthService{q: store, now: func() time.Time { return now }}
	state, _, err := svc.BeginInstall(context.Background(), testUUID(1), testUUID(2), "https://app.example.test/installed")
	if err != nil {
		t.Fatalf("begin: %v", err)
	}
	// Advance past the ten-minute TTL: the row is still stored but unclaimable.
	store.now = func() time.Time { return now.Add(ManagedOAuthStateTTL + time.Minute) }
	svc.now = store.now
	if _, err := svc.ConsumeState(context.Background(), state); err != ErrInvalidOAuthState {
		t.Fatalf("expired consume err = %v, want ErrInvalidOAuthState", err)
	}
}

func TestManagedOAuth_InvalidRedirectRefused(t *testing.T) {
	now := time.Now()
	svc := &ManagedOAuthService{q: newMemOAuthStore(func() time.Time { return now }), now: func() time.Time { return now }}
	for _, redirect := range []string{"", "not-a-url", "ftp://files.example.test/x"} {
		if _, _, err := svc.BeginInstall(context.Background(), testUUID(1), testUUID(2), redirect); err == nil {
			t.Fatalf("redirect %q must be refused", redirect)
		}
	}
}

func TestManagedOAuth_ExchangeCode(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if err := r.ParseForm(); err != nil {
			t.Errorf("parse form: %v", err)
		}
		if r.Form.Get("code") == "bad-code" {
			_, _ = w.Write([]byte(`{"ok":false,"error":"invalid_code"}`))
			return
		}
		_, _ = w.Write([]byte(`{"ok":true,"access_token":"xoxb-managed","app_id":"A123","bot_user_id":"UBOT","team":{"id":"T1"},"authed_user":{"id":"U9"}}`))
	}))
	defer server.Close()
	svc := &ManagedOAuthService{httpClient: server.Client(), tokenURL: server.URL, clientID: "cid", clientSecret: "csec"}
	got, err := svc.ExchangeCode(context.Background(), "good-code", "https://app.example.test/callback")
	if err != nil {
		t.Fatalf("exchange: %v", err)
	}
	if got.BotToken != "xoxb-managed" || got.TeamID != "T1" || got.AppID != "A123" {
		t.Fatalf("unexpected access: %+v", got)
	}
	if _, err := svc.ExchangeCode(context.Background(), "bad-code", "https://app.example.test/callback"); err == nil {
		t.Fatal("refused exchange must be an error")
	}
	unconfigured := &ManagedOAuthService{httpClient: server.Client(), tokenURL: server.URL}
	if _, err := unconfigured.ExchangeCode(context.Background(), "good-code", "https://app.example.test/callback"); err == nil {
		t.Fatal("exchange without credentials must fail loudly, not half-work")
	}
}

func TestManagedOAuth_RequiresQueries(t *testing.T) {
	if _, err := NewManagedOAuthService(ManagedOAuthConfig{}); err == nil {
		t.Fatal("nil queries must be refused")
	}
}
