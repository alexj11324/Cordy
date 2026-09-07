package daemon

import (
	"context"
	"errors"
	"fmt"
	"math/rand"
	"os"
	"sync"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/cli"
	"github.com/orvilo-ai/orvilo/server/pkg/agent"
)

// runtimeSetWatcher is a tiny pub/sub for runtime-set changes. It exists
// because more than one supervisor (taskWakeupLoop, heartbeatLoop, pollLoop)
// needs to react to runtime-set changes; a single buffered channel would
// race so only the first listener would learn about each change.
//
// Each subscriber gets a 1-slot channel; missed nudges coalesce into a
// single signal — the subscriber is expected to re-derive the current
// runtime set via allRuntimeIDs() rather than relying on edge counts.
type runtimeSetWatcher struct {
	mu          sync.Mutex
	subscribers map[chan struct{}]struct{}
}

func newRuntimeSetWatcher() *runtimeSetWatcher {
	return &runtimeSetWatcher{subscribers: make(map[chan struct{}]struct{})}
}

// Subscribe returns a channel that receives a non-blocking nudge whenever
// the runtime set changes, and an unsubscribe func the caller must invoke
// when done.
func (w *runtimeSetWatcher) Subscribe() (<-chan struct{}, func()) {
	ch := make(chan struct{}, 1)
	w.mu.Lock()
	w.subscribers[ch] = struct{}{}
	w.mu.Unlock()
	return ch, func() {
		w.mu.Lock()
		delete(w.subscribers, ch)
		w.mu.Unlock()
	}
}

func (w *runtimeSetWatcher) notify() {
	w.mu.Lock()
	defer w.mu.Unlock()
	for ch := range w.subscribers {
		select {
		case ch <- struct{}{}:
		default:
		}
	}
}

// wsHeartbeatFreshness defines how long a WS heartbeat ack is considered
// "fresh enough" to suppress the HTTP heartbeat for that runtime. The window
// is 2× HeartbeatInterval so a single dropped WS ack still keeps HTTP
// suppressed, but two missed acks (~30s of WS silence) re-enable HTTP — well
// inside the server-side 45s offline threshold.
func (d *Daemon) wsHeartbeatFreshness() time.Duration {
	if d.cfg.HeartbeatInterval <= 0 {
		return 30 * time.Second
	}
	return 2 * d.cfg.HeartbeatInterval
}

// recordWSHeartbeatAck stamps the runtime as having received a fresh WS
// heartbeat ack from the server. Called by the WS read pump.
func (d *Daemon) recordWSHeartbeatAck(runtimeID string) {
	if runtimeID == "" {
		return
	}
	d.wsHBMu.Lock()
	d.wsHBLastAck[runtimeID] = time.Now()
	d.wsHBMu.Unlock()
}

// wsHeartbeatRecentlyAcked reports whether the runtime received a WS
// heartbeat ack inside the freshness window. The HTTP heartbeat loop uses
// this to skip duplicate work when WS is already keeping the runtime alive.
func (d *Daemon) wsHeartbeatRecentlyAcked(runtimeID string) bool {
	d.wsHBMu.RLock()
	last, ok := d.wsHBLastAck[runtimeID]
	d.wsHBMu.RUnlock()
	if !ok {
		return false
	}
	return time.Since(last) < d.wsHeartbeatFreshness()
}

// clearWSHeartbeatAcks drops all WS heartbeat freshness records. Called on
// WS disconnect so HTTP heartbeats resume on the next tick.
func (d *Daemon) clearWSHeartbeatAcks() {
	d.wsHBMu.Lock()
	for k := range d.wsHBLastAck {
		delete(d.wsHBLastAck, k)
	}
	d.wsHBMu.Unlock()
}

