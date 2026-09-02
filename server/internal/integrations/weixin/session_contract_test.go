package weixin

import (
	"context"
	"errors"
	"strconv"
	"testing"
	"time"
)

func TestMemoryInstallSessionStoreTrimsIDsExpiresAndEnforcesCap(t *testing.T) {
	store, ok := NewMemorySessionStore().(*memorySessionStore)
	if !ok {
		t.Fatal("NewMemorySessionStore returned an unexpected implementation")
	}
	if err := store.Put(t.Context(), InstallSession{ID: "expired", ExpiresAt: time.Now().Add(-time.Second)}); err != nil {
		t.Fatal(err)
	}
	if _, err := store.Get(t.Context(), " expired "); !errors.Is(err, ErrInstallSessionNotFound) {
		t.Fatalf("expired session error = %v", err)
	}

	expires := time.Now().Add(time.Minute)
	if err := store.Put(t.Context(), InstallSession{ID: " session ", ExpiresAt: expires}); err != nil {
		t.Fatal(err)
	}
	got, err := store.Get(t.Context(), "session")
	if err != nil || got.ID != "session" {
		t.Fatalf("trimmed lookup = %#v, %v", got, err)
	}
	if err := store.Delete(t.Context(), " session "); err != nil {
		t.Fatal(err)
	}
	if _, err := store.Get(t.Context(), "session"); !errors.Is(err, ErrInstallSessionNotFound) {
		t.Fatalf("deleted session error = %v", err)
	}

	for i := 0; i < installSessionCap; i++ {
		if err := store.Put(t.Context(), InstallSession{ID: "cap-" + strconv.Itoa(i), ExpiresAt: expires}); err != nil {
			t.Fatalf("put %d: %v", i, err)
		}
	}
	if err := store.Put(t.Context(), InstallSession{ID: "over-cap", ExpiresAt: expires}); err == nil {
		t.Fatal("expected install session capacity error")
	}
}

func TestFallbackInstallSessionStoreReadsFallbackAfterPrimaryFailure(t *testing.T) {
	fallback := NewMemorySessionStore()
	primary := &unavailableSessionStore{}
	store := &fallbackSessionStore{primary: primary, fallback: fallback}
	session := InstallSession{ID: "fallback-session", ExpiresAt: time.Now().Add(time.Minute)}
	if err := store.Put(t.Context(), session); err != nil {
		t.Fatal(err)
	}
	got, err := store.Get(t.Context(), session.ID)
	if err != nil || got.ID != session.ID {
		t.Fatalf("fallback lookup = %#v, %v", got, err)
	}
}

type unavailableSessionStore struct{}

func (*unavailableSessionStore) Put(_ context.Context, _ InstallSession) error {
	return errors.New("redis unavailable")
}

func (*unavailableSessionStore) Get(_ context.Context, _ string) (InstallSession, error) {
	return InstallSession{}, errors.New("redis unavailable")
}

func (*unavailableSessionStore) Delete(_ context.Context, _ string) error {
	return errors.New("redis unavailable")
}
