package engine

import (
	"context"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
)

func TestWorkspaceInstallationCannotPersistWithoutHubRouting(t *testing.T) {
	for _, text := range []string{"hello", "/agents 2", "/new"} {
		t.Run(text, func(t *testing.T) {
			h := newHarness(t)
			h.inst.inst.AgentID = pgtype.UUID{Valid: true}
			h.media.noMedia = true
			t.Cleanup(func() {
				ctx, cancel := context.WithTimeout(context.Background(), time.Second)
				defer cancel()
				if !h.router.Drain(ctx) {
					t.Error("router did not finish the test's detached work")
				}
			})
			msg := p2pMessage(t)
			msg.Text, msg.CommandText = text, text
			if err := h.router.Handle(context.Background(), msg); err == nil {
				t.Error("workspace-owned installation proceeded without a Hub to select its Agent")
			}
			if h.binder.ensureCalls != 0 || h.binder.startCalls != 0 || h.binder.appendedParams().InstallationID.Valid {
				t.Error("an unresolved nil-Agent installation created or wrote a Chat")
			}
			if h.tasks.wasCalled() || h.tasks.wasPrepared() {
				t.Error("an unresolved workspace installation attempted an Agent run")
			}
			if h.dedup.releases() != 1 || h.dedup.marks() != 0 {
				t.Error("missing Hub configuration must release the claim for retry after repair")
			}
		})
	}
}
