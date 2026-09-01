package main

import (
	"context"
	"log/slog"
	"time"

	"github.com/patchbay-ai/patchbay/server/internal/service"
)

const (
	automationQuotaReconcileInterval   = time.Minute
	automationQuotaTerminalRecoveryAge = 10 * time.Minute
	// Manual/API dispatches have no durable retry owner. Six hours is far
	// beyond normal dispatch latency while still releasing a genuinely
	// abandoned slot before the entitlement period rolls over.
	automationQuotaPartialRecoveryAge = 6 * time.Hour
	automationQuotaReconcileBatch     = 100
)

func runAutomationQuotaReconciler(ctx context.Context, svc *service.AutomationService) {
	ticker := time.NewTicker(automationQuotaReconcileInterval)
	defer ticker.Stop()
	for {
		now := time.Now()
		if settled, err := svc.ReconcileAutomationQuotaReservations(
			ctx,
			now.Add(-automationQuotaTerminalRecoveryAge),
			now.Add(-automationQuotaPartialRecoveryAge),
			automationQuotaReconcileBatch,
		); err != nil {
			if ctx.Err() == nil {
				slog.Warn("automation quota reconciler failed", "error", err)
			}
		} else if settled > 0 {
			slog.Info("automation quota reconciler settled reservations", "count", settled)
		}
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
	}
}