// heartbeatLoop supervises per-runtime HTTP heartbeat goroutines. Each runtime
// gets an independent ticker so a slow heartbeat for one runtime cannot block
// heartbeats for any other runtime — this matters when a single daemon serves
// multiple workspaces, because the previous shared loop would serialize an
// up-to-30s HTTP timeout across every runtime in the set.
func (d *Daemon) heartbeatLoop(ctx context.Context) {
	runtimeSetCh, unsub := d.runtimeSet.Subscribe()
	defer unsub()

	cancels := make(map[string]context.CancelFunc)
	defer func() {
		for _, cancel := range cancels {
			cancel()
		}
	}()

	sync := func() {
		want := make(map[string]struct{})
		for _, rid := range d.allRuntimeIDs() {
			want[rid] = struct{}{}
		}
		for rid, cancel := range cancels {
			if _, ok := want[rid]; !ok {
				cancel()
				delete(cancels, rid)
			}
		}
		for rid := range want {
			if _, ok := cancels[rid]; ok {
				continue
			}
			rctx, rcancel := context.WithCancel(ctx)
			cancels[rid] = rcancel
			go d.runRuntimeHeartbeat(rctx, rid)
		}
	}

	sync()
	for {
		select {
		case <-ctx.Done():
			return
		case <-runtimeSetCh:
			sync()
		}
	}
}

// runRuntimeHeartbeat owns the HTTP heartbeat schedule for a single runtime.
// The first tick fires after a small jittered delay (up to one full interval)
// to avoid a thundering herd when the daemon registers many runtimes at once.
func (d *Daemon) runRuntimeHeartbeat(ctx context.Context, rid string) {
	interval := d.cfg.HeartbeatInterval
	if interval <= 0 {
		interval = 15 * time.Second
	}
	// Jittered initial delay; cap at the interval so the first beat still
	// happens within one period.
	if jitter := time.Duration(rand.Int63n(int64(interval))); jitter > 0 {
		select {
		case <-ctx.Done():
			return
		case <-time.After(jitter):
		}
	}

	consecutiveTransientFailures := 0
	tick := func() {
		if d.runHeartbeatTick(ctx, rid) {
			consecutiveTransientFailures++
			if consecutiveTransientFailures == 2 {
				d.client.CloseIdleConnections()
			}
			return
		}
		consecutiveTransientFailures = 0
	}

	tick()

	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			tick()
		}
	}
}

// runHeartbeatTick returns true when the HTTP heartbeat hit a transient
// failure that should count toward stale idle-connection cleanup.
func (d *Daemon) runHeartbeatTick(ctx context.Context, rid string) bool {
	// Skip HTTP heartbeat for runtimes that successfully acked a recent
	// WebSocket heartbeat. The WS path keeps last_seen_at fresh and delivers
	// actions, so the HTTP write would be a duplicate DB update. If the WS
	// heartbeat goes silent the freshness window expires and HTTP resumes
	// automatically on the next tick — that is the fallback the WS path
	// relies on.
	if d.wsHeartbeatRecentlyAcked(rid) {
		d.logger.Debug("heartbeat: skipping HTTP tick, WS recently acked", "runtime_id", rid)
		return false
	}
	d.logger.Debug("heartbeat: HTTP tick", "runtime_id", rid)
	resp, err := d.client.SendHeartbeat(ctx, rid)
	if err != nil {
		if ctx.Err() == nil {
			if isRuntimeNotFoundError(err) {
				// Server says this runtime is gone — recover instead of
				// looping on the dead UUID. handleRuntimeGone coalesces
				// concurrent callers and runs the recovery HTTP call under
				// the daemon root context so notifyRuntimeSetChanged
				// tearing down this heartbeat goroutine cannot abort it.
				go d.handleRuntimeGone(rid)
				return false
			}
			d.logger.Warn("heartbeat failed", "runtime_id", rid, "error", err)
		}
		return ctx.Err() == nil && isTransientError(err)
	}
	if resp != nil && resp.RuntimeGone {
		// The WS path returns a successful ack with RuntimeGone=true for the
		// same scenario; treat it the same way here in case HTTP starts
		// surfacing this signal too.
		go d.handleRuntimeGone(rid)
		return false
	}
	d.handleHeartbeatActions(ctx, rid, resp)
	return false
}

