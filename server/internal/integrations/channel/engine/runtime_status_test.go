package engine

import (
	"context"
	"errors"
	"testing"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
)

type observationStore struct {
	*fakeStore
	observations []channel.RuntimeObservation
	writeError   error
}

func (s *observationStore) ObserveRuntime(_ context.Context, _ pgtype.UUID, _ string, observation channel.RuntimeObservation) (bool, error) {
	if s.writeError != nil {
		return false, s.writeError
	}
	s.observations = append(s.observations, observation)
	return true, nil
}

func TestRuntimeStatusRequiresProviderConfirmationAndRetriesPersistence(t *testing.T) {
	store := &observationStore{fakeStore: newFakeStore()}
	supervisor := NewSupervisor(store, store, channel.NewRegistry(), nil, fastConfig())
	status := supervisor.newRuntimeStatus(uid(72), "generation")
	ctx := channel.WithRuntimeReporter(context.Background(), status.report)
	status.renewed(ctx)
	if got := store.observations[0].State; got != "starting" {
		t.Fatalf("lease renewal invented a provider handshake: %s", got)
	}
	store.writeError = errors.New("storage unavailable")
	if channel.ReportConnected(ctx) {
		t.Fatal("unpersisted provider confirmation reported success")
	}
	store.writeError = nil
	status.renewed(ctx)
	if got := store.observations[len(store.observations)-1].State; got != "healthy" {
		t.Fatalf("one-shot provider confirmation was lost after storage recovery: %s", got)
	}
	status.finish(channel.RuntimeObservation{State: "offline", ErrorCode: "lease_lost"})
	if channel.ReportConnected(ctx) {
		t.Fatal("late callback revived a closed observer")
	}
	if got := store.observations[len(store.observations)-1].State; got != "offline" {
		t.Fatalf("late callback replaced disconnected state: %s", got)
	}
}

func TestRuntimeLeaseRecoveryDoesNotOverwriteLaterProviderError(t *testing.T) {
	store := &observationStore{fakeStore: newFakeStore()}
	supervisor := NewSupervisor(store, store, channel.NewRegistry(), nil, fastConfig())
	status := supervisor.newRuntimeStatus(uid(73), "generation")
	ctx := context.Background()
	status.report(ctx, channel.RuntimeObservation{State: "healthy"})
	status.renewalFailed(ctx)
	status.renewed(ctx)
	if got := store.observations[len(store.observations)-1].State; got != "healthy" {
		t.Fatal("temporary lease error did not restore the confirmed connection")
	}
	status.renewalFailed(ctx)
	status.report(ctx, channel.RuntimeObservation{State: "error", ErrorCode: "authentication_failed"})
	status.renewed(ctx)
	if got := store.observations[len(store.observations)-1]; got.ErrorCode != "authentication_failed" {
		t.Fatalf("lease recovery masked a later provider failure: %+v", got)
	}
}
