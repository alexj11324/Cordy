package middleware

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"sync"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/auth"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

type guestAuthTestRow struct {
	values []any
	err    error
}

func (r guestAuthTestRow) Scan(dest ...any) error {
	if r.err != nil {
		return r.err
	}
	if len(dest) != len(r.values) {
		return fmt.Errorf("scan received %d destinations for %d values", len(dest), len(r.values))
	}
	for i, dst := range dest {
		target := reflect.ValueOf(dst)
		if target.Kind() != reflect.Pointer || target.IsNil() {
			return fmt.Errorf("scan destination %d is not a non-nil pointer", i)
		}
		value := reflect.ValueOf(r.values[i])
		if !value.IsValid() {
			target.Elem().Set(reflect.Zero(target.Elem().Type()))
			continue
		}
		if !value.Type().AssignableTo(target.Elem().Type()) {
			return fmt.Errorf("scan value %d has type %s, destination expects %s", i, value.Type(), target.Elem().Type())
		}
		target.Elem().Set(value)
	}
	return nil
}

type guestAuthTestDB struct {
	session     db.GuestSession
	user        db.User
	revoked     db.GuestSession
	sessionErr  error
	userErr     error
	revokeErr   error
	queryCount  int
	revokeCount int
}

type guestLogoutRaceDB struct {
	mu          sync.Mutex
	session     db.GuestSession
	revokeCount int
}

func (d *guestLogoutRaceDB) Exec(_ context.Context, _ string, _ ...interface{}) (pgconn.CommandTag, error) {
	return pgconn.NewCommandTag(""), nil
}

func (d *guestLogoutRaceDB) Query(_ context.Context, _ string, _ ...interface{}) (pgx.Rows, error) {
	return nil, errors.New("unexpected Query call")
}

func (d *guestLogoutRaceDB) QueryRow(_ context.Context, query string, _ ...interface{}) pgx.Row {
	d.mu.Lock()
	defer d.mu.Unlock()

	switch {
	case strings.Contains(query, "UPDATE guest_session"):
		if d.session.Status != guestSessionActive {
			return guestAuthTestRow{err: pgx.ErrNoRows}
		}
		d.session.Status = "revoked"
		d.revokeCount++
		return guestAuthTestRow{values: guestSessionScanValues(d.session)}
	case strings.Contains(query, "FROM guest_session"):
		return guestAuthTestRow{values: guestSessionScanValues(d.session)}
	default:
		return guestAuthTestRow{err: fmt.Errorf("unexpected QueryRow: %s", query)}
	}
}

func (d *guestAuthTestDB) Exec(_ context.Context, _ string, _ ...interface{}) (pgconn.CommandTag, error) {
	return pgconn.NewCommandTag(""), nil
}

func (d *guestAuthTestDB) Query(_ context.Context, _ string, _ ...interface{}) (pgx.Rows, error) {
	return nil, errors.New("unexpected Query call")
}

func (d *guestAuthTestDB) QueryRow(_ context.Context, query string, _ ...interface{}) pgx.Row {
	d.queryCount++
	switch {
	case strings.Contains(query, "UPDATE guest_session"):
		d.revokeCount++
		if d.revokeErr != nil {
			return guestAuthTestRow{err: d.revokeErr}
		}
		return guestAuthTestRow{values: guestSessionScanValues(d.revoked)}
	case strings.Contains(query, "FROM guest_session"):
		return guestAuthTestRow{values: guestSessionScanValues(d.session), err: d.sessionErr}
	case strings.Contains(query, `FROM "user"`):
		return guestAuthTestRow{values: userScanValues(d.user), err: d.userErr}
	default:
		return guestAuthTestRow{err: fmt.Errorf("unexpected QueryRow: %s", query)}
	}
}

func guestSessionScanValues(session db.GuestSession) []any {
	return []any{
		session.ID,
		session.UserID,
		session.TokenHash,
		session.Status,
		session.CreatedAt,
		session.ClaimedAt,
		session.ClaimedBy,
	}
}

func userScanValues(user db.User) []any {
	return []any{
		user.ID,
		user.Name,
		user.Email,
		user.AvatarUrl,
		user.CreatedAt,
		user.UpdatedAt,
		user.OnboardedAt,
		user.OnboardingQuestionnaire,
		user.CloudWaitlistEmail,
		user.CloudWaitlistReason,
		user.StarterContentState,
		user.Language,
		user.ProfileDescription,
		user.Timezone,
		user.IsGuest,
	}
}

func guestAuthTestToken() string {
	return guestTokenPrefix + strings.Repeat("a", guestTokenHexLength)
}

func guestAuthTestUser(isGuest bool) db.User {
	return db.User{
		ID:      pgtype.UUID{Bytes: [16]byte{1}, Valid: true},
		Name:    "Guest",
		Email:   "guest@example.invalid",
		IsGuest: isGuest,
	}
}

func guestAuthTestSession(token string, status string) db.GuestSession {
	user := guestAuthTestUser(true)
	return db.GuestSession{
		ID:        pgtype.UUID{Bytes: [16]byte{2}, Valid: true},
		UserID:    user.ID,
		TokenHash: auth.HashToken(token),
		Status:    status,
	}
}

