package weixin

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"testing"
	"time"

	"github.com/go-redis/redismock/v9"
	"github.com/redis/go-redis/v9"
)

func TestConfigureSessionStoreNilRedisSupportsPutGet(t *testing.T) {
	var optionalRedis *redis.Client
	ConfigureSessionStore(optionalRedis)
	t.Cleanup(func() { ConfigureSessionStore(nil) })
	store := DefaultInstallSessionStore()
	session := InstallSession{ID: "no-redis-session", QRCode: "poll-token", ExpiresAt: time.Now().Add(time.Minute)}
	if err := store.Put(t.Context(), session); err != nil {
		t.Fatal(err)
	}
	got, err := store.Get(t.Context(), session.ID)
	if err != nil || got.QRCode != session.QRCode {
		t.Fatalf("configured memory store lookup = %#v, %v", got, err)
	}
}

func TestConfigureSessionStoreReadsRedisAcrossReconfiguration(t *testing.T) {
	client, mock := redismock.NewClientMock()
	t.Cleanup(func() { _ = client.Close() })
	t.Cleanup(func() { ConfigureSessionStore(nil) })
	session := InstallSession{ID: "shared-session", QRCode: "poll-token", ExpiresAt: time.Now().Add(time.Minute)}
	payload, err := json.Marshal(session)
	if err != nil {
		t.Fatal(err)
	}
	key := installSessionKey + session.ID
	mock.CustomMatch(func(_ []interface{}, actual []interface{}) error {
		if len(actual) != 5 || actual[0] != "set" || actual[1] != key || actual[3] != "px" {
			return fmt.Errorf("unexpected Redis SET shape: %v", actual)
		}
		value, ok := actual[2].([]byte)
		if !ok || !bytes.Equal(value, payload) {
			return errors.New("Redis SET did not preserve the install session")
		}
		ttl, ok := actual[4].(int64)
		if !ok || ttl <= 0 || ttl > time.Minute.Milliseconds() {
			return errors.New("Redis SET did not retain the bounded install TTL")
		}
		return nil
	}).ExpectSet(key, payload, time.Minute).SetVal("OK")
	mock.ExpectGet(key).SetVal(string(payload))
	ConfigureSessionStore(client)
	if err := DefaultInstallSessionStore().Put(t.Context(), session); err != nil {
		t.Fatal(err)
	}
	ConfigureSessionStore(client)
	got, err := DefaultInstallSessionStore().Get(t.Context(), session.ID)
	if err != nil || got.QRCode != session.QRCode {
		t.Fatalf("configured Redis store lookup = %#v, %v", got, err)
	}
	if err := mock.ExpectationsWereMet(); err != nil {
		t.Fatal(err)
	}
}

func TestConfigureSessionStoreClosedRedisFallsBackToMemory(t *testing.T) {
	client := redis.NewClient(&redis.Options{Addr: "127.0.0.1:1"})
	if err := client.Close(); err != nil {
		t.Fatal(err)
	}
	ConfigureSessionStore(client)
	t.Cleanup(func() { ConfigureSessionStore(nil) })
	session := InstallSession{ID: "closed-redis-session", QRCode: "poll-token", ExpiresAt: time.Now().Add(time.Minute)}
	store := DefaultInstallSessionStore()
	if err := store.Put(t.Context(), session); err != nil {
		t.Fatal(err)
	}
	got, err := store.Get(t.Context(), session.ID)
	if err != nil || got.QRCode != session.QRCode {
		t.Fatalf("configured fallback lookup = %#v, %v", got, err)
	}
}

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
