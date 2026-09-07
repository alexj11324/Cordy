package auth

import (
	"context"
	"errors"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

type guestIdentityFixture struct {
	session db.GuestSession
	user    db.User
	err     error
	calls   int
	hash    string
}

func (f *guestIdentityFixture) GetGuestSessionByTokenHash(_ context.Context, hash string) (db.GuestSession, error) {
	f.calls++
	f.hash = hash
	return f.session, f.err
}
func (f *guestIdentityFixture) GetUser(context.Context, pgtype.UUID) (db.User, error) {
	return f.user, nil
}
func TestResolveGuestUserChecksLiveIdentity(t *testing.T) {
	token := GuestTokenPrefix + strings.Repeat("a", 40)
	id := pgtype.UUID{Bytes: [16]byte{1}, Valid: true}
	for _, tc := range []struct {
		name, status   string
		guest, validID bool
		lookupErr      error
		wantInvalid    bool
	}{
		{"active", "active", true, true, nil, false},
		{"revoked", "revoked", true, true, nil, true},
		{"claimed", "claimed", true, true, nil, true},
		{"formal account", "active", false, true, nil, true},
		{"missing identity", "active", true, false, nil, true},
		{"missing session", "active", true, true, pgx.ErrNoRows, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := &guestIdentityFixture{session: db.GuestSession{ID: id, UserID: id, Status: tc.status}, user: db.User{ID: id, IsGuest: tc.guest}, err: tc.lookupErr}
			f.user.ID.Valid = tc.validID
			_, err := ResolveGuestUser(context.Background(), f, token)
			if errors.Is(err, ErrInvalidGuestToken) != tc.wantInvalid {
				t.Fatalf("error=%v, invalid=%v", err, tc.wantInvalid)
			}
			if !tc.wantInvalid && err != nil {
				t.Fatal(err)
			}
			if f.hash != HashToken(token) {
				t.Fatal("lookup did not use token hash")
			}
		})
	}
	f := &guestIdentityFixture{session: db.GuestSession{ID: id, UserID: id, Status: GuestSessionActive}, user: db.User{ID: id, IsGuest: true}}
	if _, err := ResolveGuestUser(context.Background(), f, token); err != nil {
		t.Fatal(err)
	}
	f.session.Status = "revoked"
	if _, err := ResolveGuestUser(context.Background(), f, token); !errors.Is(err, ErrInvalidGuestToken) {
		t.Fatalf("revocation was cached: %v", err)
	}
	if f.calls != 2 {
		t.Fatalf("lookup count=%d", f.calls)
	}
}
func TestResolveGuestUserRejectsMalformedBeforeLookup(t *testing.T) {
	f := &guestIdentityFixture{}
	for _, raw := range []string{"", "pbg_bad", "pbg_" + strings.Repeat("g", 40), "pby_" + strings.Repeat("a", 40)} {
		if _, err := ResolveGuestUser(context.Background(), f, raw); !errors.Is(err, ErrInvalidGuestToken) {
			t.Fatalf("malformed token accepted: %v", err)
		}
	}
	if f.calls != 0 {
		t.Fatal("malformed token reached database")
	}
}
func TestResolveGuestUserPreservesStoreFailure(t *testing.T) {
	failure := errors.New("database unavailable")
	f := &guestIdentityFixture{err: failure}
	_, err := ResolveGuestUser(context.Background(), f, GuestTokenPrefix+strings.Repeat("a", 40))
	if !errors.Is(err, failure) || errors.Is(err, ErrInvalidGuestToken) {
		t.Fatalf("store failure misclassified: %v", err)
	}
}