func assertGuestErrorDoesNotLeakSecret(t *testing.T, body, token string) {
	t.Helper()
	for _, secret := range []string{token, auth.HashToken(token), "token_hash"} {
		if strings.Contains(body, secret) {
			t.Fatalf("guest error response leaked %q: %s", secret, body)
		}
	}
}

func TestAuth_GuestBearerAuthenticatesActiveGuest(t *testing.T) {
	token := guestAuthTestToken()
	user := guestAuthTestUser(true)
	session := guestAuthTestSession(token, guestSessionActive)
	store := &guestAuthTestDB{session: session, user: user, revoked: session}

	var gotUserID, gotEmail, gotGuest, gotAgent, gotTask, gotWorkspace, gotActor string
	handler := Auth(db.New(store), nil, nil)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotUserID = r.Header.Get("X-User-ID")
		gotEmail = r.Header.Get("X-User-Email")
		gotGuest = r.Header.Get("X-Guest-User")
		gotAgent = r.Header.Get("X-Agent-ID")
		gotTask = r.Header.Get("X-Task-ID")
		gotWorkspace = r.Header.Get("X-Workspace-ID")
		gotActor = r.Header.Get("X-Actor-Source")
		w.WriteHeader(http.StatusNoContent)
	}))

	req := httptest.NewRequest(http.MethodGet, "/api/me", nil)
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("X-Guest-User", "false")
	req.Header.Set("X-Agent-ID", "forged-agent")
	req.Header.Set("X-Task-ID", "forged-task")
	req.Header.Set("X-Workspace-ID", "forged-workspace")
	req.Header.Set("X-Actor-Source", "task_token")
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)

	if w.Code != http.StatusNoContent {
		t.Fatalf("expected 204, got %d: %s", w.Code, w.Body.String())
	}
	if gotUserID != uuidToString(user.ID) {
		t.Fatalf("X-User-ID = %q, want %q", gotUserID, uuidToString(user.ID))
	}
	if gotEmail != user.Email || gotGuest != "true" {
		t.Fatalf("guest identity headers = email %q, guest %q", gotEmail, gotGuest)
	}
	if gotAgent != "" || gotTask != "" || gotWorkspace != "" || gotActor != "" {
		t.Fatalf("guest bearer retained forged identity headers: agent=%q task=%q workspace=%q actor=%q", gotAgent, gotTask, gotWorkspace, gotActor)
	}
	if store.queryCount != 2 {
		t.Fatalf("guest bearer made %d database lookups, want session and owner", store.queryCount)
	}
}

func TestAuth_GuestBearerRejectsMalformedAndUnavailable(t *testing.T) {
	tests := []struct {
		name    string
		token   string
		queries *db.Queries
		want    int
	}{
		{
			name:  "malformed pbg token",
			token: guestTokenPrefix + "not-hex",
			want:  http.StatusUnauthorized,
		},
		{
			name:  "storage unavailable",
			token: guestAuthTestToken(),
			want:  http.StatusServiceUnavailable,
		},
		{
			name:    "session lookup unavailable",
			token:   guestAuthTestToken(),
			queries: db.New(&guestAuthTestDB{sessionErr: errors.New("database unavailable")}),
			want:    http.StatusServiceUnavailable,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var nextCalled bool
			handler := Auth(tt.queries, nil, nil)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				nextCalled = true
				w.WriteHeader(http.StatusNoContent)
			}))
			req := httptest.NewRequest(http.MethodGet, "/api/me", nil)
			req.Header.Set("Authorization", "Bearer "+tt.token)
			w := httptest.NewRecorder()
			handler.ServeHTTP(w, req)
			if w.Code != tt.want {
				t.Fatalf("status = %d, want %d: %s", w.Code, tt.want, w.Body.String())
			}
			assertGuestErrorDoesNotLeakSecret(t, w.Body.String(), tt.token)
			if nextCalled {
				t.Fatal("guest auth failure reached downstream handler")
			}
		})
	}
}

func TestAuth_GuestBearerRejectsTerminalOrFormalOwner(t *testing.T) {
	tests := []struct {
		name       string
		status     string
		ownerGuest bool
	}{
		{name: "revoked session", status: "revoked", ownerGuest: true},
		{name: "claimed session", status: "claimed", ownerGuest: true},
		{name: "formal owner", status: guestSessionActive, ownerGuest: false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			token := guestAuthTestToken()
			session := guestAuthTestSession(token, tt.status)
			store := &guestAuthTestDB{
				session: session,
				user:    guestAuthTestUser(tt.ownerGuest),
			}
			handler := Auth(db.New(store), nil, nil)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				t.Fatal("terminal or formal-owner guest bearer reached downstream handler")
			}))
			req := httptest.NewRequest(http.MethodGet, "/api/me", nil)
			req.Header.Set("Authorization", "Bearer "+token)
			w := httptest.NewRecorder()
			handler.ServeHTTP(w, req)
			if w.Code != http.StatusUnauthorized {
				t.Fatalf("status = %d, want 401: %s", w.Code, w.Body.String())
			}
		})
	}
}

