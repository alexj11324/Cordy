package engine

import (
	"context"
	"errors"
	"testing"

	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
)

type groupRouteInstaller struct {
	*fakeInstaller
	agentID pgtype.UUID
	calls   int
	err     error
}

func (f *groupRouteInstaller) FinalizeInstallation(_ context.Context, inst ResolvedInstallation, _ channel.InboundMessage) (ResolvedInstallation, error) {
	f.calls++
	inst.AgentID = f.agentID
	inst.RouteRevision = int64(f.calls)
	return inst, f.err
}

func TestRouter_GroupRouteFinalizedOnlyAfterAddressingAndMembership(t *testing.T) {
	for _, tc := range []struct {
		name      string
		addressed bool
		err       error
	}{
		{name: "idle group chatter"},
		{name: "unbound sender", addressed: true, err: ErrSenderUnbound},
		{name: "removed member", addressed: true, err: ErrSenderNotMember},
	} {
		t.Run(tc.name, func(t *testing.T) {
			h := newHarness(t)
			installer := &groupRouteInstaller{fakeInstaller: h.inst, agentID: uid(91)}
			set := h.router.sets[channel.TypeFeishu]
			set.Installation = installer
			h.ident.err = tc.err
			msg := p2pMessage(t)
			msg.Source.ChatType = channel.ChatTypeGroup
			msg.AddressedToBot = tc.addressed
			if _, _, err := h.router.dispatch(context.Background(), set, msg, false, false); err != nil {
				t.Fatal(err)
			}
			if installer.calls != 0 || h.binder.ensureCalls != 0 {
				t.Fatal("rejected group message must not discover a route or create a session")
			}
		})
	}
}

func TestRouter_GroupRouteTargetReachesSessionAndOutbound(t *testing.T) {
	for _, startChat := range []bool{false, true} {
		h := newHarness(t)
		h.media.noMedia = true
		installer := &groupRouteInstaller{fakeInstaller: h.inst, agentID: uid(91)}
		set := h.router.sets[channel.TypeFeishu]
		set.Installation = installer
		msg := p2pMessage(t)
		msg.Source.ChatType = channel.ChatTypeGroup
		msg.AddressedToBot = true
		msg.SkipAgentRun = true
		msg.CommandText = msg.Text
		_, outbound, err := h.router.dispatch(context.Background(), set, msg, false, startChat)
		if err != nil {
			t.Fatal(err)
		}
		actual := h.binder.lastEnsure.Installation
		if startChat {
			actual = h.binder.lastStart.Installation
		}
		if actual.AgentID != installer.agentID || outbound.AgentID != installer.agentID {
			t.Fatalf("start=%v: session agent=%v, outbound agent=%v, want group agent=%v", startChat, actual.AgentID, outbound.AgentID, installer.agentID)
		}
		if outbound.ID != h.inst.inst.ID || outbound.WorkspaceID != h.inst.inst.WorkspaceID {
			t.Fatal("group reassignment must preserve installation and workspace identity")
		}
		if !startChat && h.binder.lastAppend.Installation != outbound {
			t.Fatal("append must receive the same resolved target and revision as the session and outbound path")
		}
	}
}

func TestRouter_GroupRouteConflictResolvesNewTargetBeforeRetry(t *testing.T) {
	h := newHarness(t)
	h.media.noMedia = true
	installer := &groupRouteInstaller{fakeInstaller: h.inst, agentID: uid(91)}
	set := h.router.sets[channel.TypeFeishu]
	set.Installation = installer
	h.binder.ensureHook = func() {
		h.binder.ensureErr = nil
		if h.binder.ensureCalls == 1 {
			installer.agentID = uid(92)
			h.binder.ensureErr = ErrRouteChanged
		}
	}
	msg := p2pMessage(t)
	msg.Source.ChatType = channel.ChatTypeGroup
	msg.AddressedToBot = true
	msg.SkipAgentRun = true
	res, outbound, err := h.router.dispatch(context.Background(), set, msg, false, false)
	if err != nil {
		t.Fatal(err)
	}
	if installer.calls != 2 || outbound.AgentID != uid(92) || h.binder.lastEnsure.Installation.AgentID != uid(92) {
		t.Fatalf("retry reused stale group target: finalizations=%d outbound=%v session=%v", installer.calls, outbound.AgentID, h.binder.lastEnsure.Installation.AgentID)
	}
	if res.Outcome != OutcomeIngested || h.dedup.claimCalls != 1 || h.dedup.releases() != 0 {
		t.Fatal("route retry must keep the original dedup claim and ingest once")
	}
}

func TestRouter_GroupRouteFailureReleasesClaimWithoutWriting(t *testing.T) {
	h := newHarness(t)
	want := errors.New("group route database unavailable")
	set := h.router.sets[channel.TypeFeishu]
	set.Installation = &groupRouteInstaller{fakeInstaller: h.inst, err: want}
	msg := p2pMessage(t)
	msg.Source.ChatType = channel.ChatTypeGroup
	msg.AddressedToBot = true
	_, _, err := h.router.dispatch(context.Background(), set, msg, false, false)
	if !errors.Is(err, want) || h.dedup.releases() != 1 || h.binder.ensureCalls != 0 {
		t.Fatalf("route failure must release without writing: err=%v releases=%d sessions=%d", err, h.dedup.releases(), h.binder.ensureCalls)
	}
}