// handleHeartbeatActions dispatches the pending-action set returned by either
// transport (HTTP POST /api/daemon/heartbeat or WS daemon:heartbeat_ack).
// Each action is dispatched in its own goroutine so a slow handler cannot
// block subsequent heartbeats.
func (d *Daemon) handleHeartbeatActions(ctx context.Context, runtimeID string, resp *HeartbeatResponse) {
	if resp == nil {
		return
	}
	if resp.PendingUpdate != nil || resp.PendingModelList != nil || resp.PendingLocalSkills != nil || resp.PendingLocalSkillImport != nil {
		d.logger.Debug("heartbeat: pending actions",
			"runtime_id", runtimeID,
			"update", resp.PendingUpdate != nil,
			"model_list", resp.PendingModelList != nil,
			"local_skills", resp.PendingLocalSkills != nil,
			"local_skill_import", resp.PendingLocalSkillImport != nil,
		)
	}
	if resp.PendingUpdate != nil {
		go d.handleUpdate(ctx, runtimeID, resp.PendingUpdate)
	}
	if resp.PendingModelList != nil {
		if rt := d.findRuntime(runtimeID); rt != nil {
			go d.handleModelList(ctx, *rt, resp.PendingModelList.ID)
		}
	}
	if resp.PendingLocalSkills != nil {
		if rt := d.findRuntime(runtimeID); rt != nil {
			go d.handleLocalSkillList(ctx, *rt, resp.PendingLocalSkills.ID)
		}
	}
	// Prefer the batch field (new backend); fall back to singular (old backend).
	if len(resp.PendingLocalSkillImports) > 0 {
		if rt := d.findRuntime(runtimeID); rt != nil {
			for _, imp := range resp.PendingLocalSkillImports {
				go d.handleLocalSkillImport(ctx, *rt, imp)
			}
		}
	} else if resp.PendingLocalSkillImport != nil {
		if rt := d.findRuntime(runtimeID); rt != nil {
			go d.handleLocalSkillImport(ctx, *rt, *resp.PendingLocalSkillImport)
		}
	}
}

// handlePendingWorkHint reacts to a server-pushed daemon:pending_work frame by
// sending ONE immediate heartbeat for the runtime, then dispatching whatever
// that heartbeat claimed (MUL-5444).
//
// Why a heartbeat and not the work itself: the hint deliberately carries no
// request payload, so the server never has to un-claim anything when delivery
// fails, and a duplicated or replayed hint cannot duplicate work — the claim
// stays atomic inside the store's PopPending. A hint that never arrives (daemon
// offline, WS gap, relay drop) costs nothing but the pre-existing wait for the
// next scheduled heartbeat.
//
// The HTTP heartbeat is used on purpose rather than queueing a WS frame: the
// hint arrives on the read pump, the WS write path may be backed up or tearing
// down, and this is a human-interactive, low-frequency path where one extra
// request is cheaper than a missed wakeup. Note it intentionally bypasses the
// wsHeartbeatRecentlyAcked suppression that the scheduled HTTP tick honours —
// that suppression exists to avoid duplicate periodic writes, not to block an
// explicitly requested pull.
func (d *Daemon) handlePendingWorkHint(runtimeID, kind string) {
	if runtimeID == "" {
		return
	}
	if d.findRuntime(runtimeID) == nil {
		// Not one of ours (stale relay fanout, or the runtime was just pruned).
		return
	}

	d.pendingWorkMu.Lock()
	if d.pendingWorkInflight == nil {
		d.pendingWorkInflight = make(map[string]struct{})
	}
	if d.pendingWorkLastRun == nil {
		d.pendingWorkLastRun = make(map[string]time.Time)
	}
	// Drop long-idle bookkeeping so the map can't grow with every runtime this
	// process has ever seen.
	for id, at := range d.pendingWorkLastRun {
		if time.Since(at) > pendingWorkHintBookkeepingTTL {
			delete(d.pendingWorkLastRun, id)
		}
	}
	if _, inflight := d.pendingWorkInflight[runtimeID]; inflight {
		d.pendingWorkMu.Unlock()
		return
	}
	// Rate limit per runtime. The hint is caller-triggered by interactive model
	// or capability endpoints, so without a floor a request loop would become a
	// heartbeat amplifier. A suppressed hint costs nothing but the pre-existing
	// wait for the scheduled heartbeat.
	if last, ok := d.pendingWorkLastRun[runtimeID]; ok && time.Since(last) < pendingWorkHintMinInterval {
		d.pendingWorkMu.Unlock()
		d.logger.Debug("pending work hint throttled", "runtime_id", runtimeID, "kind", kind)
		return
	}
	d.pendingWorkInflight[runtimeID] = struct{}{}
	d.pendingWorkLastRun[runtimeID] = time.Now()
	d.pendingWorkMu.Unlock()
	defer func() {
		d.pendingWorkMu.Lock()
		delete(d.pendingWorkInflight, runtimeID)
		d.pendingWorkMu.Unlock()
	}()

	// Root context: handleHeartbeatActions hands this ctx to the actual work, so
	// it must outlive this function. Capability discovery and local-skill imports
	// are normally quick filesystem operations, while model discovery may shell
	// out to a CLI for up to ~40s on the slowest provider (see
	// agent.hermesDiscoveryTimeout). Only the heartbeat request itself is
	// time-bounded.
	ctx := d.recoveryContext()
	if ctx.Err() != nil {
		return
	}
	hbCtx, cancel := context.WithTimeout(ctx, pendingWorkHeartbeatTimeout)
	resp, err := d.client.SendHeartbeat(hbCtx, runtimeID)
	cancel()
	if err != nil {
		if isRuntimeNotFoundError(err) {
			go d.handleRuntimeGone(runtimeID)
			return
		}
		d.logger.Debug("pending work hint heartbeat failed", "runtime_id", runtimeID, "kind", kind, "error", err)
		return
	}
	if resp == nil {
		return
	}
	if resp.RuntimeGone {
		go d.handleRuntimeGone(runtimeID)
		return
	}
	d.logger.Debug("pending work hint served", "runtime_id", runtimeID, "kind", kind)
	d.handleHeartbeatActions(ctx, runtimeID, resp)
}

