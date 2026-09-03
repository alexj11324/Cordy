package engine

import (
	"context"
	"sync"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
)

// runtimeStatus serializes observations within one supervisor generation.
// The store fences other generations; closed fences callbacks still unwinding
// locally after Disconnect. Lease renewal retries an unpersisted one-shot
// handshake without restarting an otherwise working provider connection.
type runtimeStatus struct {
	supervisor *Supervisor
	id         pgtype.UUID
	token      string
	mu         sync.Mutex
	latest     channel.RuntimeObservation
	closed     bool
	restore    bool
}

func (s *Supervisor) newRuntimeStatus(id pgtype.UUID, token string) *runtimeStatus {
	return &runtimeStatus{supervisor: s, id: id, token: token, latest: channel.RuntimeObservation{State: "starting"}}
}

func (r *runtimeStatus) persistLocked(ctx context.Context, observation channel.RuntimeObservation) bool {
	r.latest = observation
	ctx, cancel := context.WithTimeout(ctx, r.supervisor.cfg.LeaseReleaseTimeout)
	defer cancel()
	applied, err := r.supervisor.store.ObserveRuntime(ctx, r.id, r.token, observation)
	if err != nil {
		r.supervisor.cfg.Logger.WarnContext(ctx, "channel connection status persistence failed", "installation_id", uuidString(r.id), "error", err)
		return false
	}
	return applied
}

func (r *runtimeStatus) report(ctx context.Context, observation channel.RuntimeObservation) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.closed || ctx.Err() != nil {
		return false
	}
	return r.persistLocked(ctx, observation)
}

func (r *runtimeStatus) renewalFailed(ctx context.Context) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.closed {
		return
	}
	r.restore = r.restore || r.latest.State == "healthy"
	r.persistLocked(ctx, channel.RuntimeObservation{State: "degraded", ErrorCode: "lease_renewal_failed"})
}

func (r *runtimeStatus) renewed(ctx context.Context) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.closed {
		return
	}
	observation := r.latest
	if r.restore && observation.ErrorCode == "lease_renewal_failed" {
		observation = channel.RuntimeObservation{State: "healthy"}
	}
	r.restore = false
	r.persistLocked(ctx, observation)
}

func (r *runtimeStatus) finish(observation channel.RuntimeObservation) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.closed {
		return
	}
	r.closed = true
	r.persistLocked(context.Background(), observation)
}