func TestAuth_NonGuestJWTCannotForgeGuestMarker(t *testing.T) {
	token := generateToken(validClaims(), auth.JWTSecret())
	var gotGuest string
	handler := Auth(nil, nil, nil)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotGuest = r.Header.Get("X-Guest-User")
		w.WriteHeader(http.StatusNoContent)
	}))
	req := httptest.NewRequest(http.MethodGet, "/api/me", nil)
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("X-Guest-User", "true")
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)

	if w.Code != http.StatusNoContent {
		t.Fatalf("expected 204, got %d: %s", w.Code, w.Body.String())
	}
	if gotGuest != "" {
		t.Fatalf("ordinary JWT retained forged X-Guest-User=%q", gotGuest)
	}
}

func TestRevokeGuestOnLogoutConsumesPresentedBearer(t *testing.T) {
	token := guestAuthTestToken()
	session := guestAuthTestSession(token, guestSessionActive)
	revoked := session
	revoked.Status = "revoked"
	store := &guestAuthTestDB{session: session, revoked: revoked}

	var nextCalled bool
	handler := RevokeGuestOnLogout(db.New(store))(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		nextCalled = true
		w.WriteHeader(http.StatusOK)
	}))
	req := httptest.NewRequest(http.MethodPost, "/auth/logout", nil)
	req.Header.Set("Authorization", "Bearer "+token)
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK || !nextCalled {
		t.Fatalf("logout status=%d nextCalled=%v, want 200 and downstream", w.Code, nextCalled)
	}
	if store.revokeCount != 1 {
		t.Fatalf("logout issued %d revoke mutations, want 1", store.revokeCount)
	}
}

func TestRevokeGuestOnLogoutIsIdempotentAndFailClosed(t *testing.T) {
	token := guestAuthTestToken()

	t.Run("terminal session passes through without mutation", func(t *testing.T) {
		session := guestAuthTestSession(token, "revoked")
		store := &guestAuthTestDB{session: session}
		handler := RevokeGuestOnLogout(db.New(store))(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(http.StatusOK)
		}))
		req := httptest.NewRequest(http.MethodPost, "/auth/logout", nil)
		req.Header.Set("Authorization", "Bearer "+token)
		w := httptest.NewRecorder()
		handler.ServeHTTP(w, req)
		if w.Code != http.StatusOK || store.revokeCount != 0 {
			t.Fatalf("status=%d revokeCount=%d, want 200 and no mutation", w.Code, store.revokeCount)
		}
	})

	t.Run("missing query dependency returns 503", func(t *testing.T) {
		handler := RevokeGuestOnLogout(nil)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			t.Fatal("unavailable logout reached downstream handler")
		}))
		req := httptest.NewRequest(http.MethodPost, "/auth/logout", nil)
		req.Header.Set("Authorization", "Bearer "+token)
		w := httptest.NewRecorder()
		handler.ServeHTTP(w, req)
		if w.Code != http.StatusServiceUnavailable {
			t.Fatalf("status = %d, want 503", w.Code)
		}
		assertGuestErrorDoesNotLeakSecret(t, w.Body.String(), token)
	})

	t.Run("unknown token remains idempotent", func(t *testing.T) {
		store := &guestAuthTestDB{sessionErr: pgx.ErrNoRows}
		handler := RevokeGuestOnLogout(db.New(store))(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(http.StatusOK)
		}))
		req := httptest.NewRequest(http.MethodPost, "/auth/logout", nil)
		req.Header.Set("Authorization", "Bearer "+token)
		w := httptest.NewRecorder()
		handler.ServeHTTP(w, req)
		if w.Code != http.StatusOK {
			t.Fatalf("status = %d, want 200", w.Code)
		}
	})
}

func TestRevokeGuestOnLogoutConcurrentRequestsAreIdempotent(t *testing.T) {
	token := guestAuthTestToken()
	store := &guestLogoutRaceDB{session: guestAuthTestSession(token, guestSessionActive)}
	handler := RevokeGuestOnLogout(db.New(store))(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))

	const requests = 2
	statuses := make(chan int, requests)
	var wg sync.WaitGroup
	for range requests {
		wg.Add(1)
		go func() {
			defer wg.Done()
			req := httptest.NewRequest(http.MethodPost, "/auth/logout", nil)
			req.Header.Set("Authorization", "Bearer "+token)
			w := httptest.NewRecorder()
			handler.ServeHTTP(w, req)
			statuses <- w.Code
		}()
	}
	wg.Wait()
	close(statuses)

	for status := range statuses {
		if status != http.StatusOK {
			t.Fatalf("concurrent logout status = %d, want 200", status)
		}
	}
	store.mu.Lock()
	revokeCount := store.revokeCount
	store.mu.Unlock()
	if revokeCount != 1 {
		t.Fatalf("concurrent logout committed %d revocations, want exactly 1", revokeCount)
	}
}