// handleModelList resolves the provider's supported models (via static
// catalog or by shelling out to the agent CLI) and reports the result
// back to the server.
//
// How a discovery failure is reported is the provider's decision, not this
// function's. Providers with a safe static catalog swallow the error and return
// the stand-in (marked Fallback); providers without one — hermes — return the
// error, and it is forwarded as status=failed so the picker can show the reason
// and keep manual entry (MUL-6606). Both outcomes leave the creatable dropdown
// usable; only the second one tells the user why it is empty.
func (d *Daemon) handleModelList(ctx context.Context, rt Runtime, requestID string) {
	d.logger.Info("model list requested", "runtime_id", rt.ID, "request_id", requestID, "provider", rt.Provider)

	// Discovery must enumerate the binary this runtime will actually execute,
	// otherwise the picker advertises a catalog the launched CLI never agreed
	// to (MUL-5789). Mirror runTask's resolution order: a custom runtime
	// profile (MUL-3284) owns the executable path, and such a runtime can live
	// on a host with NO built-in agent of the same protocol family installed —
	// so a custom runtime must never fail on the built-in lookup. A custom
	// path is also never re-resolved: like runTask, we don't second-guess a
	// path the profile pinned.
	var execPath string
	// fixedArgs mirrors the launch prefix runTask would use. Discovery has to
	// enumerate the CLI the profile actually runs, so a subcommand wrapper is
	// probed as `ccms start q36 models`, not `ccms models` (GH #7046).
	var fixedArgs []string
	if customSpec, isCustom := d.customProfileLaunchForRuntime(rt.ID); isCustom {
		execPath = customSpec.path
		fixedArgs = agent.FilterLaunchPrefix(rt.Provider, customSpec.fixedArgs, d.logger)
		d.logger.Info("model list uses custom runtime profile command",
			"runtime_id", rt.ID, "provider", rt.Provider, "command_path", execPath,
			"fixed_args", len(fixedArgs))
	} else if entry, ok := d.agents()[rt.Provider]; ok {
		// Built-in provider: self-heal a pinned executable path an in-place
		// upgrade deleted (MUL-4486).
		entry, _ = d.resolveAgentEntry(ctx, rt.Provider, entry)
		execPath = entry.Path
	} else {
		d.reportModelListResult(ctx, rt, requestID, map[string]any{
			"status": "failed",
			"error":  fmt.Sprintf("no agent configured for provider %q", rt.Provider),
		})
		return
	}

	catalog, err := listModels(ctx, rt.Provider, agent.NewCommand(execPath, fixedArgs))
	if err != nil {
		d.reportModelListResult(ctx, rt, requestID, map[string]any{
			"status": "failed",
			"error":  err.Error(),
		})
		return
	}
	models := catalog.Models
	if catalog.Fallback {
		d.logger.Warn("model discovery fell back to a static catalog; reporting it as non-authoritative",
			"runtime_id", rt.ID, "provider", rt.Provider, "path", execPath, "count", len(models))
	}
	if rt.Provider == "codearts" && len(models) == 0 {
		if home, homeErr := os.UserHomeDir(); homeErr == nil {
			configuredModels, configErr := loadCodeArtsConfiguredModels(home)
			if configErr != nil {
				d.logger.Warn("CodeArts custom model discovery failed",
					"runtime_id", rt.ID, "path", codeArtsUserConfigPath(home), "error", configErr)
			} else if len(configuredModels) > 0 {
				models = configuredModels
				d.logger.Info("CodeArts model discovery used user-configured providers",
					"runtime_id", rt.ID, "path", codeArtsUserConfigPath(home), "count", len(models))
			}
		}
	}

	// Wire format matches handler.ModelEntry. Use a struct (not
	// map[string]string) so the Default bool and the per-model
	// Thinking catalog round-trip — without it the UI loses its
	// "default" badge on the advertised pick and the thinking-level
	// picker for claude/codex (MUL-2339).
	type thinkingLevelWire struct {
		Value       string `json:"value"`
		Label       string `json:"label"`
		Description string `json:"description,omitempty"`
	}
	type modelThinkingWire struct {
		SupportedLevels []thinkingLevelWire `json:"supported_levels"`
		DefaultLevel    string              `json:"default_level,omitempty"`
	}
	type modelServiceTierWire struct {
		ID          string `json:"id"`
		Name        string `json:"name"`
		Description string `json:"description,omitempty"`
	}
	type modelWire struct {
		ID                                  string                 `json:"id"`
		Label                               string                 `json:"label"`
		Provider                            string                 `json:"provider,omitempty"`
		Default                             bool                   `json:"default,omitempty"`
		Thinking                            *modelThinkingWire     `json:"thinking,omitempty"`
		ServiceTiers                        []modelServiceTierWire `json:"service_tiers,omitempty"`
		SupportsExplicitStandardServiceTier bool                   `json:"supports_explicit_standard_service_tier,omitempty"`
	}
	wire := make([]modelWire, 0, len(models))
	for _, m := range models {
		entry := modelWire{
			ID:                                  m.ID,
			Label:                               m.Label,
			Provider:                            m.Provider,
			Default:                             m.Default,
			SupportsExplicitStandardServiceTier: m.SupportsExplicitStandardServiceTier,
		}
		if m.Thinking != nil {
			levels := make([]thinkingLevelWire, 0, len(m.Thinking.SupportedLevels))
			for _, lvl := range m.Thinking.SupportedLevels {
				levels = append(levels, thinkingLevelWire{
					Value:       lvl.Value,
					Label:       lvl.Label,
					Description: lvl.Description,
				})
			}
			entry.Thinking = &modelThinkingWire{
				SupportedLevels: levels,
				DefaultLevel:    m.Thinking.DefaultLevel,
			}
		}
		for _, tier := range m.ServiceTiers {
			entry.ServiceTiers = append(entry.ServiceTiers, modelServiceTierWire{
				ID:          tier.ID,
				Name:        tier.Name,
				Description: tier.Description,
			})
		}
		wire = append(wire, entry)
	}
	d.reportModelListResult(ctx, rt, requestID, map[string]any{
		"status":    "completed",
		"models":    wire,
		"supported": agent.ModelSelectionSupported(rt.Provider),
		// Additive field: the models are still worth rendering, but the server
		// must not persist them as this runtime's real catalog (MUL-5549).
		// Older servers ignore it and keep the previous behaviour.
		"fallback": catalog.Fallback,
	})
}

