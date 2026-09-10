package daemon

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/orvilo-ai/orvilo/server/pkg/agent"
	"github.com/orvilo-ai/orvilo/server/pkg/redact"
)

// providerExecutionClient can persist an in-flight transcript and resume pointer;
// the provider stage has no access to claims, task transitions or daemon tokens.
type providerExecutionClient interface {
	ReportTaskMessages(context.Context, string, []TaskMessageData) error
	PinTaskSession(context.Context, string, string, string) error
}

// taskMessageState converts the provider-neutral Message event into the state
// vocabulary consumed by AI Elements' ToolHeader. A daemon tool-use event is a
// complete invocation event, including tools with no arguments, so it remains
// in the running state until the matching result arrives. Tool results carry
// a provider status when one is available, while providers that do not report
// one are successful by default because they emitted a terminal result event.
func taskMessageState(msg agent.Message) string {
	switch msg.Type {
	case agent.MessageToolUse:
		return "input-available"
	case agent.MessageToolResult:
		switch strings.ToLower(strings.TrimSpace(msg.Status)) {
		case "denied", "rejected":
			return "output-denied"
		case "failed", "error", "errored", "cancelled", "canceled":
			return "output-error"
		default:
			return "output-available"
		}
	default:
		return ""
	}
}

// providerExecution owns the stream/drain lifecycle of one provider session.
// Its counter remains shared with the owning daemon for health and shutdown.
type providerExecution struct {
	client       providerExecutionClient
	idleWatchdog time.Duration
	toolWatchdog time.Duration
	runningTasks *atomic.Int64
}

// attributedTextBuffer coalesces provider text chunks without losing the
// provider's explicit attribution. Citation offsets arrive relative to each
// chunk, so appending shifts them into the coalesced UTF-8 byte range.
type attributedTextBuffer struct {
	content   strings.Builder
	sources   []agent.MessageSource
	citations []agent.MessageCitation
	sourceIDs map[string]agent.MessageSource
}

func (b *attributedTextBuffer) append(msg agent.Message) {
	base := b.content.Len()
	b.content.WriteString(msg.Content)
	if len(msg.Sources) > 0 && b.sourceIDs == nil {
		b.sourceIDs = make(map[string]agent.MessageSource, len(msg.Sources))
	}
	remappedIDs := make(map[string]string, len(msg.Sources))
	for _, source := range msg.Sources {
		originalID := source.ID
		if existing, exists := b.sourceIDs[source.ID]; exists {
			if existing == source {
				remappedIDs[originalID] = source.ID
				continue
			}
			for suffix := 2; ; suffix++ {
				candidate := fmt.Sprintf("%s#%d", originalID, suffix)
				if _, used := b.sourceIDs[candidate]; !used {
					source.ID = candidate
					break
				}
			}
		}
		remappedIDs[originalID] = source.ID
		b.sourceIDs[source.ID] = source
		b.sources = append(b.sources, source)
	}
	for _, citation := range msg.Citations {
		if remapped, ok := remappedIDs[citation.SourceID]; ok {
			citation.SourceID = remapped
		}
		citation.Start += base
		citation.End += base
		b.citations = append(b.citations, citation)
	}
}

func (b *attributedTextBuffer) drain() (string, []agent.MessageSource, []agent.MessageCitation) {
	content := b.content.String()
	sources := b.sources
	citations := b.citations
	b.content.Reset()
	b.sources = nil
	b.citations = nil
	b.sourceIDs = nil
	return content, sources, citations
}

