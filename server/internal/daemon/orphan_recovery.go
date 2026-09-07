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
			return fmt.Errorf("read terminal reports before orphan recovery: %w", err)
		}
		pending = make([]protocol.PendingTerminalReport, 0, len(reports))
		for _, report := range reports {
			pending = append(pending, protocol.PendingTerminalReport{TaskID: report.TaskID, ClaimFence: report.Identity.ClaimFence})
		}
	}
	return d.client.RecoverOrphans(ctx, runtimeID, pending...)
}
