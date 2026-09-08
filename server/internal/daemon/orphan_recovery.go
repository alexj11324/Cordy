package daemon

import (
	"context"
	"fmt"

	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

// recoverOrphans preserves results already saved by the previous process even
// when their terminal callback is unavailable throughout startup. Sending the
// current outbox snapshot also covers runtime re-registration after startup.
func (d *Daemon) recoverOrphans(ctx context.Context, runtimeID string) error {
	var pending []protocol.PendingTerminalReport
	if d.terminalStore != nil {
		reports, err := d.terminalStore.Pending()
		if err != nil {
			d.terminalRecoveryFailed.Store(true)
			return fmt.Errorf("read terminal reports before orphan recovery: %w", err)
		}
		pending = make([]protocol.PendingTerminalReport, 0, len(reports))
		for _, report := range reports {
			pending = append(pending, protocol.PendingTerminalReport{TaskID: report.TaskID, ClaimFence: report.Identity.ClaimFence})
		}
	}
	if err := d.client.RecoverOrphans(ctx, runtimeID, pending...); err != nil {
		d.terminalRecoveryFailed.Store(true)
		return err
	}
	return nil
}

// recoverTrackedOrphans retries every tracked runtime after a recovery failure.
// A failed read or recovery request pauses claims globally because a fresh
// heartbeat would otherwise keep old tasks alive while the server still thinks
// they belong to the previous daemon. Only a complete pass clears the barrier.
func (d *Daemon) recoverTrackedOrphans(ctx context.Context) error {
	d.mu.Lock()
	workspaces := make([]struct {
		id         string
		runtimeIDs []string
	}, 0, len(d.workspaces))
	for id, ws := range d.workspaces {
		workspaces = append(workspaces, struct {
			id         string
			runtimeIDs []string
		}{id: id, runtimeIDs: append([]string(nil), ws.runtimeIDs...)})
	}
	d.mu.Unlock()

	for _, workspace := range workspaces {
		for _, runtimeID := range workspace.runtimeIDs {
			if err := d.recoverOrphans(ctx, runtimeID); err != nil {
				return fmt.Errorf("recover orphans for workspace %s runtime %s: %w", workspace.id, runtimeID, err)
			}
		}
	}
	d.terminalRecoveryFailed.Store(false)
	return nil
}
