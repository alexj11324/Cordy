package hostedcapacity

import (
	"context"
	"log/slog"
	"time"

	"golang.org/x/sync/errgroup"

	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

const (
	defaultSweepInterval = 5 * time.Minute
	defaultConcurrency   = 8
)

// WorkerConfig tunes the background reconciler. Zero values take the defaults.
type WorkerConfig struct {
	Interval    time.Duration
	Concurrency int
	Logger      *slog.Logger
}

// Worker sweeps every workspace that holds installed connections and re-aligns
// its pause markers with the current Cloud policy. Subscription changes land
// here within one interval even when no install is attempted; the per-request
// reconcile in Limiter.InstallationLimit keeps latency low, this sweep keeps
// the state convergent.
//
// An unavailable policy preserves the last authoritative pause state — the
// sweep never guesses a cap it could not read. A disabled resolver never
// starts a goroutine at all (Run returns immediately), so self-hosted
// deployments pay nothing.
type Worker struct {
	resolver *Resolver
	queries  Queries
	tx       txStarter
	interval time.Duration
	limit    int
	logger   *slog.Logger
}

// NewWorker builds the sweep. Returns nil when the resolver is nil or
// disabled, so the caller's `if w != nil { go w.Run(ctx) }` is the whole
// lifecycle.
func NewWorker(resolver *Resolver, q *db.Queries, tx txStarter, cfg WorkerConfig) *Worker {
	if resolver == nil || !resolver.Enabled() {
		return nil
	}
	interval := cfg.Interval
	if interval <= 0 {
		interval = defaultSweepInterval
	}
	concurrency := cfg.Concurrency
	if concurrency <= 0 {
		concurrency = defaultConcurrency
	}
	logger := cfg.Logger
	if logger == nil {
		logger = slog.Default()
	}
	return &Worker{
		resolver: resolver,
		queries:  dbQueries{q},
		tx:       tx,
		interval: interval,
		limit:    concurrency,
		logger:   logger,
	}
}

// Run sweeps until the context is cancelled. The first sweep is delayed to
// the first tick so a restarting server does not stampede Cloud at boot.
func (w *Worker) Run(ctx context.Context) {
	if w == nil {
		return
	}
	ticker := time.NewTicker(w.interval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			if err := w.Sweep(ctx); err != nil && ctx.Err() == nil {
				w.logger.WarnContext(ctx, "hosted installation capacity sweep failed", "error", err.Error())
			}
		}
	}
}

// Sweep reconciles every workspace holding installed connections, at most
// `Concurrency` at a time.
func (w *Worker) Sweep(ctx context.Context) error {
	workspaceIDs, err := w.queries.ListHostedInstallationWorkspaces(ctx)
	if err != nil {
		return err
	}
	group, groupCtx := errgroup.WithContext(ctx)
	group.SetLimit(w.limit)
	for _, workspaceID := range workspaceIDs {
		workspaceID := workspaceID
		group.Go(func() error {
			w.reconcileWorkspace(groupCtx, workspaceID)
			return nil
		})
	}
	return group.Wait()
}

// reconcileWorkspace is best-effort per workspace: one workspace's Cloud
// failure or DB error is logged and skipped, never fatal to the sweep.
func (w *Worker) reconcileWorkspace(ctx context.Context, workspaceID pgtype.UUID) {
	policy := w.resolver.Resolve(ctx, workspaceID)
	if policy.Disabled() || policy.Unavailable() {
		// Disabled cannot happen (the worker never starts), unavailable
		// preserves the last authoritative pause state.
		return
	}
	result, err := Reconcile(ctx, w.queries, w.tx, workspaceID, policy.Limit())
	if err != nil {
		w.logger.WarnContext(ctx, "failed to reconcile hosted installation capacity",
			"workspace_id", workspaceUUIDString(workspaceID), "error", err.Error())
		return
	}
	if len(result.Paused) > 0 || len(result.Resumed) > 0 {
		w.logger.InfoContext(ctx, "reconciled hosted installation capacity",
			"workspace_id", workspaceUUIDString(workspaceID),
			"paused", len(result.Paused),
			"resumed", len(result.Resumed))
	}
}