// executeAndDrain runs a backend, drains its message stream (forwarding to the
// server), and waits for the final result. msgSeq numbers the reported task
// messages and is owned by the caller so a same-task retry continues the
// sequence instead of restarting at 1 — the server orders the transcript by
// seq alone, and duplicate seqs would interleave the two attempts' rows.
func (p *providerExecution) executeAndDrain(ctx context.Context, backend agent.Backend, prompt string, opts agent.ExecOptions, taskLog *slog.Logger, taskID, codexHome string, msgSeq *atomic.Int32) (agent.Result, int32, error) {
	// Wrap the caller's ctx so the idle watchdog (below) can interrupt both
	// the agent subprocess (via the ctx passed to backend.Execute) AND the
	// drain loop with a single cancel. Without this layer the backend would
	// stay tied to the parent ctx and our cancellation could only abort
	// drain, leaving the subprocess running.
	agentCtx, agentCancel := context.WithCancel(ctx)
	defer agentCancel()

	session, err := backend.Execute(agentCtx, prompt, opts)
	if err != nil {
		// One provider-agnostic boundary for launches: every backend's
		// cmd.Start() failure arrives here, so diagnosing ENOEXEC at this point
		// covers claude, opencode and any CLI added later without a wrap in
		err = agent.ExplainExecError(err)
		taskLog.Debug("backend execute returned error", "error", err)
		return agent.Result{}, 0, err
	}
	// This counter intentionally starts at the narrower provider-session
	// boundary, not at the earlier server-side StartTask transition.
	p.runningTasks.Add(1)
	defer p.runningTasks.Add(-1)
	taskLog.Debug("backend started, draining messages")

	// Bound the drain loop only when there is a wall-clock cap. With a positive
	// opts.Timeout, give the drain a slightly longer deadline than the backend
	// so it can still collect the backend's own timeout Result if the scanner
	// is stuck on a hung stdout pipe (the extra 30 s covers cleanup after the
	// backend's own deadline fires). With no cap (opts.Timeout <= 0) the
	// inactivity watchdog is the only liveness net, so the drain must NOT
	// impose its own deadline either — otherwise an actively streaming long run
	var drainCtx context.Context
	var drainCancel context.CancelFunc
	if opts.Timeout > 0 {
		drainCtx, drainCancel = context.WithTimeout(agentCtx, opts.Timeout+30*time.Second)
	} else {
		drainCtx, drainCancel = context.WithCancel(agentCtx)
	}
	defer drainCancel()

	var toolCount atomic.Int32
	// lastActivityAt records (as unix nanos) when the drain loop most
	// recently received a message from the backend. The idle watchdog
	// reads this to decide whether the agent has gone silent for too long.
	// Initialise to the start so a backend that never emits a single
	// message also trips the watchdog.
	var lastActivityAt atomic.Int64
	lastActivityAt.Store(time.Now().UnixNano())
	// inFlightTools counts tool_use messages that haven't yet been paired
	// with a matching tool_result. A non-zero count means the agent is
	// legitimately waiting on a tool (e.g. `npm install`, `docker build`)
	// that may run far longer than the idle window without emitting any
	// message — so while a tool is in flight the watchdog applies the larger
	// AgentToolWatchdog budget instead of treating that silence as a hang.
	var inFlightTools atomic.Int32
	var idleWatchdogFired atomic.Bool
	// idleWatchdogThreshold records (as nanos) which silence budget actually
	// tripped the watchdog — the idle window or the larger in-flight-tool
	// window — so the failure message reports the real duration.
	idleWindow := p.idleWatchdog
	// A provider may opt into a shorter per-run no-message budget. The global
	// zero remains authoritative so ORVILO_AGENT_IDLE_WATCHDOG=0 still disables
	// the entire watchdog suite. Tool calls continue to use AgentToolWatchdog.
	if idleWindow > 0 && opts.IdleWatchdogTimeout > 0 && opts.IdleWatchdogTimeout < idleWindow {
		idleWindow = opts.IdleWatchdogTimeout
	}
	var idleWatchdogThreshold atomic.Int64
	idleWatchdogThreshold.Store(int64(idleWindow))
	if idleWindow > 0 {
		go p.runIdleWatchdog(agentCtx, idleWindow, p.toolWatchdog, &lastActivityAt, &inFlightTools, &idleWatchdogFired, &idleWatchdogThreshold, agentCancel, session.Messages, taskLog, taskID)
	}

	// drainFinished closes after the drain goroutine has flushed the last
	// message batch, so the result hand-off below can wait for the transcript
	// tail to be persisted.
	drainFinished := make(chan struct{})
	go func() {
		defer close(drainFinished)
		var mu sync.Mutex
		var pendingText attributedTextBuffer
		var pendingThinking attributedTextBuffer
		var batch []TaskMessageData
		callIDToTool := map[string]string{}

		flush := func() {
			mu.Lock()
			if pendingThinking.content.Len() > 0 {
				s := msgSeq.Add(1)
				content, sources, citations := pendingThinking.drain()
				batch = append(batch, TaskMessageData{
					Seq:       int(s),
					Type:      "thinking",
					Content:   content,
					Sources:   sources,
					Citations: citations,
				})
			}
			if pendingText.content.Len() > 0 {
				s := msgSeq.Add(1)
				content, sources, citations := pendingText.drain()
				batch = append(batch, TaskMessageData{
					Seq:       int(s),
					Type:      "text",
					Content:   content,
					Sources:   sources,
					Citations: citations,
				})
			}
			toSend := batch
			batch = nil
			mu.Unlock()

			if len(toSend) > 0 {
				sendCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
				if err := p.client.ReportTaskMessages(sendCtx, taskID, toSend); err != nil {
					taskLog.Debug("failed to report task messages", "error", err)
				} else {
					taskLog.Debug("reported task messages", "count", len(toSend), "last_seq", toSend[len(toSend)-1].Seq)
				}
				cancel()
			}
		}

		ticker := time.NewTicker(500 * time.Millisecond)
		defer ticker.Stop()

		done := make(chan struct{})
		tickerDone := make(chan struct{})
		go func() {
			defer close(tickerDone)
			for {
				select {
				case <-ticker.C:
					flush()
				case <-done:
					return
				}
			}
		}()

		var sessionPinned atomic.Bool
		for {
			select {
			case msg, ok := <-session.Messages:
				if !ok {
					goto drainDone
				}
				// Stamp activity as soon as a message lands. The idle
				// watchdog reads this to decide whether the backend has
				// gone silent — stamping before processing makes sure a
				// slow downstream call (mu.Lock contention, batch resize)
				// can't be misattributed to backend silence.
				lastActivityAt.Store(time.Now().UnixNano())
				switch msg.Type {
				case agent.MessageStatus:
					// Persist the session/work_dir as soon as the backend
					// reveals them. Without this, a daemon crash mid-run
					// loses the resume pointer and the auto-retry fires
					// without context.
					// rollout is actually in the store, so a crash-recovery pointer
					// the daemon cannot resume never poisons the next follow-up
					// (FailAgentTask keeps the pinned session_id via COALESCE, so a
					// bad mid-flight pin survives a later terminal failure). Codex
					// reveals the session id on a single task_started status, so a
					// background waiter polls for the rollout for the life of the
					// run and pins the moment it lands — a rollout that flushes
					// after this status is still pinned in-flight (crash recovery
					// preserved), while a session whose rollout never lands is never
					// pinned. The terminal report is the authoritative writer.
					// Non-Codex providers (codexHome == "") pin immediately.
					if msg.SessionID != "" && !sessionPinned.Swap(true) {
						sid := msg.SessionID
						wd := opts.Cwd
						go func() {
							if !waitCodexRolloutPresent(drainCtx, codexHome, sid) {
								taskLog.Debug("skip pinning codex session: rollout not present before run ended",
									"session_id", sid, "codex_home", codexHome)
								return
							}
							pinCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
							defer cancel()
							if err := p.client.PinTaskSession(pinCtx, taskID, sid, wd); err != nil {
								taskLog.Debug("pin session failed", "error", err)
							}
						}()
					}
				case agent.MessageToolUse:
					n := toolCount.Add(1)
					inFlightTools.Add(1)
					taskLog.Info(fmt.Sprintf("tool #%d: %s", n, msg.Tool))
					if msg.CallID != "" {
						mu.Lock()
						callIDToTool[msg.CallID] = msg.Tool
						mu.Unlock()
					}
					s := msgSeq.Add(1)
					mu.Lock()
					batch = append(batch, TaskMessageData{
						Seq:    int(s),
						Type:   "tool_use",
						Tool:   msg.Tool,
						CallID: msg.CallID,
						State:  taskMessageState(msg),
						// Redact before the payload leaves this process, not
						// only on arrival. The server redacts again in its
						// ingest handler, but that is the *remote* side: a
						// daemon that self-updated ahead of the server — or one
						// talking to a server mid-rollout — would otherwise ship
						// whole-file edit contents (a deleted .env, a patched
						// credential) to a peer that does not scrub nested
						// values yet. Deployment order is not a control we
						// have, so this side has to be safe on its own.
						Input: redact.InputMap(msg.Input),
					})
					mu.Unlock()
				case agent.MessageToolResult:
					// Decrement only when the count would stay >= 0. A stray
					// tool_result with no matching tool_use (backend bug or
					// reconnect mid-stream) shouldn't push the counter
					// negative — that would re-arm the watchdog one tool_use
					// too early on the next call.
					for {
						cur := inFlightTools.Load()
						if cur <= 0 {
							break
						}
						if inFlightTools.CompareAndSwap(cur, cur-1) {
							break
						}
					}
					s := msgSeq.Add(1)
					output := msg.Output
					if len(output) > 8192 {
						output = output[:8192]
					}
					toolName := msg.Tool
					if toolName == "" && msg.CallID != "" {
						mu.Lock()
						toolName = callIDToTool[msg.CallID]
						mu.Unlock()
					}
					taskLog.Info("tool_result observed", "seq", s, "tool", toolName, "call_id", msg.CallID)
					mu.Lock()
					batch = append(batch, TaskMessageData{
						Seq:    int(s),
						Type:   "tool_result",
						Tool:   toolName,
						Output: output,
						CallID: msg.CallID,
						State:  taskMessageState(msg),
					})
					mu.Unlock()
				case agent.MessageThinking:
					if msg.Content != "" {
						mu.Lock()
						pendingThinking.append(msg)
						mu.Unlock()
					}
				case agent.MessageText:
					if msg.Content != "" {
						taskLog.Debug("agent", "text", truncateLog(msg.Content, 200))
						mu.Lock()
						pendingText.append(msg)
						mu.Unlock()
					}
				case agent.MessageError:
					taskLog.Error("agent error", "content", msg.Content)
					s := msgSeq.Add(1)
					mu.Lock()
					batch = append(batch, TaskMessageData{
						Seq:     int(s),
						Type:    "error",
						Content: msg.Content,
					})
					mu.Unlock()
				}
			case <-drainCtx.Done():
				goto drainDone
			}
		}
	drainDone:
		close(done)
		// Let any tick-driven flush finish before the final one: a flush still
		// in flight would otherwise keep posting batches after this goroutine
		// signalled that the transcript tail was persisted.
		<-tickerDone
		flush()
	}()

	// waitForDrain blocks until the drain goroutine has flushed the transcript
	// tail, so every terminal return below hands control back only after the
	// task's reported messages are persisted — a consumer reading them at the
	// terminal transition would otherwise see a transcript that is non-empty
	// but truncated, indistinguishable from a complete one. Bounded so a
	// backend that never closes its message channel cannot stall the terminal
	// transition: after 10s the drain loop is cancelled and given a window
	// wide enough for its worst-case exit — an in-flight tick flush plus the
	// final one, each capped by the 5s ReportTaskMessages timeout and neither
	// interruptible by the cancel (they post on context.Background()).
	waitForDrain := func() {
		select {
		case <-drainFinished:
		case <-time.After(10 * time.Second):
			drainCancel()
			select {
			case <-drainFinished:
			case <-time.After(12 * time.Second):
				taskLog.Warn("transcript drain did not stop after cancel; completing anyway")
			}
		}
	}

	select {
	case result := <-session.Result:
		waitForDrain()
		if idleWatchdogFired.Load() {
			// The backend's wait goroutine (e.g. claude.go) translates the
			// SIGKILL we delivered via agentCancel into Status="aborted".
			// Re-tag it as "idle_watchdog" so runTask routes the
			// disposition through a dedicated failure_reason, not the
			// generic "agent_error" bucket the aborted path falls into.
			result.Status = "idle_watchdog"
			if result.Error == "" {
				result.Error = idleWatchdogReason(time.Duration(idleWatchdogThreshold.Load()))
			}
		}
		return result, toolCount.Load(), nil
	case <-drainCtx.Done():
		// The drain loop is exiting on this same Done signal; wait for its
		// final flush so the timeout/watchdog/cancel terminals below cannot
		// hand back (and let runTask fail-and-broadcast) a still-flushing
		// transcript either.
		waitForDrain()
		// Idle watchdog cancels via agentCancel(), which propagates here as
		// context.Canceled. Check this BEFORE the generic cancelled/timeout
		// classifiers so a watchdog-induced stop isn't misreported as
		// "task cancelled by server".
		if idleWatchdogFired.Load() {
			return agent.Result{
				Status: "idle_watchdog",
				Error:  idleWatchdogReason(time.Duration(idleWatchdogThreshold.Load())),
			}, toolCount.Load(), nil
		}
		// Distinguish external cancellation (e.g. server-initiated cancel
		// because the issue was reassigned, or the user invoked CancelTask)
		// from genuine drain-deadline timeouts. context.Canceled means the
		// upstream runCtx fired runCancel(); context.DeadlineExceeded is the
		// drain deadline expiring on its own.
		if errors.Is(drainCtx.Err(), context.Canceled) {
			return agent.Result{
				Status: "cancelled",
				Error:  "task cancelled by upstream context (server cancel or daemon shutdown)",
			}, toolCount.Load(), nil
		}
		return agent.Result{
			Status: "timeout",
			Error:  "agent did not produce result within drain timeout",
		}, toolCount.Load(), nil
	}
}