func (d *Daemon) handleLocalSkillList(ctx context.Context, rt Runtime, requestID string) {
	d.logger.Info("runtime local skills requested", "runtime_id", rt.ID, "request_id", requestID, "provider", rt.Provider)

	skills, supported, err := listRuntimeLocalSkills(rt.Provider)
	if err != nil {
		d.reportLocalSkillListResult(ctx, rt, requestID, map[string]any{
			"status": "failed",
			"error":  err.Error(),
		})
		return
	}
	mcpServers, mcpSupported, err := listRuntimeLocalMcpServers(rt.Provider)
	if err != nil {
		d.logger.Warn("runtime local MCP discovery failed", "runtime_id", rt.ID, "provider", rt.Provider, "error", err)
		mcpServers = []runtimeLocalMcpServerSummary{}
		mcpSupported = false
	}

	d.reportLocalSkillListResult(ctx, rt, requestID, map[string]any{
		"status":        "completed",
		"skills":        skills,
		"supported":     supported,
		"mcp_servers":   mcpServers,
		"mcp_supported": mcpSupported,
	})
}

func (d *Daemon) handleLocalSkillImport(ctx context.Context, rt Runtime, pending PendingLocalSkillImport) {
	d.logger.Info("runtime local skill import requested", "runtime_id", rt.ID, "request_id", pending.ID, "provider", rt.Provider, "skill_key", pending.SkillKey)

	skill, supported, err := loadRuntimeLocalSkillBundle(rt.Provider, pending.SkillKey)
	if err != nil {
		d.reportLocalSkillImportResult(ctx, rt, pending.ID, map[string]any{
			"status": "failed",
			"error":  err.Error(),
		})
		return
	}
	if !supported {
		d.reportLocalSkillImportResult(ctx, rt, pending.ID, map[string]any{
			"status": "failed",
			"error":  fmt.Sprintf("provider %q does not expose runtime local skills", rt.Provider),
		})
		return
	}

	d.reportLocalSkillImportResult(ctx, rt, pending.ID, map[string]any{
		"status": "completed",
		"skill":  skill,
	})
}

