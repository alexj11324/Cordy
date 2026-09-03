package engine

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
)

type rejectedObserverStore struct {
	*fakeStore
	claimed chan struct{}
	once    sync.Once
}

func (s *rejectedObserverStore) ClaimRuntimeObserver(context.Context, pgtype.UUID, string) error {
	s.once.Do(func() { close(s.claimed) })
	return errors.New("observation storage unavailable")
}

func TestSupervisorDoesNotConnectWithoutDurableObserverClaim(t *testing.T) {
	store := &rejectedObserverStore{fakeStore: newFakeStore(), claimed: make(chan struct{})}
	store.installations = []Installation{activeInst(uid(71), "connection-fixture")}
	built := make(chan struct{}, 1)
	registry := channel.NewRegistry()
	registry.Register(channel.TypeFeishu, func(channel.Config) (channel.Channel, error) {
		built <- struct{}{}
		return &fakeChannel{typ: channel.TypeFeishu}, nil
	})
	supervisor := NewSupervisor(store, store, registry, nil, fastConfig())
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer func() { cancel(); supervisor.Wait() }()
	go supervisor.Run(ctx)
	select {
	case <-store.claimed:
		cancel()
		supervisor.Wait()
		select {
		case <-built:
			t.Fatal("transport started after its durable observation claim failed")
		default:
		}
	case <-built:
		t.Fatal("transport started without claiming the current connection status generation")
	case <-ctx.Done():
		t.Fatal("supervisor made no connection-status claim")
	}
}