// idleWatchdogReason formats the human-facing explanation surfaced on
// idle_watchdog dispositions. Centralised so the result-arrival branch and the
// drain-timeout branch in executeAndDrain emit identical wording.
func idleWatchdogReason(window time.Duration) string {
	return fmt.Sprintf("agent produced no new messages for %s and message queue was empty; force-stopped by idle watchdog", window)
}

// idleWatchdogTickInterval picks how often the idle watchdog re-checks the
// silence budget. Half the window is the base rate, capped at
// idleWatchdogMaxTick so a run is force-stopped within window + tick rather
// than window * 1.5. Sub-nanosecond halves fall back to the window itself so
// tests can pass tiny budgets and still get a valid ticker.
//
// There used to be a `window >= time.Minute && interval < 30*time.Second` floor
// here, meant to keep production polling no faster than 30 s. It was
// unreachable — window >= 1 min implies window/2 >= 30 s — and its only effect
// was to make the tests around it read as if a floor were being exercised.
// Production windows are minutes or hours, so window/2 already clears 30 s.
func idleWatchdogTickInterval(window time.Duration) time.Duration {
	interval := window / 2
	if interval > idleWatchdogMaxTick {
		interval = idleWatchdogMaxTick
	}
	if interval <= 0 {
		interval = window
	}
	return interval
}