// runtimeReportBackoffs defines the retry schedule for delivering any
// daemon→server async result (model list, local-skill list, local-skill
// import). First attempt runs immediately, then we back off. The sum
// (≈6.5s) leaves most of the server-side running timeout (60s) available for
// discovery and report attempts. Each HTTP attempt has its own client timeout,
// so this schedule alone is not an end-to-end delivery deadline.
//
// Overridable for tests to avoid real sleeps.
var runtimeReportBackoffs = []time.Duration{0, 500 * time.Millisecond, 2 * time.Second, 4 * time.Second}

// reportLocalSkillListResult delivers a list-report to the server with retry
// on transient failures. See reportRuntimeResultWithRetry for semantics.
func (d *Daemon) reportLocalSkillListResult(ctx context.Context, rt Runtime, requestID string, payload map[string]any) {
	d.reportRuntimeResultWithRetry(ctx, "local_skill_list", rt.ID, requestID, func(ctx context.Context) error {
		return d.client.ReportLocalSkillListResult(ctx, rt.ID, requestID, payload)
	})
}

// reportLocalSkillImportResult delivers an import-report to the server with
// retry on transient failures.
func (d *Daemon) reportLocalSkillImportResult(ctx context.Context, rt Runtime, requestID string, payload map[string]any) {
	d.reportRuntimeResultWithRetry(ctx, "local_skill_import", rt.ID, requestID, func(ctx context.Context) error {
		return d.client.ReportLocalSkillImportResult(ctx, rt.ID, requestID, payload)
	})
}

// reportModelListResult delivers a model-list report to the server with retry
// on transient failures. Without this the daemon used to fire once and
// swallow any 5xx, leaving the request stranded in "running" on the server
// until its 60s timeout — defeating the multi-node store fix.
func (d *Daemon) reportModelListResult(ctx context.Context, rt Runtime, requestID string, payload map[string]any) {
	d.reportRuntimeResultWithRetry(ctx, "model_list", rt.ID, requestID, func(ctx context.Context) error {
		return d.client.ReportModelListResult(ctx, rt.ID, requestID, payload)
	})
}

// reportRuntimeResultWithRetry retries `fn` on 5xx / network errors and
// stops on success, 4xx, or after exhausting runtimeReportBackoffs.
//
// Why this exists: the server persists the report through a Redis / DB
// write; on a transient store failure it correctly returns 500. Without a
// client-side retry the daemon would fire once, swallow the error, and the
// pending request stays in "running" on the server until its timeout — which
// is exactly the "daemon did not respond" failure mode the multi-node store
// fix was meant to eliminate. 4xx is treated as permanent (request-not-found,
// cross-workspace token rejected, bad body) — retrying those just wastes
// heartbeat cycles.
func (d *Daemon) reportRuntimeResultWithRetry(ctx context.Context, kind, runtimeID, requestID string, fn func(context.Context) error) {
	var lastErr error
	for attempt, wait := range runtimeReportBackoffs {
		if wait > 0 {
			select {
			case <-ctx.Done():
				d.logger.Error("runtime async report cancelled",
					"kind", kind, "runtime_id", runtimeID, "request_id", requestID,
					"attempt", attempt, "error", ctx.Err())
				return
			case <-time.After(wait):
			}
		}
		err := fn(ctx)
		if err == nil {
			if attempt > 0 {
				d.logger.Info("runtime async report succeeded after retry",
					"kind", kind, "runtime_id", runtimeID, "request_id", requestID,
					"attempt", attempt+1)
			}
			return
		}
		lastErr = err

		// 4xx is permanent (request expired, workspace mismatch, malformed
		// body). No amount of retrying will make it succeed.
		var reqErr *requestError
		if errors.As(err, &reqErr) && reqErr.StatusCode >= 400 && reqErr.StatusCode < 500 {
			d.logger.Error("runtime async report rejected — not retrying",
				"kind", kind, "runtime_id", runtimeID, "request_id", requestID,
				"status", reqErr.StatusCode, "error", err)
			return
		}

		d.logger.Warn("runtime async report failed — will retry",
			"kind", kind, "runtime_id", runtimeID, "request_id", requestID,
			"attempt", attempt+1, "error", err)
	}
	d.logger.Error("runtime async report exhausted retries",
		"kind", kind, "runtime_id", runtimeID, "request_id", requestID, "error", lastErr)
}