// runIdleWatchdog ticks until either agentCtx is cancelled or the backend has
// been silent past the applicable budget. On firing, it records the tripped
// threshold, sets fired, and calls cancel, which propagates to the agent
// subprocess (via the ctx passed to backend.Execute) and to drainCtx. The
// silence budget depends on whether a tool call is in flight:
//
//  1. No tool in flight — a silent backend is a hang after `window`.
//  2. A tool in flight (tool_use with no matching tool_result yet) —
//     `toolWindow` applies instead. It defaults to `window`, so the two are
//     normally identical and this branch only changes which duration the
//     failure message reports; an operator who deliberately sets
//     ORVILO_AGENT_TOOL_WATCHDOG higher buys long tools extra room, and
//     toolWindow <= 0 keeps the historical behavior of never force-stopping
//     while a tool is in flight. Without this in-flight budget a backend that
//     emits tool_use and never the matching tool_result would run forever now
//
// In both cases the watchdog also requires the session.Messages buffer to be
// empty — a buffered-but-undrained message means the drain loop is behind, not
// the backend.
//
// Polling rate comes from idleWatchdogTickInterval, so a run is force-stopped
// somewhere between its budget and budget + tick, never earlier.
func (p *providerExecution) runIdleWatchdog(agentCtx context.Context, window, toolWindow time.Duration, lastActivityAt *atomic.Int64, inFlightTools *atomic.Int32, fired *atomic.Bool, firedThreshold *atomic.Int64, cancel context.CancelFunc, messages <-chan agent.Message, taskLog *slog.Logger, taskID string) {
	ticker := time.NewTicker(idleWatchdogTickInterval(window))
	defer ticker.Stop()
	for {
		select {
		case <-agentCtx.Done():
			return
		case <-ticker.C:
			// Pick the silence budget. A tool in flight is expected to be
			// silent (a long build/install/test emits nothing between
			// tool_use and tool_result), so it gets the larger toolWindow;
			// toolWindow <= 0 disables the in-flight bound entirely.
			threshold := window
			toolInFlight := inFlightTools.Load() > 0
			if toolInFlight {
				if toolWindow <= 0 {
					continue
				}
				threshold = toolWindow
			}
			last := time.Unix(0, lastActivityAt.Load())
			idleFor := time.Since(last)
			if idleFor < threshold {
				continue
			}
			// A buffered-but-undrained message means the drain loop is
			// behind, not the backend. Wait one more tick rather than
			// killing a backend that is still producing output.
			if len(messages) > 0 {
				continue
			}
			taskLog.Warn("idle watchdog firing: no agent activity, force-stopping run",
				"task", shortID(taskID),
				"idle_for", idleFor.Round(time.Second).String(),
				"threshold", threshold.String(),
				"tool_in_flight", toolInFlight,
			)
			firedThreshold.Store(int64(threshold))
			fired.Store(true)
			cancel()
			return
		}
	}
}

func mergeUsage(a, b map[string]agent.TokenUsage) map[string]agent.TokenUsage {
	if len(a) == 0 {
		return b
	}
	if len(b) == 0 {
		return a
	}
	merged := make(map[string]agent.TokenUsage, len(a)+len(b))
	for model, u := range a {
		merged[model] = u
	}
	for model, u := range b {
		existing := merged[model]
		existing.InputTokens += u.InputTokens
		existing.OutputTokens += u.OutputTokens
		existing.CacheReadTokens += u.CacheReadTokens
		existing.CacheWriteTokens += u.CacheWriteTokens
		existing.CostUSDTicks += u.CostUSDTicks
		merged[model] = existing
	}
	return merged
}