// handleUpdate performs the CLI update when triggered by the server via heartbeat.
func (d *Daemon) handleUpdate(ctx context.Context, runtimeID string, update *PendingUpdate) {
	// Desktop-managed daemons share their CLI binary with the Electron app,
	// which is responsible for shipping and replacing it. Letting the daemon
	// self-update would just get overwritten on the next Desktop launch and
	// could brick the embedded binary mid-update. Refuse cleanly.
	if d.cfg.LaunchedBy == "desktop" {
		d.logger.Info("refusing CLI self-update: daemon is managed by Desktop", "runtime_id", runtimeID, "update_id", update.ID)
		d.reportUpdateResult(ctx, runtimeID, update.ID, map[string]any{
			"status": "failed",
			"error":  "CLI is managed by Orvilo Desktop — update the Desktop app to upgrade the CLI",
		})
		return
	}

	switch d.tryBeginServerUpdate(ctx) {
	case serverUpdateAlreadyRunning:
		d.logger.Warn("update deferred: another update is already in progress", "runtime_id", runtimeID, "update_id", update.ID)
		d.reportUpdateResult(ctx, runtimeID, update.ID, map[string]any{
			"status": "failed",
			"error":  "another runtime update is already in progress on this machine",
		})
		return
	case serverUpdateRuntimeBusy:
		d.logger.Info("update deferred: task or claim in progress", "runtime_id", runtimeID, "update_id", update.ID)
		d.reportUpdateResult(ctx, runtimeID, update.ID, map[string]any{
			"status": "failed",
			"error":  "runtime update deferred because agent work is starting or still active; retry when the machine is idle",
		})
		return
	}
	restarting := false
	defer func() {
		if !restarting {
			d.releaseClaimBarrier()
			d.updating.Store(false)
		}
	}()

	d.logger.Info("CLI update requested", "runtime_id", runtimeID, "update_id", update.ID, "target_version", update.TargetVersion)

	// Report running status.
	d.reportUpdateResult(ctx, runtimeID, update.ID, map[string]any{
		"status": "running",
	})

	output, err := d.runUpdateFn(update.TargetVersion)
	if err != nil {
		d.logger.Error("CLI update failed", "error", err, "output", output)
		d.reportUpdateResult(ctx, runtimeID, update.ID, map[string]any{
			"status": "failed",
			"error":  err.Error(),
		})
		return
	}

	d.logger.Info("CLI update completed successfully", "output", output)
	d.reportUpdateResult(ctx, runtimeID, update.ID, map[string]any{
		"status": "completed",
		"output": fmt.Sprintf("Updated to %s", update.TargetVersion),
	})

	// Trigger daemon restart with the new binary.
	d.triggerRestart()
	restarting = d.RestartBinary() != ""
}

type serverUpdateAcquireResult uint8

const (
	serverUpdateAcquired serverUpdateAcquireResult = iota
	serverUpdateAlreadyRunning
	serverUpdateRuntimeBusy
)

// tryBeginServerUpdate atomically claims update ownership, pauses new claims,
// and lets any claim already in flight finish its handoff. An empty claim can
// therefore never starve an update; a claim that returns work increments
// activeTasks before exitClaim, so the final check still defers safely.
func (d *Daemon) tryBeginServerUpdate(ctx context.Context) serverUpdateAcquireResult {
	if !d.updating.CompareAndSwap(false, true) {
		return serverUpdateAlreadyRunning
	}

	d.claimMu.Lock()
	if d.pauseClaims || d.terminalPersistenceFailed.Load() || d.terminalRecoveryFailed.Load() || d.terminalDeliveryBlocked.Load() || d.activeTasks.Load() > 0 {
		d.claimMu.Unlock()
		d.updating.Store(false)
		return serverUpdateRuntimeBusy
	}
	d.pauseClaims = true
	d.claimMu.Unlock()

	ticker := time.NewTicker(10 * time.Millisecond)
	defer ticker.Stop()
	for {
		d.claimMu.Lock()
		if d.claimsInFlight == 0 {
			if d.activeTasks.Load() == 0 {
				d.claimMu.Unlock()
				return serverUpdateAcquired
			}
			d.pauseClaims = false
			d.claimMu.Unlock()
			d.updating.Store(false)
			return serverUpdateRuntimeBusy
		}
		d.claimMu.Unlock()

		select {
		case <-ctx.Done():
			d.releaseClaimBarrier()
			d.updating.Store(false)
			return serverUpdateRuntimeBusy
		case <-ticker.C:
		}
	}
}

// runUpdate executes the brew-or-download upgrade against targetVersion and
// returns the human-readable output (always populated, even on failure when
// brew gives us a useful diagnostic). The caller is responsible for the
// `updating` CAS guard and for reporting status back to the server / triggering
// the restart — extracted so the server-triggered path (handleUpdate) and the
// auto-update poller (autoUpdateLoop) share the exact same execution body.
func (d *Daemon) runUpdate(targetVersion string) (string, error) {
	if cli.IsBrewInstall() {
		d.logger.Info("updating CLI via Homebrew...")
		out, err := cli.UpdateViaBrew()
		if err != nil {
			return out, fmt.Errorf("brew upgrade failed: %w", err)
		}
		return out, nil
	}
	d.logger.Info("updating CLI via direct download...", "target_version", targetVersion)
	out, err := cli.UpdateViaDownload(targetVersion)
	if err != nil {
		return out, fmt.Errorf("download update failed: %w", err)
	}
	return out, nil
}

// updateReportBackoffs defines the retry schedule for delivering CLI update
// status back to the server. This mirrors localSkillReportBackoffs because
// both features have the same user-visible failure mode: the daemon completed
// work locally, but a transient report failure leaves the UI waiting until the
// server-side request times out.
//
// Overridable for tests to avoid real sleeps.
var updateReportBackoffs = []time.Duration{0, 500 * time.Millisecond, 2 * time.Second, 4 * time.Second}

func (d *Daemon) reportUpdateResult(ctx context.Context, runtimeID, updateID string, payload map[string]any) {
	d.reportUpdateResultWithRetry(ctx, runtimeID, updateID, func(ctx context.Context) error {
		return d.client.ReportUpdateResult(ctx, runtimeID, updateID, payload)
	})
}

func (d *Daemon) reportUpdateResultWithRetry(ctx context.Context, runtimeID, updateID string, fn func(context.Context) error) {
	var lastErr error
	for attempt, wait := range updateReportBackoffs {
		if wait > 0 {
			select {
			case <-ctx.Done():
				d.logger.Error("CLI update report cancelled",
					"runtime_id", runtimeID, "update_id", updateID,
					"attempt", attempt, "error", ctx.Err())
				return
			case <-time.After(wait):
			}
		}

		err := fn(ctx)
		if err == nil {
			if attempt > 0 {
				d.logger.Info("CLI update report succeeded after retry",
					"runtime_id", runtimeID, "update_id", updateID,
					"attempt", attempt+1)
			}
			return
		}
		lastErr = err

		var reqErr *requestError
		if errors.As(err, &reqErr) && reqErr.StatusCode >= 400 && reqErr.StatusCode < 500 {
			d.logger.Error("CLI update report rejected — not retrying",
				"runtime_id", runtimeID, "update_id", updateID,
				"status", reqErr.StatusCode, "error", err)
			return
		}

		d.logger.Warn("CLI update report failed — will retry",
			"runtime_id", runtimeID, "update_id", updateID,
			"attempt", attempt+1, "error", err)
	}
	d.logger.Error("CLI update report exhausted retries",
		"runtime_id", runtimeID, "update_id", updateID, "error", lastErr)
}
