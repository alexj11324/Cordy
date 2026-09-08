package daemon

import (
	"context"
	"errors"
	"fmt"
	"hash/fnv"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/orvilo-ai/orvilo/server/pkg/agent"
	"golang.org/x/sync/errgroup"
)

// setAgentVersion records the detected CLI version for an agent provider so
// later task-dispatch code (e.g. Codex sandbox policy) can read it.
//
// A blank detection never replaces a version we already know. checkAgentMinVersion
// returns nil for any provider with no MinVersions entry, so a CLI whose
// `--version` exits 0 without printing anything parseable reaches here with an
// empty string — and overwriting the cache with it would strip the input every
// version-keyed policy reads, on a provider that was working a moment ago.
// "Couldn't read it" is not a version.
//
// This cache is the daemon's LOCAL knowledge only — it is not a record of
// what the server has been told. Every path that probes writes through here
// (the converge round, the workspace sync, a self-heal at task launch), while
// each register call covers only the workspaces it was invoked for. What the
// server accepted is therefore tracked per workspace, in
// workspaceState.builtinVersions; refreshAgentVersions compares the two.
func (d *Daemon) setAgentVersion(provider, version string) {
	d.versionsMu.Lock()
	prev := d.agentVersions[provider]
	if version == "" && prev != "" {
		d.versionsMu.Unlock()
		d.logger.Warn("agent CLI reported no version; keeping the previous one",
			"provider", provider, "previous", prev)
		return
	}
	d.agentVersions[provider] = version
	d.versionsMu.Unlock()
}

// agentVersion returns the last-detected CLI version for an agent provider,
// or an empty string if unknown.
func (d *Daemon) agentVersion(provider string) string {
	d.versionsMu.RLock()
	defer d.versionsMu.RUnlock()
	return d.agentVersions[provider]
}

// builtinVersionsFromPayload extracts provider -> version from a registration
// payload's BUILT-IN entries. Custom profile entries (profile_id set) are not
// version-tracked — the drift path owns their lifecycle.
func builtinVersionsFromPayload(runtimes []map[string]string) map[string]string {
	out := make(map[string]string, len(runtimes))
	for _, rt := range runtimes {
		if rt["profile_id"] != "" {
			continue
		}
		out[rt["type"]] = rt["version"]
	}
	return out
}

// workspaceRegisterLock returns the mutex serializing one workspace's whole
// registration sequence: send Register, apply or reject the response, then
// deregister the rows that apply refused or dropped.
//
// The per-workspace version record (workspaceState.builtinVersions) must
// reflect the order the SERVER processed the register calls in, because the
// server's upsert order decides which payload's versions its rows end up
// holding. Registration entry points are concurrent (refresh, sync, a
// runtime_gone recovery, profile drift), and without this lock two calls for
// the same workspace could complete their HTTP responses in the opposite
// order from the server's processing — recording the NEWER payload locally
// while the server kept the OLDER one. The refresh round would then see the
// record agreeing with disk and never re-register: the mismatch is invisible
// precisely because the record is wrong, so it persists until the next
// version change or restart. Holding the lock across the request AND the
// record makes lock order = server order = record order.
//
// The section extends past the send because the cleanup has the same problem in
// a nastier form. A deregistration is decided under d.mu and issued after
// releasing it, so a recovery register completing in that gap re-creates the
// same row — usually under the same runtime ID — and the older Deregister lands
// on top of it. The daemon then tracks and heartbeats a runtime the server has
// marked offline, which is the direction that silently strands work: the server
// will not route to it, and neither side notices the disagreement. A
// point-in-time tracking re-check cannot close that, because the gap is between
// the check and the request; only putting the request itself inside the order
// can. Every apply and every cleanup on a workspace therefore runs under this
// lock, via withWorkspaceRegisterLock.
func (d *Daemon) workspaceRegisterLock(workspaceID string) *sync.Mutex {
	mu, _ := d.registerSerial.LoadOrStore(workspaceID, &sync.Mutex{})
	return mu.(*sync.Mutex)
}

// withWorkspaceRegisterLock runs one workspace's registration sequence as a
// single ordered step. Everything that sends a Register for a workspace, folds
// the response into local state, or deregisters rows that response cost, must
// run inside fn — see workspaceRegisterLock for why the boundary sits after the
// cleanup rather than after the send.
//
// The lock is per workspace, so sequences for different workspaces still run
// concurrently and nothing here is ever held across two of them.
func (d *Daemon) withWorkspaceRegisterLock(workspaceID string, fn func() error) error {
	mu := d.workspaceRegisterLock(workspaceID)
	mu.Lock()
	defer mu.Unlock()
	return fn()
}

// recordBuiltinVersionsSent stores, per provider, the version a SUCCESSFUL
// register call carried for a workspace (see workspaceState.builtinVersions).
// Merged per provider rather than replaced: a provider absent from this
// payload (its probe failed this round) was not re-sent, so the server still
// holds whatever the previous call carried and the record must keep saying so.
// Callers invoke this only after client.Register succeeds — a failed call
// records nothing, which is what keeps the workspace behind for the next
// refresh round — and while holding the workspace's register lock, so the
// record's write order matches the server's processing order
// (workspaceRegisterLock). A workspace not yet tracked records nothing here;
// the sync path seeds the record when it creates the workspaceState.
func (d *Daemon) recordBuiltinVersionsSent(workspaceID string, runtimes []map[string]string) {
	sent := builtinVersionsFromPayload(runtimes)
	if len(sent) == 0 {
		return
	}
	d.mu.Lock()
	defer d.mu.Unlock()
	ws, ok := d.workspaces[workspaceID]
	if !ok {
		return
	}
	if ws.builtinVersions == nil {
		ws.builtinVersions = make(map[string]string, len(sent))
	}
	for provider, version := range sent {
		// Demoted while this register was in flight: its rows are being
		// deregistered, so recording what the payload carried would tell the
		// refresh path the server holds a version for a provider that is on its
		// way offline — and demoteBelowMinimumRuntimes just deleted that entry.
		if d.providerDemotedLocked(provider) {
			continue
		}
		ws.builtinVersions[provider] = version
	}
}

// refreshHealedVersion keeps a self-healed {path, version} pair honest when the
// healed binary is later replaced in place.
//
// resolveAgentEntry deliberately returns healed.version rather than the shared
// agentVersions cache whenever a previous self-heal is still live, so that a
// reader which observes the healed path necessarily observes the version
// detected for it. Nothing refreshed that pair when the SAME path was
// subsequently overwritten, so the daemon kept keying version-sensitive policy
// — the Codex sandbox in runTask — off the version captured at heal time. A
// re-probe would update agentVersions and the version reported to the server
// while tasks silently kept running under the old policy.
//
// Only touches the entry when the path still matches the one just probed: a
// heal that has since moved the provider elsewhere owns its own pairing.
func (d *Daemon) refreshHealedVersion(provider, path, version string) {
	if version == "" {
		return
	}
	d.resolvedPathsMu.Lock()
	defer d.resolvedPathsMu.Unlock()
	healed, ok := d.resolvedPaths[provider]
	if !ok || healed.path != path || healed.version == version {
		return
	}
	d.resolvedPaths[provider] = healedAgent{path: path, version: version}
}

// hasDetectedAgentVersions reports whether any CLI version has ever been
// detected. refreshAgentVersions uses it as the "has anything ever registered"
// guard.
func (d *Daemon) hasDetectedAgentVersions() bool {
	d.versionsMu.RLock()
	defer d.versionsMu.RUnlock()
	return len(d.agentVersions) > 0
}

// healedAgent bundles a self-healed executable path with the CLI version
// detected for it. The two are published together under resolvedPathsMu and
// returned together from resolveAgentEntry, so a caller that observes the new
// path necessarily observes the matching version — closing the window where a
// just-upgraded binary would run under the daemon's previous version policy
// (MUL-4486 review).
type healedAgent struct {
	path    string
	version string
}

// resolveAgentEntry returns entry with a usable executable path plus the CLI
// version that corresponds to that path. It resolves retargetable Windows
// installer junctions per launch and self-heals vanished pinned paths on other
// platforms (MUL-4486).
//
// The daemon pins each agent's discovered entry point at startup so a later
// PATH change cannot redirect a task launch. POSIX discovery also resolves
// symlinks to a concrete path. But a version manager
// (Homebrew Cask, nvm/fnm) upgrading in place deletes the old versioned
// directory that pinned path points into and repoints the stable command name
// at the new version — leaving the daemon holding a path that no longer exists
// until it restarts. Every consumer of entry.Path (task launch, model listing,
// version detection at registration) then hard-fails with "executable not
// found".
//
// The returned version always matches the returned path, so callers key
// version-sensitive policy (e.g. the Codex sandbox) off it directly rather than
// re-reading the shared version cache, which a concurrent heal could still be
// updating.
//
// Behaviour:
//   - A Windows stable entry point -> its final target is verified and returned
//     with the detected version; a retarget publishes the new pair atomically.
//   - A previous self-heal that is still live -> returned with its paired
//     version. This is checked first so that once we've re-resolved to a new
//     binary, a reappearing stale path (a downgrade / reinstall recreating the
//     old versioned directory) cannot re-pair that old binary with the healed
//     version — a mismatched {old path, new version} (MUL-4486 review).
//   - Otherwise, pinned Path still present -> returned unchanged, paired with
//     its registration-detected version. The anti-redirect guarantee holds for
//     the normal (never-healed) case: a live pinned binary is never
//     second-guessed even if PATH now points elsewhere.
//   - Pinned Path gone and no live heal -> re-resolve entry.Command once
//     (preserving the ~/.patchbay/hooks exclusion and the login-shell fallback).
//     Before adopting the re-resolved binary it is version-detected and run
//     through the same minimum-version gate registration applies. This
//     reproduces exactly what a daemon restart would resolve, so it is no less
//     safe than the documented restart workaround — only automatic.
//   - Re-resolution fails, the candidate can't be version-detected, or it is
//     below the minimum supported version -> entry is returned unchanged so the
//     candidate is never launched and the downstream error still surfaces.
func (d *Daemon) resolveAgentEntry(ctx context.Context, provider string, entry AgentEntry) (AgentEntry, string) {
	resolved, version, _ := d.resolveAgentEntryWithHeal(ctx, provider, entry)
	return resolved, version
}

// resolveAgentEntryForLaunch is the strict task-launch boundary. Windows
// installer junctions must yield a verified final target before the first
// launch; otherwise the stable entry could retarget after registration and run
// a binary whose version and minimum-version policy were never checked.
func (d *Daemon) resolveAgentEntryForLaunch(ctx context.Context, provider string, entry AgentEntry) (AgentEntry, string, error) {
	resolved, version, outcome := d.resolveAgentEntryWithHeal(ctx, provider, entry)
	if outcome.rejected != nil {
		return entry, d.agentVersion(provider), outcome.rejected
	}
	if outcome.failure != nil {
		return entry, d.agentVersion(provider), fmt.Errorf("resolve agent executable %q for launch: %w", entry.Path, outcome.failure)
	}
	if outcome.adopted.path != "" {
		return resolved, version, nil
	}
	return resolved, version, nil
}

// healOutcome is what one self-heal attempt concluded. At most one half is
// meaningful: adopted names a binary that cleared the same gates registration
// applies, while rejected carries the typed verdict for a candidate that was
// found and version-detected but refused for being below the minimum supported
// version. rejected is nil unless the verdict is genuine — it is only ever set
// from a *agent.BelowMinimumError, which by construction carries a version
// that parsed.
type healOutcome struct {
	adopted  healedAgent
	rejected *agent.BelowMinimumError
	failure  error
}

// resolveAgentEntryWithHeal is resolveAgentEntry plus what the self-heal
// concluded, for the one caller that must act on a refusal rather than just
// decline to launch it.
//
// A refusal is a verdict about disk, not a transient failure: the pinned path
// is gone AND the binary its command now resolves to is too old. Registration
// needs to hear that, because otherwise it goes on to probe the vanished path,
// fails, and reports "version detection failed" — which by design leaves the
// runtime online, claiming tasks for a CLI that cannot launch.
func (d *Daemon) resolveAgentEntryWithHeal(ctx context.Context, provider string, entry AgentEntry) (AgentEntry, string, healOutcome) {
	// Windows installer entry points are stable junctions whose final target can
	// change while the old release remains installed. Resolve the final path on
	// every launch and adopt a changed target only after pairing it with a freshly
	// detected, supported version. Other platforms return handled=false and keep
	// the existing pinned-path self-heal semantics below.
	var launchOutcome healOutcome
	if launchPath, handled, err := executablePathForLaunch(entry.Path); handled {
		if err != nil {
			d.logger.Warn("resolve agent executable for launch failed; keeping discovered path",
				"provider", provider, "path", entry.Path, "error", err)
			launchOutcome.failure = err
		} else if outcome, ok := d.resolveAgentLaunchTarget(ctx, provider, entry, launchPath); ok {
			entry.Path = outcome.adopted.path
			return entry, outcome.adopted.version, outcome
		} else {
			launchOutcome = outcome
		}
	}

	// A prior self-heal wins over the original pinned path: it carries a
	// {path, version} pair we already verified together, so it can never regress
	// to the mismatched pairing a reappearing stale path would produce.
	d.resolvedPathsMu.RLock()
	healed, ok := d.resolvedPaths[provider]
	d.resolvedPathsMu.RUnlock()
	if ok && agentExecutablePresent(healed.path) {
		entry.Path = healed.path
		launchOutcome.adopted = healed
		return entry, healed.version, launchOutcome
	}

	if agentExecutablePresent(entry.Path) {
		return entry, d.agentVersion(provider), launchOutcome
	}

	if entry.Command == "" {
		return entry, d.agentVersion(provider), healOutcome{}
	}

	// Coalesce concurrent heals for the same provider: the first task through
	// pays for the re-resolve + version detection, the rest share its result
	// instead of each spawning their own login shell and `--version` probe.
	command := entry.Command
	v, _, _ := d.healGroup.Do(provider, func() (any, error) {
		return d.healAgentPath(ctx, provider, command), nil
	})
	outcome, _ := v.(healOutcome)
	if outcome.adopted.path == "" {
		return entry, d.agentVersion(provider), outcome
	}
	entry.Path = outcome.adopted.path
	return entry, outcome.adopted.version, outcome
}

// resolveAgentLaunchTarget handles platforms whose stable discovered entry
// point can retarget a different still-live executable. It returns ok only
// when a verified {path, version} pair is available; a rejected or transiently
// unreadable target falls through to the existing path handling so it is never
// published under a stale version.
func (d *Daemon) resolveAgentLaunchTarget(ctx context.Context, provider string, entry AgentEntry, launchPath string) (healOutcome, bool) {
	const maxRetargetAttempts = 4
	for attempt := 0; attempt < maxRetargetAttempts; attempt++ {
		d.resolvedPathsMu.RLock()
		cached, cachedOK := d.resolvedPaths[provider]
		d.resolvedPathsMu.RUnlock()
		if cachedOK && cached.path == launchPath && agentExecutablePresent(cached.path) {
			return healOutcome{adopted: cached}, true
		}

		// Coalesce only callers that observed the same concrete release. A
		// provider-only key can make a post-retarget caller inherit the previous
		// release's result even though both files remain present.
		key := provider + "\x00" + launchPath
		v, _, _ := d.healGroup.Do(key, func() (any, error) {
			d.resolvedPathsMu.RLock()
			current, ok := d.resolvedPaths[provider]
			d.resolvedPathsMu.RUnlock()
			if ok && current.path == launchPath && agentExecutablePresent(current.path) {
				return healOutcome{adopted: current}, nil
			}
			return d.adoptAgentPath(ctx, provider, entry.Command, launchPath, "resolved stable entry point for launch"), nil
		})
		outcome, _ := v.(healOutcome)

		// The installer may retarget while version detection is running. Resolve
		// again before returning and retry against the target visible now.
		currentPath, _, err := executablePathForLaunch(entry.Path)
		if err != nil {
			if outcome.adopted.path != "" {
				return outcome, true
			}
			outcome.failure = err
			return outcome, false
		}
		if currentPath != launchPath {
			launchPath = currentPath
			continue
		}
		if outcome.adopted.path != "" {
			return outcome, true
		}
		if cachedOK && agentExecutablePresent(cached.path) {
			outcome.adopted = cached
			return outcome, true
		}
		return outcome, false
	}

	return healOutcome{failure: errors.New("installer entry point changed repeatedly while resolving it")}, false
}

// healAgentPath re-resolves command for provider and, if a usable binary is
// found, records it and returns it. "Usable" means: it resolves, its version
// can be detected, and that version meets the same minimum-version gate
// registration enforces. Path and version are published together under
// resolvedPathsMu so any observer of the path also sees the matching version;
// the shared d.agentVersion cache is refreshed too, for registration hygiene.
// It returns a zero adopted pair when nothing usable was found, so the caller
// keeps the (stale) pinned entry and the candidate is never launched. Runs
// under healGroup, one invocation at a time per provider.
//
// A candidate refused by the minimum-version gate is reported back rather than
// swallowed: not launching it is right, but it is also the whole verdict a
// registration round needs to take the provider's runtimes offline.
func (d *Daemon) healAgentPath(ctx context.Context, provider, command string) healOutcome {
	// Re-check the cache: a predecessor under the same singleflight key may have
	// already populated it, or a prior heal completed between the read above and
	// entering here.
	d.resolvedPathsMu.RLock()
	cached, ok := d.resolvedPaths[provider]
	d.resolvedPathsMu.RUnlock()
	if ok && agentExecutablePresent(cached.path) {
		return healOutcome{adopted: cached}
	}

	newPath, found := reresolveAgentCommand(command)
	if !found {
		return healOutcome{}
	}
	if launchPath, handled, err := executablePathForLaunch(newPath); handled {
		if err != nil {
			d.logger.Warn("resolve re-discovered agent executable for launch failed; keeping discovered path",
				"provider", provider, "path", newPath, "error", err)
			return healOutcome{failure: err}
		} else {
			newPath = launchPath
		}
	}
	return d.adoptAgentPath(ctx, provider, command, newPath, "re-resolved after pinned path vanished")
}

func (d *Daemon) adoptAgentPath(ctx context.Context, provider, command, newPath, reason string) healOutcome {
	// Verify before adopting. An in-place "upgrade" that actually repoints at an
	// older or broken install must not be launched under the daemon's stale
	// version policy, and must not slip past the minimum-version gate that the
	// registration path applies (MUL-4486 review).
	version, err := detectAgentVersion(ctx, agent.Command{Path: newPath})
	if err != nil {
		d.logger.Warn("re-resolved agent executable failed version detection; keeping pinned path",
			"provider", provider, "command", command, "new_path", newPath, "error", err)
		return healOutcome{failure: err}
	}
	if err := checkAgentMinVersion(provider, version); err != nil {
		var tooOld *agent.BelowMinimumError
		if !errors.As(err, &tooOld) {
			// Read something, understood nothing: not a verdict. Refusing to
			// adopt is still right, but reporting a rejection would let the
			// caller demote a runtime on an unreadable version — the exact
			// transient case the below-minimum machinery must never act on.
			d.logger.Warn("re-resolved agent executable version could not be validated; keeping pinned path",
				"provider", provider, "command", command, "new_path", newPath, "version", version, "error", err)
			return healOutcome{failure: err}
		}
		d.logger.Warn("re-resolved agent executable is below the minimum supported version; not adopting it",
			"provider", provider, "command", command, "new_path", newPath, "version", version, "error", err)
		return healOutcome{rejected: tooOld}
	}

	adopted := healedAgent{path: newPath, version: version}
	// Publish path + version atomically: any reader that sees the new path in
	// resolveAgentEntry gets the matching version out of the same struct value.
	d.resolvedPathsMu.Lock()
	if d.resolvedPaths == nil {
		d.resolvedPaths = make(map[string]healedAgent)
	}
	d.resolvedPaths[provider] = adopted
	d.resolvedPathsMu.Unlock()
	// Keep the registration version cache fresh too. The task path reads the
	// version returned alongside the resolved path (above), not this map, so its
	// staleness can never gate a launch — this is hygiene for the registration
	// report and any future d.agentVersion reader.
	d.setAgentVersion(provider, version)

	d.logger.Info("adopted resolved agent executable",
		"provider", provider, "command", command, "new_path", newPath, "version", version, "reason", reason)
	return healOutcome{adopted: adopted}
}

func (d *Daemon) notifyRuntimeSetChanged() {
	d.runtimeSet.notify()
}

// reregisterCoalesceWindow caps how often the daemon re-registers a workspace
// after detecting a runtime_not_found response. Many stale runtime IDs may be
// reported within seconds of each other (one delete clears all of a daemon's
// runtimes), and a single re-register call replaces every runtime in the
// workspace, so concurrent recoveries must collapse to one API call.
const reregisterCoalesceWindow = 30 * time.Second

// reregisterFailureBackoff is the additional wait inserted before the next
// re-register attempt when the previous one failed. This prevents heartbeat
// ticks (~15s) from converting a server-side log flood into a re-register
// flood when re-registration itself is failing (workspace removed, server
// unreachable, ...).
const reregisterFailureBackoff = 60 * time.Second

// handleRuntimeGone is the single recovery entry point shared by the HTTP
// heartbeat path, the runtime poller, and the WebSocket runtime_gone ack
// handler. All three may notice the same stale runtime within a few ms of
// each other, so this function:
//
//   - keys an in-flight set on runtimeID to drop concurrent calls for the same
//     ID after the first one is already cleaning up;
//   - keys a per-workspace next-attempt timestamp on workspaceID so that
//     concurrent recoveries triggered by the SAME initial event coalesce to a
//     single registerRuntimesForWorkspace call. The slot is cleared on success
//     so a later distinct runtime deletion in the same workspace can trigger
//     its own recovery without waiting for the coalesce window to expire; and
//   - keys a per-workspace last-completed timestamp so that a straggler whose
//     removeStaleRuntime took long enough that a sibling fully ran AND cleared
//     the slot can still recognize itself as same-wave and bail. Without this,
//     the success-case slot clear opens a race where the late caller re-claims
//     an empty slot and double-registers.
//
// On failure of the underlying re-register, the next-attempt timestamp is
// extended by reregisterFailureBackoff so we don't replace a server-side log
// flood with a daemon-side register flood. workspaceSyncLoop will retry
// independently every DefaultWorkspaceSyncInterval as a safety net.
//
// The recovery HTTP call uses the daemon root context, not the caller's. The
// heartbeat path's per-runtime ctx is cancelled by notifyRuntimeSetChanged the
// moment we prune the dead UUID, and if we forwarded that ctx the in-flight
// register would self-cancel mid-flight.
func (d *Daemon) handleRuntimeGone(runtimeID string) {
	if runtimeID == "" {
		return
	}

	// entryAt anchors the same-wave-straggler check at the bottom of the
	// function. Captured at the very top so removeStaleRuntime mutex
	// contention can't push it past a sibling's register completion.
	entryAt := time.Now()

	// Stampede control per runtime ID.
	d.runtimeGoneMu.Lock()
	if _, inflight := d.runtimeGoneInflight[runtimeID]; inflight {
		d.runtimeGoneMu.Unlock()
		return
	}
	d.runtimeGoneInflight[runtimeID] = struct{}{}
	d.runtimeGoneMu.Unlock()
	defer func() {
		d.runtimeGoneMu.Lock()
		delete(d.runtimeGoneInflight, runtimeID)
		d.runtimeGoneMu.Unlock()
	}()

	workspaceID, removed := d.removeStaleRuntime(runtimeID)
	if !removed {
		// Already gone from local state — a parallel recovery already
		// cleaned this up, or workspaceSyncLoop pruned the whole workspace.
		return
	}

	d.logger.Info("runtime deleted server-side; pruned from local state",
		"runtime_id", runtimeID, "workspace_id", workspaceID)
	d.notifyRuntimeSetChanged()

	if !d.tryClaimRegisterSlot(workspaceID, entryAt, time.Now()) {
		d.logger.Debug("skip re-register: coalescing with recent attempt",
			"workspace_id", workspaceID)
		return
	}

	err := d.reregisterWorkspaceAfterRuntimeGone(d.recoveryContext(), workspaceID)
	d.recordRegisterCompletion(workspaceID, time.Now(), err)
	if err != nil {
		// Logged at Warn (not Error) because workspaceSyncLoop retries
		// independently every DefaultWorkspaceSyncInterval, so a transient
		// failure here is not a stuck state — just an extra wait.
		d.logger.Warn("re-register after runtime gone failed",
			"workspace_id", workspaceID, "error", err)
	}
}

// tryClaimRegisterSlot atomically decides whether the calling goroutine should
// run registerRuntimesForWorkspace. Returns true and claims the in-flight slot
// when the caller may proceed; returns false (without mutating state) when the
// call must be coalesced with a peer.
//
// Two gates are checked under runtimeGoneMu:
//
//  1. reregisterNextAttempt: a future timestamp means a peer holds the slot or
//     a previous attempt failed and we are inside the failure backoff window.
//  2. reregisterLastCompletedAt: a timestamp at or after our entryAt means a
//     peer's register SUCCEEDED after we entered handleRuntimeGone, so the
//     workspace state is already covered for our wave and we can bail.
//     Failures intentionally don't stamp this field (see
//     recordRegisterCompletion), so a same-wave straggler whose entryAt
//     predates a failed sibling can still retry once the failure backoff
//     expires — failures don't cover anything.
//
// entryAt is the wall-clock captured at the top of handleRuntimeGone. now is
// passed in (rather than read inside) so tests can drive the gate
// deterministically without sleeping.
func (d *Daemon) tryClaimRegisterSlot(workspaceID string, entryAt, now time.Time) bool {
	d.runtimeGoneMu.Lock()
	defer d.runtimeGoneMu.Unlock()
	if next, ok := d.reregisterNextAttempt[workspaceID]; ok && now.Before(next) {
		return false
	}
	if last, ok := d.reregisterLastCompletedAt[workspaceID]; ok && !last.Before(entryAt) {
		return false
	}
	d.reregisterNextAttempt[workspaceID] = now.Add(reregisterCoalesceWindow)
	return true
}

// recordRegisterCompletion records the outcome of a register call. On success
// it stamps lastCompletedAt (which suppresses same-wave stragglers via
// tryClaimRegisterSlot) and clears the in-flight slot so a genuinely later
// runtime deletion can claim immediately. On failure it extends
// reregisterNextAttempt by the failure backoff and intentionally does NOT
// stamp lastCompletedAt — a failed register did not cover any workspace
// state, so a same-wave straggler whose entryAt predates the failure must
// still be allowed to retry once the backoff expires. workspaceSyncLoop only
// retries when the workspace's runtimeIDs fully drain, so partial-deletion
// recovery has to come from the straggler path.
func (d *Daemon) recordRegisterCompletion(workspaceID string, completedAt time.Time, err error) {
	d.runtimeGoneMu.Lock()
	defer d.runtimeGoneMu.Unlock()
	if err != nil {
		d.reregisterNextAttempt[workspaceID] = completedAt.Add(reregisterFailureBackoff)
		return
	}
	d.reregisterLastCompletedAt[workspaceID] = completedAt
	delete(d.reregisterNextAttempt, workspaceID)
}

// recoveryContext returns the daemon root context for long-running recovery
// HTTP calls (re-register, recover-orphans) that must survive the heartbeat
// loop tearing down a per-runtime context. Falls back to Background when the
// daemon was not started via Run(), e.g. unit-test fixtures.
func (d *Daemon) recoveryContext() context.Context {
	if d.rootCtx != nil {
		return d.rootCtx
	}
	return context.Background()
}

// removeStaleRuntime drops a runtime ID from its owning workspace's runtimeIDs
// list, the daemon-level runtimeIndex, and the WS heartbeat freshness map.
// Returns the workspace ID and true if the runtime was tracked, "" and false
// otherwise.
//
// Callers must NOT replace workspaceState pointers — only mutate fields in
// place — because ensureRepoReady holds workspaceState.repoRefreshMu through
// long repo-sync calls. See syncWorkspacesFromAPI for the same invariant.
func (d *Daemon) removeStaleRuntime(runtimeID string) (string, bool) {
	d.mu.Lock()
	var workspaceID string
	for wsID, ws := range d.workspaces {
		found := false
		filtered := ws.runtimeIDs[:0:0]
		for _, rid := range ws.runtimeIDs {
			if rid == runtimeID {
				found = true
				continue
			}
			filtered = append(filtered, rid)
		}
		if found {
			ws.runtimeIDs = filtered
			workspaceID = wsID
			break
		}
	}
	if workspaceID == "" {
		d.mu.Unlock()
		return "", false
	}
	delete(d.runtimeIndex, runtimeID)
	d.mu.Unlock()

	d.wsHBMu.Lock()
	delete(d.wsHBLastAck, runtimeID)
	d.wsHBMu.Unlock()

	return workspaceID, true
}

// workspaceNeedsRuntimeRecovery reports whether a tracked workspace currently
// has zero runtime IDs — the state reached when handleRuntimeGone pruned every
// runtime and its inline re-register failed. workspaceSyncLoop calls this on
// each tick so the workspace can recover without waiting for an external
// trigger.
func (d *Daemon) workspaceNeedsRuntimeRecovery(workspaceID string) bool {
	d.mu.Lock()
	defer d.mu.Unlock()
	ws, ok := d.workspaces[workspaceID]
	if !ok {
		return false
	}
	return len(ws.runtimeIDs) == 0
}

// reregisterWorkspaceAfterRuntimeGone calls registerRuntimesForWorkspace and
// updates the existing workspaceState in place. The register response is
// authoritative for this workspace's runtime set — every configured provider
// is included, with UpsertAgentRuntime returning the same row ID for surviving
// providers and a fresh ID for any that were deleted server-side. Replacing
// (rather than appending) is required: a partial recovery, where only one
// runtime in a multi-provider workspace was deleted, would otherwise produce
// duplicates for every provider that wasn't deleted.
//
// The workspaceState pointer is NEVER replaced (see syncWorkspacesFromAPI's
// invariant about repoRefreshMu). Only fields are mutated.
// applyRegisterResponseInPlace folds a fresh /api/daemon/register response
// back into the workspaceState and runtimeIndex without replacing the
// workspaceState pointer (see syncWorkspacesFromAPI's invariant about
// repoRefreshMu). It is the shared converger used by both the runtime_gone
// recovery and the profile-drift refresh; the two callers differ only in
// follow-up side effects (RecoverOrphans / Deregister), so those stay at the
// call site.
//
// Returns:
//   - newIDs:     the runtime IDs the server returned in this response, in
//     the order they were returned. These are the daemon's authoritative
//     current runtime set after the call.
//   - droppedIDs: runtime IDs that were tracked before this call but did
//     NOT survive the response. Callers Deregister these so the server marks
//     them offline immediately instead of waiting on the 150 s
//     stale-heartbeat sweep. On the runtime_gone path the triggering row was
//     already deleted server-side (and pruned locally before the register),
//     but a SIBLING dropped here — e.g. a provider removed from the daemon's
//     config, or a disabled profile — still has a live server row that must be
//     deregistered.
//   - ok:         false when the workspace was forgotten between the
//     register call and this apply (e.g. the user left the workspace and
//     syncWorkspacesFromAPI removed it). The caller must abort silently in
//     that case — there is no state left to update.
//
// profileSig is the digest captured during the register; an empty value is
// the explicit "fetch failed, keep the previous signature" sentinel from
// appendProfileRuntimes.
//
// preserveProviders lists the built-in providers this response is not
// authoritative about, keyed by provider — see preserveProvidersFromProbe. Their
// entries are absent from the payload, and therefore from the response, either
// because the probe failed (transient) or because they were confirmed
// below-minimum by a path with no authority to demote. Either way, dropping
// their rows here would take a runtime offline outside the one path that does it
// safely, so their existing built-in runtime rows are kept instead.
//
// A provider already under a demotion hold is a different case and is still
// rejected below: that verdict was reached by demoteBelowMinimumRuntimes, which
// removed the rows under the claim barrier, and this response merely predates
// it.
func (d *Daemon) applyRegisterResponseInPlace(workspaceID string, resp *RegisterResponse, profileSig string, preserveProviders map[string]string) (newIDs, droppedIDs []string, ok bool) {
	d.mu.Lock()
	defer d.mu.Unlock()
	ws, exists := d.workspaces[workspaceID]
	if !exists {
		return nil, nil, false
	}
	// Reject entries for providers demoted since this register was sent. The
	// payload predates the verdict, so the response carries the provider as if
	// it were healthy; indexing it here would undo demoteBelowMinimumRuntimes.
	// Both sides do this under d.mu, which gives the two a total order: either
	// the demotion runs first and this rejects the late response, or the apply
	// runs first and the demotion removes the row it just added. Either way the
	// provider ends up offline. Rejected IDs join droppedIDs so the caller
	// deregisters the row the server upserted back into existence.
	newIDs = make([]string, 0, len(resp.Runtimes))
	newIDSet := make(map[string]struct{}, len(resp.Runtimes))
	rejected := make(map[string]struct{})
	for _, rt := range resp.Runtimes {
		if rt.ProfileID == "" && d.providerDemotedLocked(rt.Provider) {
			rejected[rt.ID] = struct{}{}
			droppedIDs = append(droppedIDs, rt.ID)
			continue
		}
		newIDs = append(newIDs, rt.ID)
		newIDSet[rt.ID] = struct{}{}
	}
	// Drop runtimeIndex entries for prior runtime IDs that the server did not
	// return — typically there are none for upsert-on-existing-provider, but
	// a daemon config change (provider removed) or a profile disable would
	// leak entries otherwise.
	kept := newIDs
	for _, oldID := range ws.runtimeIDs {
		if _, stillThere := newIDSet[oldID]; stillThere {
			continue
		}
		if _, alreadyRejected := rejected[oldID]; alreadyRejected {
			// The server returned this ID for a demoted provider and it is
			// already in droppedIDs; drop the index entry without recording it
			// a second time.
			delete(d.runtimeIndex, oldID)
			continue
		}
		if rt, tracked := d.runtimeIndex[oldID]; tracked && rt.ProfileID == "" {
			if _, preserve := preserveProviders[rt.Provider]; preserve {
				kept = append(kept, oldID)
				continue
			}
		}
		delete(d.runtimeIndex, oldID)
		droppedIDs = append(droppedIDs, oldID)
	}
	for _, rt := range resp.Runtimes {
		if _, skip := rejected[rt.ID]; skip {
			continue
		}
		d.runtimeIndex[rt.ID] = rt
	}
	// Response is authoritative — replace, do not append. Replacing also
	// catches the rare case where UpsertAgentRuntime returns a different ID
	// for a surviving provider (e.g. schema change); the daemon converges on
	// what the server says without leaving stale heartbeat goroutines.
	ws.runtimeIDs = kept
	if resp.ReposVersion != "" {
		ws.reposVersion = resp.ReposVersion
		ws.allowedRepoURLs = repoAllowlist(resp.Repos)
	}
	if len(resp.Settings) > 0 {
		ws.settings = resp.Settings
	}
	// Refresh the cached profile signature only when the fetch succeeded;
	// an empty sig means the GetRuntimeProfiles call failed and we must
	// preserve the previous signature so the next sync tick can still
	// detect a real drift instead of falsely thinking everything is in sync.
	if profileSig != "" {
		ws.profileSetSig = profileSig
	}
	return newIDs, droppedIDs, true
}

// mergeBuiltinRegisterResponse applies a builtins-only register response
// ADDITIVELY: returned built-in runtimes are indexed and appended, and a prior
// runtime ID that the response did not mention is left alone.
//
// applyRegisterResponseInPlace treats the response as authoritative and drops
// unmentioned IDs, which is right for the convergence paths that deliberately
// re-derive a workspace's whole runtime set. It is wrong for CLI discovery
// (MUL-5439): that payload carries built-in runtimes only, so under the
// authoritative rule it would evict the workspace's custom profile runtimes from
// runtimeIndex and stop their heartbeats, possibly mid-task. The server's
// register endpoint is a pure per-entry upsert and prunes nothing, so omitted
// runtimes still exist server-side and must be kept locally.
//
// Two deliberate omissions keep this path out of custom-profile business:
//
//   - Entries with a ProfileID are ignored. Discovery registers built-ins only,
//     so this is an invariant guard: a custom runtime must never enter the set
//     through here.
//   - profileSetSig is neither read nor written. Caching a signature observed
//     during discovery would tell refreshWorkspaceRuntimeProfiles that the
//     profile set was already converged, and a profile disabled at that moment
//     would keep its runtime alive forever.
//
// ID rotation is still handled: when the response returns a different ID for a
// built-in provider the workspace already had, that specific old ID is replaced
// rather than accumulating a duplicate heartbeat.
func (d *Daemon) mergeBuiltinRegisterResponse(workspaceID string, resp *RegisterResponse) (newIDs []string, revived revivedRuntimes, ok bool) {
	d.mu.Lock()
	defer d.mu.Unlock()
	ws, exists := d.workspaces[workspaceID]
	if !exists {
		return nil, revivedRuntimes{}, false
	}

	// Index the workspace's current built-in runtimes by provider so a rotated
	// ID replaces its predecessor instead of doubling it.
	existingByProvider := make(map[string]string, len(ws.runtimeIDs))
	kept := make([]string, 0, len(ws.runtimeIDs)+len(resp.Runtimes))
	present := make(map[string]struct{}, len(ws.runtimeIDs))
	for _, id := range ws.runtimeIDs {
		if rt, found := d.runtimeIndex[id]; found && rt.ProfileID == "" {
			existingByProvider[rt.Provider] = id
		}
		kept = append(kept, id)
		present[id] = struct{}{}
	}

	for _, rt := range resp.Runtimes {
		if rt.ProfileID != "" {
			// Not ours to manage; the drift path owns custom profiles.
			continue
		}
		// Demoted since this register was sent: the response predates the
		// verdict, so bringing the provider back here would undo the demotion.
		// Both run under d.mu, so the two are totally ordered — see the same
		// guard in applyRegisterResponseInPlace. The caller deregisters the row
		// the server upserted back.
		if d.providerDemotedLocked(rt.Provider) {
			revived.add(d, rt.ID, rt.Provider)
			continue
		}
		d.runtimeIndex[rt.ID] = rt
		if _, already := present[rt.ID]; already {
			continue
		}
		if oldID, rotated := existingByProvider[rt.Provider]; rotated && oldID != rt.ID {
			// Same runtime, new ID: swap in place and retire the old entry.
			for i, id := range kept {
				if id == oldID {
					kept[i] = rt.ID
					break
				}
			}
			delete(d.runtimeIndex, oldID)
			delete(present, oldID)
		} else {
			kept = append(kept, rt.ID)
		}
		present[rt.ID] = struct{}{}
		existingByProvider[rt.Provider] = rt.ID
		newIDs = append(newIDs, rt.ID)
	}
	ws.runtimeIDs = kept

	if resp.ReposVersion != "" {
		ws.reposVersion = resp.ReposVersion
		ws.allowedRepoURLs = repoAllowlist(resp.Repos)
	}
	if len(resp.Settings) > 0 {
		ws.settings = resp.Settings
	}
	return newIDs, revived, true
}

// providerDemotedLocked reports whether provider is currently held below the
// minimum supported version. Callers must hold d.mu — the demotion record and
// every register-response apply share that lock precisely so a late response
// can never slip between the two.
func (d *Daemon) providerDemotedLocked(provider string) bool {
	_, demoted := d.demotedProviders[provider]
	return demoted
}

// demotedOfflineReasonLocked returns the structured cause recorded for a
// demoted provider, or nil when the verdict carries none (below-minimum) or the
// provider is not demoted. Callers must hold d.mu.
func (d *Daemon) demotedOfflineReasonLocked(provider string) *RuntimeOfflineReason {
	record, demoted := d.demotedProviders[provider]
	if !demoted {
		return nil
	}
	return record.offline
}

// revivedRuntimes are the rows a register response brought back for a provider
// the daemon has already condemned: the ids to take offline again, and the
// cause to re-attach per row because that register's upsert just overwrote it.
type revivedRuntimes struct {
	ids     []string
	reasons map[string]RuntimeOfflineReason
}

// reasonsFor narrows the causes to the rows actually being deregistered. The
// caller re-checks tracking first — a row that came back legitimately in the
// meantime is dropped from the list — and sending a cause for a row we are no
// longer taking offline would attach it to a healthy runtime.
func (r revivedRuntimes) reasonsFor(runtimeIDs []string) map[string]RuntimeOfflineReason {
	if len(r.reasons) == 0 {
		return nil
	}
	out := make(map[string]RuntimeOfflineReason, len(runtimeIDs))
	for _, id := range runtimeIDs {
		if reason, ok := r.reasons[id]; ok {
			out[id] = reason
		}
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

// add records one revived row. Callers must hold d.mu (it reads the demotion
// record), and it is a no-op for a provider with no structured cause — those
// rows still need deregistering, they just have nothing to re-attach.
func (r *revivedRuntimes) add(d *Daemon, runtimeID, provider string) {
	r.ids = append(r.ids, runtimeID)
	reason := d.demotedOfflineReasonLocked(provider)
	if reason == nil {
		return
	}
	if r.reasons == nil {
		r.reasons = make(map[string]RuntimeOfflineReason, 1)
	}
	r.reasons[runtimeID] = *reason
}

// demotionRecord is a confirmed verdict about the binary on disk: the evidence
// that produced it (the rejected version, or why the CLI could not be run),
// plus the demotionSeq tick that establishes when it was reached.
//
// offline is the structured half, kept because the server can lose it. A
// register sent before the verdict still upserts the runtime row, and that
// upsert overwrites metadata wholesale — so the reason this daemon just stored
// is gone, and the cleanup that takes the revived row offline again has to
// re-attach it. Without that the server ends up "offline, no reason", which
// downgrades the refusal back to "wait for the machine" (MUL-6164).
type demotionRecord struct {
	evidence string
	offline  *RuntimeOfflineReason
	seq      uint64
}

// markProvidersDemoted records a CONFIRMED verdict so a register response still
// in flight cannot revive the provider. Callers must hold d.mu.
func (d *Daemon) markProvidersDemotedLocked(providers map[string]runtimeVerdict) {
	if len(providers) == 0 {
		return
	}
	if d.demotedProviders == nil {
		d.demotedProviders = make(map[string]demotionRecord, len(providers))
	}
	// One tick per verdict batch: every record written here is newer than any
	// probe round that snapshotted the counter before this call.
	d.demotionSeq++
	for provider, verdict := range providers {
		d.demotedProviders[provider] = demotionRecord{
			evidence: verdict.reason,
			offline:  verdict.offline,
			seq:      d.demotionSeq,
		}
	}
}

// notExecutableConfirmWindow is how long a "the OS will not run this file"
// verdict must keep reproducing before the daemon acts on it.
//
// The verdict itself is deterministic, but the file is not: the repair we tell
// users to run (`node <pkg>/install.cjs`) overwrites the bin entry in place, and
// a probe that lands mid-copy sees a truncated file. Requiring a second sighting
// this far apart makes an overwrite window impossible to mistake for a broken
// install, and costs a genuinely broken install only one extra probe round.
// A var so tests can collapse the wait.
var notExecutableConfirmWindow = time.Minute

// confirmNotExecutable records that this round found provider unrunnable and
// reports whether the verdict is now old enough to act on.
//
// The first sighting only starts the clock. Two sightings are required no matter
// how the window is configured, so concurrent probe rounds — four callers reach
// detectBuiltinRuntimes — cannot combine into an instant demotion.
func (d *Daemon) confirmNotExecutable(provider string, now time.Time) bool {
	d.mu.Lock()
	defer d.mu.Unlock()
	first, seen := d.notExecutableSince[provider]
	if !seen {
		if d.notExecutableSince == nil {
			d.notExecutableSince = make(map[string]time.Time, 1)
		}
		d.notExecutableSince[provider] = now
		return false
	}
	return now.Sub(first) >= notExecutableConfirmWindow
}

// clearNotExecutable forgets the pending verdict for a provider that probed OK,
// so a later unrelated failure starts its own confirmation window instead of
// inheriting a stale one.
func (d *Daemon) clearNotExecutable(provider string) {
	d.mu.Lock()
	defer d.mu.Unlock()
	delete(d.notExecutableSince, provider)
}

// demotionSeqSnapshot returns the current demotion counter. A probe round takes
// this BEFORE it samples any version, so a later clearProviderDemotions call can
// prove its evidence postdates a verdict rather than merely arriving after it.
func (d *Daemon) demotionSeqSnapshot() uint64 {
	d.mu.Lock()
	defer d.mu.Unlock()
	return d.demotionSeq
}

// clearProviderDemotions drops the verdict for providers whose version probed
// acceptable again, which is what lets converge register them back.
//
// sampledAfter is the counter the calling round snapshotted before it probed. A
// hold recorded after that snapshot is NEWER evidence than anything this round
// saw, so it survives: four callers probe concurrently and sampling happens off
// the lock, which means "returned last" says nothing about "looked last". The
// round that overlapped a demotion simply declines to clear it and the next one
// — which starts after the verdict exists, so its sample cannot predate it —
// does the release. Recovery is at worst one round late; clearing on stale
// evidence would let a stale register response through every other guard.
func (d *Daemon) clearProviderDemotions(providers []string, sampledAfter uint64) {
	if len(providers) == 0 {
		return
	}
	d.mu.Lock()
	defer d.mu.Unlock()
	if len(d.demotedProviders) == 0 {
		return
	}
	for _, provider := range providers {
		record, was := d.demotedProviders[provider]
		if !was {
			continue
		}
		if record.seq > sampledAfter {
			d.logger.Info("keeping demotion hold: this probe round started before the verdict",
				"provider", provider, "verdict", record.evidence)
			continue
		}
		delete(d.demotedProviders, provider)
		d.logger.Info("agent CLI is usable again",
			"provider", provider, "previous_verdict", record.evidence)
	}
}

// untrackedRuntimeIDs filters ids down to those the daemon does not currently
// track, so a deregistration decided a moment ago cannot take a row offline that
// a NEWER legitimate registration has since brought back.
//
// Every deregistration is decided under d.mu and issued after releasing it —
// the HTTP call must not hold the daemon lock. A recovery register completing in
// that gap re-creates the same server-side row, usually under the same runtime
// ID, and the older cleanup would then knock out the row that just recovered.
//
// This filter is only half the guarantee, and on its own it is a TOCTOU: the
// recovery can just as easily complete between the filter and the request.
// Callers must therefore run both inside the workspace's register lock, which is
// what makes the check and the Deregister one ordered step against every other
// registration for that workspace.
func (d *Daemon) untrackedRuntimeIDs(ids []string) []string {
	if len(ids) == 0 {
		return nil
	}
	d.mu.Lock()
	defer d.mu.Unlock()
	out := make([]string, 0, len(ids))
	for _, id := range ids {
		if _, tracked := d.runtimeIndex[id]; tracked {
			continue
		}
		out = append(out, id)
	}
	return out
}

func (d *Daemon) reregisterWorkspaceAfterRuntimeGone(ctx context.Context, workspaceID string) error {
	var newIDs []string
	// Send, apply and clean up as one ordered step — see workspaceRegisterLock.
	err := d.withWorkspaceRegisterLock(workspaceID, func() error {
		resp, profileSig, preserve, err := d.registerRuntimesForWorkspaceLocked(ctx, workspaceID)
		if err != nil {
			return fmt.Errorf("register runtimes: %w", err)
		}

		ids, droppedIDs, ok := d.applyRegisterResponseInPlace(workspaceID, resp, profileSig, preserve)
		if !ok {
			return fmt.Errorf("workspace %s no longer tracked", workspaceID)
		}
		newIDs = ids

		for _, rid := range newIDs {
			d.logger.Info("re-registered runtime after server-side deletion",
				"workspace_id", workspaceID, "runtime_id", rid)
		}

		// A sibling runtime dropped by this recovery (a provider removed from the
		// daemon's config, a disabled profile) still has a live server row — the
		// runtime_gone trigger only deleted its own. Eagerly mark those offline,
		// matching the drift path, instead of leaving them claimable until the
		// stale-heartbeat sweep.
		d.deregisterDroppedRuntimes(ctx, workspaceID, droppedIDs, "runtime_gone recovery", nil)
		return nil
	})
	if err != nil {
		return err
	}
	d.notifyRuntimeSetChanged()

	// Tell the server about any tasks the previous (now-deleted) runtime
	// was working on, mirroring the registration path's recover-orphans call.
	// This is intentionally scoped to the runtime_gone recovery: the
	// runtimes were truly gone server-side, so anything still in
	// dispatched/running/waiting_local_directory on those rows is an orphan
	// that needs to be failed-and-retried. The drift-refresh path (which
	// also feeds applyRegisterResponseInPlace) deliberately skips this step
	// because its surviving runtime IDs may still be actively executing
	// tasks for the user (MUL-3332).
	for _, rid := range newIDs {
		if err := d.recoverOrphans(ctx, rid); err != nil {
			return fmt.Errorf("recover-orphans after re-register failed for runtime %s: %w", rid, err)
		}
	}
	return nil
}

// deregisterRuntimes notifies the server that all runtimes are going offline.
func (d *Daemon) deregisterRuntimes() {
	runtimeIDs := d.allRuntimeIDs()
	if len(runtimeIDs) == 0 {
		d.logger.Debug("deregister: no runtimes to deregister")
		return
	}

	d.logger.Debug("deregistering runtimes on shutdown", "count", len(runtimeIDs), "runtime_ids", runtimeIDs)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := d.client.Deregister(ctx, runtimeIDs, nil); err != nil {
		d.logger.Warn("failed to deregister runtimes on shutdown", "error", err)
	} else {
		d.logger.Info("deregistered runtimes", "count", len(runtimeIDs))
	}
}

// allRuntimeIDs returns all runtime IDs across all watched workspaces.
func (d *Daemon) allRuntimeIDs() []string {
	d.mu.Lock()
	defer d.mu.Unlock()
	var ids []string
	for _, ws := range d.workspaces {
		ids = append(ids, ws.runtimeIDs...)
	}
	return ids
}

// findRuntime looks up a Runtime by its ID.
func (d *Daemon) findRuntime(id string) *Runtime {
	d.mu.Lock()
	defer d.mu.Unlock()
	if rt, ok := d.runtimeIndex[id]; ok {
		return &rt
	}
	return nil
}

// recordProfileLaunch remembers the absolute executable path and fixed launch
// args resolved for a custom runtime profile. Called from
// registerRuntimesForWorkspace. Lazily initializes the map so test fixtures
// that build a Daemon literal without seeding every map don't panic.
func (d *Daemon) recordProfileLaunch(profileID, path, version string, fixedArgs []string) {
	if profileID == "" || path == "" {
		return
	}
	d.mu.Lock()
	defer d.mu.Unlock()
	if d.profileLaunchSpecs == nil {
		d.profileLaunchSpecs = make(map[string]profileLaunchSpec)
	}
	d.profileLaunchSpecs[profileID] = profileLaunchSpec{
		path:      path,
		version:   version,
		fixedArgs: append([]string(nil), fixedArgs...),
	}
}

// customProfileLaunchForRuntime returns the resolved custom executable path and
// fixed args for a claimed task's RuntimeID, and whether the runtime is a
// custom-profile runtime. It returns false for built-in runtimes (no profile)
// and for runtimes whose profile command was never resolved on this host.
func (d *Daemon) customProfileLaunchForRuntime(runtimeID string) (profileLaunchSpec, bool) {
	if runtimeID == "" {
		return profileLaunchSpec{}, false
	}
	d.mu.Lock()
	defer d.mu.Unlock()
	rt, ok := d.runtimeIndex[runtimeID]
	if !ok || rt.ProfileID == "" {
		return profileLaunchSpec{}, false
	}
	spec, ok := d.profileLaunchSpecs[rt.ProfileID]
	if !ok || spec.path == "" {
		return profileLaunchSpec{}, false
	}
	spec.fixedArgs = append([]string(nil), spec.fixedArgs...)
	return spec, true
}

// runtimeVersionProbeConcurrency bounds how many `<cli> --version` probes run
// at once during registration. Version detection is process-spawn bound rather
// than CPU bound, and each probe already carries its own timeout, so a modest
// fan-out is safe even when a host has many agent CLIs installed.
const runtimeVersionProbeConcurrency = 8

// runtimeVersionProbeAttempts bounds how many times one provider's `<cli>
// --version` probe runs inside a single probe round before that provider is
// dropped from the registration payload.
//
// A round now serves a whole batch of workspace registrations (MUL-5225), so a
// single failed attempt no longer costs one workspace its runtime — it costs
// every workspace registered in that batch, and nothing re-probes until a
// daemon restart or a standalone re-registration. That amplification is worth
// one retry because the failure is usually transient: cfg.Agents only holds
// CLIs that resolved when the daemon started, so a probe failing later is
// typically a version manager swapping the binary in place (vanished path,
// ETXTBSY) or fork/exec briefly failing under a startup burst. Retrying only
// the failed providers keeps the round O(M) — it never reintroduces
// per-workspace probing.
const runtimeVersionProbeAttempts = 2

// runtimeVersionProbeRetryDelay spaces a retry so an in-flight binary swap or a
// momentary fork/exec failure has time to settle. Providers are probed
// concurrently, so a round pays this once, not once per failed provider.
// Overridable for tests.
var runtimeVersionProbeRetryDelay = 500 * time.Millisecond

// runtimeVersionProbeRetryWindow gates the retry on how quickly the failure
// came back. A probe that burned its full timeout is a hung CLI, not a hiccup,
// and retrying it would double the worst case for the whole round — the same
// latency that used to push the desktop runtime step into its empty "no runtime
// found" state before probes were parallelized (MUL-5119). Only fast failures,
// which are the transient ones, are retried.
//
// The window is measured over the WHOLE attempt, self-heal included: a vanished
// pinned path sends resolveAgentEntry through its own version probe of the
// re-resolved candidate, so an attempt can spend its entire budget there and
// still fail instantly on the outer probe of the stale path.
//
// A self-heal candidate rejected by the minimum-version gate is also retried
// within this window, deliberately: the rejection is a verdict about whatever
// PATH resolved to during the upgrade window the heal exists for, and the
// retry gives the upgrade a beat to publish the new binary before the verdict
// demotes runtimes (see probeBuiltinRuntime).
//
// Overridable for tests.
var runtimeVersionProbeRetryWindow = time.Second

// builtinProbeVerdict distinguishes the ways a provider can fail its probe,
// which callers must treat differently.
//
// "Could not read a version" is transient by construction — the CLI was busy,
// mid-upgrade, or fork/exec hiccuped — so the right response is to leave
// whatever is registered alone and try again. The other two are confirmed
// verdicts about a binary that is on disk right now, and leaving either
// registered means the daemon keeps handing work to a CLI it has already proven
// it cannot use: "read a version and it is below the minimum supported one",
// and "the OS refuses to execute the file at all". Collapsing these into one
// bool made a confirmed verdict indistinguishable from a hiccup, which is what
// let a downgraded CLI keep claiming tasks.
type builtinProbeVerdict int

const (
	builtinProbeOK builtinProbeVerdict = iota
	builtinProbeUnavailable
	builtinProbeBelowMinimum
	// builtinProbeNotExecutable: the file resolved, but the OS rejected it as
	// not a runnable program (an npm placeholder stub whose postinstall was
	// blocked is the case in the field — MUL-6164). Deterministic in the same
	// sense as below-minimum: the same bytes will be refused every time until
	// someone reinstalls, so retrying is not what fixes it.
	builtinProbeNotExecutable
)

// runtimeVerdict is one provider's confirmed verdict: the human reason that
// goes to /health and the daemon log, plus — when the cause is one the user has
// to act on — the structured record the server stores on the runtime row.
//
// The two halves are deliberately separate. The reason is prose for an operator
// reading logs; offline is a stable code plus a repair command for clients,
// which localize their own sentence around it. dispatch/reason.go's rule is
// that a reason code is decided at its source and never reverse-engineered from
// a human-readable string, and this is that source.
type runtimeVerdict struct {
	reason  string
	offline *RuntimeOfflineReason
}

// newRuntimeVerdict pairs a probe verdict with what the server needs to know
// about it. Only a verdict the user must repair carries an offline reason: a
// below-minimum CLI keeps today's behaviour (the runtime goes offline and work
// queues) because changing when THAT blocks a trigger is a separate product
// decision from this one.
//
// execPath is the pinned entry point, which is the right path for this verdict:
// a file the OS refuses to execute is present, so the self-heal that would have
// re-resolved a vanished path never runs, and the probe failed on this exact
// file.
func newRuntimeVerdict(verdict builtinProbeVerdict, reason, execPath string) runtimeVerdict {
	if verdict != builtinProbeNotExecutable {
		return runtimeVerdict{reason: reason}
	}
	offline := &RuntimeOfflineReason{Code: RuntimeOfflineCodeNotExecutable, Detail: reason}
	if repair, ok := agent.ExecFormatRepairFor(execPath); ok {
		offline.Repair = &repair
	}
	return runtimeVerdict{reason: reason, offline: offline}
}

// probeBuiltinRuntime resolves and version-detects one built-in provider,
// retrying a fast failure up to runtimeVersionProbeAttempts times. The verdict
// tells the caller how to treat a drop: builtinProbeUnavailable means the
// version could not be read (or not understood) — transient, leave whatever is
// registered alone — while builtinProbeBelowMinimum is a confirmed too-old
// verdict the caller may demote on. See builtinProbeVerdict.
//
// The second return value is a short human-readable reason when the verdict is
// not OK. It is surfaced on /health as skipped_agents so a user can tell "CLI
// not installed" apart from "CLI installed but dropped at registration", which
// was previously only visible in the daemon log (MUL-5439).
func (d *Daemon) probeBuiltinRuntime(ctx context.Context, name string, entry AgentEntry) (string, string, builtinProbeVerdict) {
	var (
		lastErr  error
		attempts int
	)
	for attempts < runtimeVersionProbeAttempts {
		if attempts > 0 {
			select {
			case <-ctx.Done():
			case <-time.After(runtimeVersionProbeRetryDelay):
			}
			// A cancelled round is shutting down or already past its deadline;
			// keep lastErr pointing at the real probe failure rather than the
			// cancellation that stopped us from retrying it.
			if ctx.Err() != nil {
				break
			}
		}
		attempts++
		// The attempt is timed from here, not from the detect call below:
		// resolveAgentEntry runs a version probe of its own on the re-resolved
		// candidate, and that probe can burn the whole timeout by itself. Timing
		// only the outer call would read "slow self-heal, then an instant
		// failure on the stale path" as a fast failure and retry it, paying the
		// slow half twice.
		startedAt := time.Now()
		// Self-heal a pinned executable path an in-place upgrade deleted
		// (MUL-4486) so version detection — and thus staying registered/online —
		// recovers without a daemon restart. resolveAgentEntry already
		// version-gates the healed binary; the detect + min-version check below
		// still runs to produce the version string this registration reports.
		// It is re-run per attempt because the heal itself can be what a retry
		// fixes: the upgrade that removed the old path may not have published
		// the new one yet on the first attempt.
		resolved, _, heal := d.resolveAgentEntryWithHeal(ctx, name, entry)
		// The pinned path is gone and the binary its command resolves to now is
		// too old. Unlike the direct case below, this verdict is about whatever
		// PATH resolves to at this instant — and the pinned path vanishing is
		// exactly the mid-upgrade window the per-attempt heal exists for, where
		// a stale sibling install can shadow the not-yet-published new binary.
		// Give it the same bounded fast-failure retry as every other outcome
		// before returning the demotable verdict; a retry that finds the
		// upgraded binary adopts it instead. It is still the only way this
		// shape reaches the caller: probing the vanished path can only produce
		// "version detection failed", which by design leaves the runtime online
		// and claiming tasks for a CLI that cannot launch.
		if heal.rejected != nil {
			if attempts < runtimeVersionProbeAttempts && time.Since(startedAt) < runtimeVersionProbeRetryWindow {
				d.logger.Debug("re-resolved agent version too old; retrying probe",
					"name", name, "attempt", attempts, "version", heal.rejected.Detected)
				continue
			}
			d.logger.Warn("skip registering runtime: re-resolved version too old",
				"name", name, "version", heal.rejected.Detected, "error", heal.rejected.Error())
			return heal.rejected.Detected, heal.rejected.Error(), builtinProbeBelowMinimum
		}
		version, err := detectAgentVersion(ctx, agent.Command{Path: resolved.Path})
		if err != nil {
			lastErr = err
			if time.Since(startedAt) >= runtimeVersionProbeRetryWindow {
				break
			}
			if attempts < runtimeVersionProbeAttempts {
				d.logger.Debug("agent version probe failed; retrying", "name", name, "attempt", attempts, "error", err)
			}
			continue
		}
		if err := checkAgentMinVersion(name, version); err != nil {
			var tooOld *agent.BelowMinimumError
			if errors.As(err, &tooOld) {
				// The verdict is a pure function of a version that PARSED, so a
				// retry would reach the same conclusion — drop the provider now.
				d.logger.Warn("skip registering runtime: version too old", "name", name, "version", version, "error", err)
				// The version is returned even though the provider is dropped: the
				// caller needs it to report what it demoted.
				return version, err.Error(), builtinProbeBelowMinimum
			}
			// The CLI ran but printed something (or nothing) the gate could not
			// parse. That is "we didn't learn a version", not "we verified it is
			// too old" — the same transient rule as a failed exec, because the
			// below-minimum verdict tears runtimes down and must never fire on
			// evidence this thin.
			lastErr = err
			if time.Since(startedAt) >= runtimeVersionProbeRetryWindow {
				break
			}
			if attempts < runtimeVersionProbeAttempts {
				d.logger.Debug("agent version unparseable; retrying", "name", name, "attempt", attempts, "error", err)
			}
			continue
		}
		d.setAgentVersion(name, version)
		d.refreshHealedVersion(name, resolved.Path, version)
		if version == "" {
			// A provider with no minimum-version floor reaches here with a blank
			// when its `--version` exits 0 printing nothing. setAgentVersion just
			// refused to let it overwrite the cache; the payload needs the same
			// protection, because a whole-set re-registration triggered by a
			// NEIGHBOUR's upgrade would otherwise send the blank and wipe a
			// version the server already knows. Reuse the last known one; a
			// provider that never had a version stays blank, as before.
			version = d.agentVersion(name)
		}
		d.logger.Debug("agent version detected", "name", name, "version", version, "path", resolved.Path)
		return version, "", builtinProbeOK
	}
	// The OS refusing to execute the file is not a failed probe, it is a
	// finding: the CLI is installed, resolvable, and unrunnable. Report it as
	// its own verdict so the caller can take the runtime offline instead of
	// keeping it online for a binary that cannot start (MUL-6164). The
	// diagnosis attached in pkg/agent rides along as the reason, so /health
	// carries the repair command and not just the errno.
	if agent.IsExecFormatError(lastErr) {
		d.logger.Warn("skip registering runtime: agent CLI is not executable on this machine",
			"name", name, "attempts", attempts, "error", lastErr)
		return "", fmt.Sprintf("agent CLI is not executable: %v", lastErr), builtinProbeNotExecutable
	}

	d.logger.Warn("skip registering runtime", "name", name, "attempts", attempts, "error", lastErr)
	reason := "version detection failed"
	if lastErr != nil {
		reason = fmt.Sprintf("version detection failed: %v", lastErr)
	}
	return "", reason, builtinProbeUnavailable
}

// detectBuiltinRuntimes version-detects every configured built-in agent CLI and
// returns a registration entry for each one that resolves and clears the
// minimum-version gate. Probes run concurrently, bounded by
// runtimeVersionProbeConcurrency.
//
// The previous implementation probed serially, so total latency was the SUM of
// every CLI's `--version` call. On an onboarding host with several coding tools
// installed that stacked into many seconds of dead time before the daemon could
// register — long enough that the desktop runtime step timed out into its empty
// "no runtime found" state while the probes were still running (MUL-5119).
// Fanning the probes out makes total latency track the SLOWEST single probe
// instead of their sum, so a freshly-created workspace lights up its runtimes
// well inside the UI's scanning window.
//
// Each probe still self-heals a vanished pinned path (MUL-4486) and re-detects
// the live version — nothing is cached on the Daemon, so an in-place CLI
// upgrade is still reported with its current version. A provider whose version
// stays undetectable across probeBuiltinRuntime's bounded attempts, or which is
// below the minimum supported version, is logged and skipped, exactly as the
// serial loop did.
//
// The result describes the machine, not a workspace, so a caller registering a
// batch of workspaces at once calls this ONCE and passes the payload to
// registerRuntimesForWorkspaceBatch for each workspace (MUL-5225).
//
// The second return value is THIS round's confirmed verdicts, provider to the
// evidence against it. It is returned rather than read back
// out of skippedAgents because that pair is a diagnostic snapshot of whichever
// round published last: four different goroutines call this (the discovery
// loop, the workspace sync, a runtime_gone re-register, a profile drift
// refresh), so a caller acting on the shared copy can act on someone else's
// probe — and demoting a runtime is not a decision to make on another round's
// evidence.
//
// The third return value is THIS round's unavailable providers (version could
// not be read), provider to reason. Callers that treat a register response as
// AUTHORITATIVE for a workspace's whole runtime set (the runtime_gone recovery
// and the profile drift refresh, via applyRegisterResponseInPlace) must
// preserve these providers' existing runtimes: they are absent from the
// payload because the probe failed, which is transient — tearing a working
// runtime down over it is exactly what the unavailable/below-minimum verdict
// split exists to prevent. Only a below-minimum verdict may demote.
func (d *Daemon) detectBuiltinRuntimes(ctx context.Context) ([]map[string]string, map[string]runtimeVerdict, map[string]string) {
	type detected struct {
		name    string
		version string
	}
	// Snapshot before any probe runs: everything sampled below is at least as
	// new as every verdict recorded up to this point, which is exactly the
	// claim clearProviderDemotions needs and cannot make from timing alone.
	sampledAfter := d.demotionSeqSnapshot()
	var (
		mu          sync.Mutex
		results     []detected
		skipped     = map[string]string{}
		demotable   = map[string]runtimeVerdict{}
		unavailable = map[string]string{}
		g           errgroup.Group
	)
	g.SetLimit(runtimeVersionProbeConcurrency)
	for name, entry := range d.agents() {
		name, entry := name, entry
		g.Go(func() error {
			version, reason, verdict := d.probeBuiltinRuntime(ctx, name, entry)
			if verdict != builtinProbeOK {
				// A not-executable verdict is deterministic, but the file can be
				// unreadable for a moment while an installer overwrites it in
				// place — which is exactly the repair we are telling users to
				// run. Demote only once the verdict has survived a second probe
				// a confirmation window later; until then it is treated as
				// transient, which costs one more round and nothing else.
				demote := verdict == builtinProbeBelowMinimum ||
					(verdict == builtinProbeNotExecutable && d.confirmNotExecutable(name, time.Now()))
				mu.Lock()
				skipped[name] = reason
				if demote {
					demotable[name] = newRuntimeVerdict(verdict, reason, entry.Path)
				} else {
					unavailable[name] = reason
				}
				mu.Unlock()
				return nil
			}
			d.clearNotExecutable(name)
			mu.Lock()
			results = append(results, detected{name: name, version: version})
			mu.Unlock()
			return nil
		})
	}
	// No probe returns a non-nil error — failures are logged and skipped above —
	// so Wait only blocks for the in-flight probes to finish.
	_ = g.Wait()

	// Publish this round's drops for /health. Replacing (not merging) keeps the
	// diagnostic honest: a provider that registered successfully this round must
	// not stay listed as skipped.
	d.setSkippedAgents(skipped)

	// Source iteration (a map) and parallel completion order are both
	// nondeterministic; sort by provider so the registration payload is stable
	// across runs and order-sensitive tests stay deterministic.
	sort.Slice(results, func(i, j int) bool { return results[i].name < results[j].name })

	// A provider that probes OK is no longer below the minimum, so release any
	// demotion held against it — otherwise the register that converge is about
	// to make would be rejected by the very guard that protects the demotion.
	// Only holds that already existed when this round started sampling are
	// released; see clearProviderDemotions for why "returned last" is not the
	// same as "sampled last".
	recovered := make([]string, 0, len(results))
	for _, r := range results {
		recovered = append(recovered, r.name)
	}
	d.clearProviderDemotions(recovered, sampledAfter)

	runtimes := make([]map[string]string, 0, len(results))
	for _, r := range results {
		displayName := providerDisplayName(r.name)
		if d.cfg.DeviceName != "" {
			displayName = fmt.Sprintf("%s (%s)", displayName, d.cfg.DeviceName)
		}
		runtimes = append(runtimes, map[string]string{
			"name":    displayName,
			"type":    r.name,
			"version": r.version,
			"status":  "online",
		})
	}
	return runtimes, demotable, unavailable
}

// cloneRuntimeEntries deep-copies a registration runtime payload. Callers that
// receive a shared built-in payload (see registerRuntimesForWorkspaceBatch) use
// this before appending their own workspace's custom runtime profiles, so one
// workspace's profiles can never leak into another's registration.
func cloneRuntimeEntries(in []map[string]string) []map[string]string {
	out := make([]map[string]string, 0, len(in))
	for _, entry := range in {
		cp := make(map[string]string, len(entry))
		for k, v := range entry {
			cp[k] = v
		}
		out = append(out, cp)
	}
	return out
}

// registerRuntimesForWorkspace registers this host's runtimes for one
// workspace, probing the built-in agent CLIs itself. This is the entry point
// for every standalone registration — a runtime_gone re-register, a profile
// drift refresh, a recovery retry — so each of those still re-detects versions
// and picks up an in-place CLI upgrade.
//
// Registering a batch of workspaces at once (daemon startup) goes through
// registerRuntimesForWorkspaceBatch instead, which shares one probe round
// across the batch.
//
// The third return value is the set of providers this round's response must not
// be treated as authoritative about, provider to reason — see
// preserveProvidersFromProbe. Callers that apply the response as AUTHORITATIVE
// must pass it to applyRegisterResponseInPlace.
//
// The caller must hold the workspace's register lock (withWorkspaceRegisterLock)
// for this call, the apply, and the cleanup that follows it.
func (d *Daemon) registerRuntimesForWorkspaceLocked(ctx context.Context, workspaceID string) (*RegisterResponse, string, map[string]string, error) {
	builtins, belowMinimum, unavailable := d.detectBuiltinRuntimes(ctx)
	resp, profileSig, err := d.registerRuntimesForWorkspaceBatchLocked(ctx, workspaceID, builtins)
	return resp, profileSig, preserveProvidersFromProbe(unavailable, belowMinimum), err
}

// preserveProvidersFromProbe returns the providers whose absence from a
// registration payload must NOT be read as "this workspace should stop hosting
// them", keyed by provider.
//
// Unavailable is the obvious half: the version could not be read, which is
// transient, so dropping the rows would tear a working runtime down over one
// failed probe.
//
// Demotable is the half that is easy to get wrong, because the verdict IS
// confirmed — a version was read and rejected, or the OS refused to run the
// file. What is missing on these paths is not the evidence but the authority to
// act on it. Taking a runtime offline requires two things neither the
// runtime_gone recovery nor the profile-drift refresh has: the claim barrier, so
// the rows are never pulled out from under a task that is still executing, and a
// seq-stamped hold, so a register sent before the verdict cannot revive the
// provider when it lands. demoteUnusableRuntimes has both and is the single
// owner of the demotion. A path that drops the rows without them reaches the
// same verdict and leaves no record that it did, so the next in-flight response
// quietly undoes it.
//
// Preserving here costs at most one refresh tick of an unusable CLI staying
// online, which is the pre-demotion status quo rather than a new exposure.
func preserveProvidersFromProbe(unavailable map[string]string, demotable map[string]runtimeVerdict) map[string]string {
	if len(demotable) == 0 {
		return unavailable
	}
	preserve := make(map[string]string, len(unavailable)+len(demotable))
	for provider, reason := range unavailable {
		preserve[provider] = reason
	}
	for provider, verdict := range demotable {
		preserve[provider] = verdict.reason
	}
	return preserve
}

// registerRuntimesForWorkspaceBatch registers one workspace against an already
// detected built-in runtime payload.
//
// Built-in CLIs are a machine-level fact, not a per-workspace one, but
// registration is per-workspace — so probing inside every registration made a
// daemon serving N workspaces spawn N×M `<cli> --version` processes at startup
// (24 workspaces × 5 agents = 120 instead of 5, MUL-5225 / #5837). Beyond the
// wasted startup time, some CLI wrappers have visible side effects when
// executed, and a single slow probe gets multiplied by the workspace count.
//
// Taking the payload as a parameter lets one probe round serve a whole
// registration batch while keeping the refresh semantics: nothing is cached on
// the Daemon, so the next standalone registration re-probes.
//
// builtins is treated as read-only and is copied before this workspace's custom
// runtime profiles are appended.
//
// The caller must hold the workspace's register lock (withWorkspaceRegisterLock)
// for this call, the apply, and the cleanup that follows it.
func (d *Daemon) registerRuntimesForWorkspaceBatchLocked(ctx context.Context, workspaceID string, builtins []map[string]string) (*RegisterResponse, string, error) {
	d.logger.Debug("registering runtimes for workspace", "workspace_id", workspaceID, "agent_count", len(d.agents()))
	runtimes := cloneRuntimeEntries(builtins)
	var failedProfiles []map[string]string

	// Append any workspace custom runtime profiles whose command resolves on
	// this host (MUL-3284). This is best-effort: a fetch error (e.g. an older
	// server returning 404) must never fail registration — the daemon simply
	// continues with the built-in runtimes it already collected. A profile
	// whose command_name is neither on PATH nor already discovered for the same
	// protocol family is skipped (the host doesn't have it).
	//
	// profileSig is a content hash of the workspace's profile list captured
	// here so an on-demand server notification can skip re-registration when
	// the effective profile set is already current (MUL-3332). An empty string
	// means the fetch failed and the caller must keep whatever signature was
	// previously cached on the workspaceState.
	profileSig := d.appendProfileRuntimes(ctx, workspaceID, &runtimes, &failedProfiles)

	if len(runtimes) == 0 && len(failedProfiles) == 0 {
		// profileSig is still meaningful even when nothing resolves: the
		// refresh path uses it to remember "we already converged on the
		// disabled-everywhere state" so duplicate change notifications are a
		// no-op instead of a re-empty-register loop. Initial-registration
		// callers that don't care about the sig discard it via _.
		return nil, profileSig, ErrNoRuntimesToRegister
	}

	req := map[string]any{
		"workspace_id":      workspaceID,
		"daemon_id":         d.cfg.DaemonID,
		"legacy_daemon_ids": d.cfg.LegacyDaemonIDs,
		"device_name":       d.cfg.DeviceName,
		"cli_version":       d.cfg.CLIVersion,
		"launched_by":       d.cfg.LaunchedBy,
		"runtimes":          runtimes,
		"failed_profiles":   failedProfiles,
	}

	resp, err := d.client.Register(ctx, req)
	if err != nil {
		return nil, "", fmt.Errorf("register runtimes: %w", err)
	}
	if len(resp.Runtimes) == 0 && len(failedProfiles) == 0 {
		return nil, "", fmt.Errorf("register runtimes: empty response")
	}
	d.logger.Debug("register response", "workspace_id", workspaceID, "runtimes", len(resp.Runtimes), "repos", len(resp.Repos), "repos_version", resp.ReposVersion)
	d.recordBuiltinVersionsSent(workspaceID, runtimes)
	return resp, profileSig, nil
}

// registerBuiltinRuntimesForWorkspace registers ONLY the built-in runtimes for a
// workspace: no custom runtime profiles are fetched or sent, and no profile
// signature is produced.
//
// This exists for the CLI-discovery path (MUL-5439), which must not participate
// in custom-profile convergence at all. Going through the profile-appending
// registration made discovery observe the profile set as a side effect, and
// caching that observation told the drift path "already converged" — so a
// profile disabled at the same moment a new CLI was discovered would keep its
// runtime alive forever (refreshWorkspaceRuntimeProfiles short-circuits on a
// matching signature). Custom profile add/edit/disable stays exclusively with
// the existing drift path.
//
// builtins is treated as read-only.
//
// The caller must hold the workspace's register lock (withWorkspaceRegisterLock)
// for this call, the merge, and the cleanup that follows it.
func (d *Daemon) registerBuiltinRuntimesForWorkspaceLocked(ctx context.Context, workspaceID string, builtins []map[string]string) (*RegisterResponse, error) {
	runtimes := cloneRuntimeEntries(builtins)
	if len(runtimes) == 0 {
		return nil, ErrNoRuntimesToRegister
	}
	req := map[string]any{
		"workspace_id":      workspaceID,
		"daemon_id":         d.cfg.DaemonID,
		"legacy_daemon_ids": d.cfg.LegacyDaemonIDs,
		"device_name":       d.cfg.DeviceName,
		"cli_version":       d.cfg.CLIVersion,
		"launched_by":       d.cfg.LaunchedBy,
		"runtimes":          runtimes,
		// Deliberately empty: this call carries no profiles, so it must not
		// report profile failures either.
		"failed_profiles": []map[string]string{},
	}
	resp, err := d.client.Register(ctx, req)
	if err != nil {
		return nil, fmt.Errorf("register builtin runtimes: %w", err)
	}
	if len(resp.Runtimes) == 0 {
		return nil, fmt.Errorf("register builtin runtimes: empty response")
	}
	d.logger.Debug("builtin register response", "workspace_id", workspaceID, "runtimes", len(resp.Runtimes))
	d.recordBuiltinVersionsSent(workspaceID, runtimes)
	return resp, nil
}

// appendProfileRuntimes fetches the workspace's enabled custom runtime
// profiles (MUL-3284) and appends a runtime registration entry for each one
// whose command_name resolves on this host. For each resolved profile
// it records the absolute command path and fixed args keyed by profile_id (via
// recordProfileLaunch) so runTask can later launch the custom executable for a
// claimed task.
//
// Best-effort by contract: any error fetching profiles (older server, network
// blip) is logged and swallowed — registration proceeds with the built-in
// runtimes already collected. A profile whose command cannot be resolved is
// skipped with an Info log (this host simply doesn't have that command).
//
// The registration entry mirrors the built-in shape: name = display_name
// (suffixed with the device name like the built-in path), type =
// protocol_family (the routing provider), version = best-effort detected
// version, status = "online", plus the profile_id the server validates.
//
// Returns a content signature of the fetched profile list (MUL-3332). The
// signature is used by on-demand profile refreshes to ignore duplicate change
// notifications while still triggering a re-register without a daemon
// restart. Returns the empty string when the fetch failed — callers must treat
// that as "unknown, do not overwrite a previously-stored signature" (otherwise
// a transient 5xx would silently flip the daemon into thinking the workspace
// has zero profiles).
func (d *Daemon) appendProfileRuntimes(ctx context.Context, workspaceID string, runtimes *[]map[string]string, failedProfiles *[]map[string]string) string {
	resp, err := d.client.GetRuntimeProfiles(ctx, workspaceID)
	if err != nil {
		// Best-effort: never fail registration because profiles couldn't be
		// fetched. An older server with no profiles route returns 404.
		d.logger.Info("skip custom runtime profiles: fetch failed (continuing with built-in runtimes)",
			"workspace_id", workspaceID, "error", err)
		return ""
	}
	if resp == nil {
		// Empty payload — same shape as "server has zero profiles". Return
		// the digest of an empty list so the sync loop can still detect a
		// later transition (zero → first profile added).
		return profileSetSignature(nil)
	}
	for _, profile := range resp.RuntimeProfiles {
		if profile.CommandName == "" || profile.ProtocolFamily == "" {
			d.logger.Warn("skip custom runtime profile: missing command_name or protocol_family",
				"workspace_id", workspaceID, "profile_id", profile.ID, "display_name", profile.DisplayName)
			continue
		}
		if !agent.IsSupportedType(profile.ProtocolFamily) {
			reason := "unsupported protocol_family: " + profile.ProtocolFamily
			d.logger.Warn("skip custom runtime profile: unsupported protocol_family",
				"workspace_id", workspaceID, "profile_id", profile.ID,
				"display_name", profile.DisplayName, "protocol_family", profile.ProtocolFamily)
			*failedProfiles = append(*failedProfiles, map[string]string{
				"profile_id":   profile.ID,
				"command_name": profile.CommandName,
				"reason":       reason,
			})
			continue
		}
		// Resolve the executable to launch for this profile. A per-machine
		// path override (MUL-3284, `patchbay runtime profile set-path`) wins
		// over the PATH lookup when it is set AND points at a real
		// executable — this is how an operator pins a profile to a binary
		// that isn't on the daemon's PATH, or selects between multiple
		// installs on the same host. A configured-but-unusable override
		// (deleted/moved/non-executable) is logged and falls back to PATH
		// and then to the matching provider command already discovered at
		// daemon startup. The latter matters for GUI-launched daemons whose
		// environment cannot resolve a CLI directly even though built-in
		// discovery found it through a login shell or provider install path.
		// When none of those sources resolves, the profile is skipped.
		var resolved string
		var failureReason string
		if override := strings.TrimSpace(d.cfg.ProfileCommandOverrides[profile.ID]); override != "" {
			if profilePathExecutable(override) {
				resolved = override
				d.logger.Info("custom runtime profile: using per-machine command path override",
					"workspace_id", workspaceID, "profile_id", profile.ID, "command_path", resolved)
			} else {
				failureReason = "Configured path override is not executable: " + override
				d.logger.Warn("custom runtime profile: command path override not executable; falling back to PATH",
					"workspace_id", workspaceID, "profile_id", profile.ID,
					"override_path", override, "command_name", profile.CommandName)
			}
		}
		if resolved == "" {
			r, err := lookPath(profile.CommandName)
			if err != nil {
				if discovered, ok := d.agents()[profile.ProtocolFamily]; ok && discovered.Command == profile.CommandName && discovered.Path != "" {
					resolved = discovered.Path
					d.logger.Info("custom runtime profile: using discovered provider command path",
						"workspace_id", workspaceID, "profile_id", profile.ID,
						"protocol_family", profile.ProtocolFamily, "command_path", resolved)
				} else {
					// Host doesn't have this command — expected on hosts that aren't
					// provisioned for this profile. Skip without failing.
					d.logger.Info("skip custom runtime profile: command not found on PATH or provider discovery",
						"workspace_id", workspaceID, "profile_id", profile.ID,
						"command_name", profile.CommandName, "error", err)
					if failureReason != "" {
						failureReason += "; "
					}
					failureReason += "command not found on PATH or provider discovery: " + profile.CommandName
					*failedProfiles = append(*failedProfiles, map[string]string{
						"profile_id":   profile.ID,
						"command_name": profile.CommandName,
						"reason":       failureReason,
					})
					continue
				}
			} else {
				resolved = r
			}
		}
		// Best-effort version detection; an empty version is acceptable. The
		// probe carries the profile's fixed_args so a wrapper reports the
		// version of the CLI it execs rather than its own: `ccms start q36
		// --version` is Claude Code's version, `ccms --version` is the
		// wrapper's, and only the former means anything to the min-version
		// gate (GH #7046).
		version, verErr := detectAgentVersion(ctx, agent.NewCommand(resolved,
			agent.FilterLaunchPrefix(profile.ProtocolFamily, profile.FixedArgs, d.logger)))
		if verErr != nil {
			d.logger.Debug("custom runtime profile: version probe failed (registering with empty version)",
				"workspace_id", workspaceID, "profile_id", profile.ID, "path", resolved, "error", verErr)
			version = ""
		}
		displayName := profile.DisplayName
		if d.cfg.DeviceName != "" {
			displayName = fmt.Sprintf("%s (%s)", displayName, d.cfg.DeviceName)
		}
		d.recordProfileLaunch(profile.ID, resolved, version, profile.FixedArgs)
		d.logger.Info("registering custom runtime profile",
			"workspace_id", workspaceID, "profile_id", profile.ID,
			"protocol_family", profile.ProtocolFamily, "command_path", resolved)
		*runtimes = append(*runtimes, map[string]string{
			"name":       displayName,
			"type":       profile.ProtocolFamily,
			"version":    version,
			"status":     "online",
			"profile_id": profile.ID,
		})
	}
	return profileSetSignature(resp.RuntimeProfiles)
}

// profileSetSignature is a stable content hash of the workspace's custom
// runtime profile list (MUL-3332). An on-demand refresh diffs this against the
// cached value after the server reports a create, edit, disable, or delete; a
// mismatch makes the daemon re-register so the new runtime instance appears
// without a restart.
//
// The hashed projection covers exactly the fields that affect what the
// daemon sends in a Register call: ID, Enabled, ProtocolFamily, CommandName,
// FixedArgs (the launch args every agent on this runtime inherits) and
// Visibility (so a hypothetical future per-creator filter still triggers
// drift). Profiles are sorted by ID first so the digest is order-independent
// (the server is allowed to return them in any order).
func profileSetSignature(profiles []RuntimeProfile) string {
	if len(profiles) == 0 {
		return "0"
	}
	sorted := append([]RuntimeProfile(nil), profiles...)
	sort.Slice(sorted, func(i, j int) bool { return sorted[i].ID < sorted[j].ID })
	h := fnv.New64a()
	// Field separator chosen to never appear in a UUID, slug, or arg.
	const sep = "\x1f"
	for _, p := range sorted {
		fmt.Fprintf(h, "%s%s%t%s%s%s%s%s%s%s",
			p.ID, sep,
			p.Enabled, sep,
			p.ProtocolFamily, sep,
			p.CommandName, sep,
			p.Visibility, sep,
		)
		for _, a := range p.FixedArgs {
			fmt.Fprintf(h, "%s%s", a, sep)
		}
		// Record list end so [a,b] and [ab] hash differently.
		h.Write([]byte("\x1e"))
	}
	return strconv.FormatUint(h.Sum64(), 16)
}

// refreshWorkspaceRuntimeProfiles fetches the workspace's enabled custom
// runtime profile list (MUL-3332), compares its content signature against
// the value cached on the workspaceState, and triggers a re-register when
// the signature has drifted. This is the entry point used by the daemon
// WebSocket change notification so profiles added / edited / disabled via the
// web UI or CLI become visible without a daemon restart.
//
// Best-effort: a fetch error (older server, network blip) preserves the cached
// signature. A successfully-fetched-but-unchanged signature can result from a
// duplicate notification and short-circuits without any further work.
//
// On drift the function takes a path that deliberately differs from
// reregisterWorkspaceAfterRuntimeGone in two ways:
//
//  1. It does NOT call RecoverOrphans for the returned runtime IDs. The
//     server's RecoverOrphanedTasksForRuntime hard-fails every
//     dispatched/running/waiting_local_directory task on a runtime, which is
//     the correct response when a runtime row was actually deleted server-
//     side, but a catastrophic false positive on profile drift: a built-in
//     runtime still actively executing tasks would have its work killed
//     just because the user added a sibling custom profile.
//
//  2. It tolerates ErrNoRuntimesToRegister (custom-only daemon disables its
//     only profile) by Deregistering the now-stale local runtime IDs and
//     clearing local tracking. Without this, registerRuntimesForWorkspace
//     would short-circuit on the empty list, the daemon would keep polling
//     and heartbeating runtimes that should be offline, and the server
//     would leave them online for the full 150 s stale-heartbeat window.
//
// The workspaceState pointer is never replaced (matches the invariant
// documented on syncWorkspacesFromAPI and reregisterWorkspaceAfterRuntimeGone).
func (d *Daemon) refreshWorkspaceRuntimeProfiles(ctx context.Context, workspaceID string) error {
	refreshCtx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()

	resp, err := d.client.GetRuntimeProfiles(refreshCtx, workspaceID)
	if err != nil {
		// Older server (no profiles route) returns 404; let the on-demand caller
		// log the failure at debug level.
		return err
	}
	var profiles []RuntimeProfile
	if resp != nil {
		profiles = resp.RuntimeProfiles
	}
	live := profileSetSignature(profiles)

	d.mu.Lock()
	ws, ok := d.workspaces[workspaceID]
	if !ok {
		d.mu.Unlock()
		// Workspace was removed while the refresh was in flight — nothing to do.
		return nil
	}
	cached := ws.profileSetSig
	d.mu.Unlock()

	if cached == live {
		return nil
	}

	d.logger.Info("custom runtime profile set changed; refreshing workspace runtimes",
		"workspace_id", workspaceID, "previous_sig", cached, "current_sig", live,
		"profile_count", len(profiles))

	// Send, apply and clean up as one ordered step — see workspaceRegisterLock.
	return d.withWorkspaceRegisterLock(workspaceID, func() error {
		return d.applyProfileDriftRegistration(ctx, workspaceID)
	})
}

// applyProfileDriftRegistration is refreshWorkspaceRuntimeProfiles' registration
// half. The caller must hold the workspace's register lock.
func (d *Daemon) applyProfileDriftRegistration(ctx context.Context, workspaceID string) error {
	regResp, profileSig, preserve, err := d.registerRuntimesForWorkspaceLocked(ctx, workspaceID)
	if err != nil {
		if errors.Is(err, ErrNoRuntimesToRegister) {
			// Convergence-to-zero: a custom-only daemon's only enabled
			// profile was just disabled / deleted, and there are no built-in
			// agents to fall back on. Drop the daemon's local tracking and
			// proactively Deregister the orphaned server-side rows so the
			// runtime list converges to empty without waiting on the 150 s
			// stale-heartbeat sweep.
			//
			// An empty payload only proves "nothing to host" for the providers
			// whose probe actually concluded, and only for the verdicts this path
			// is allowed to act on. A provider dropped as UNAVAILABLE or confirmed
			// below-minimum is absent for a reason this path must not treat as
			// authoritative (see preserveProvidersFromProbe), so its existing
			// built-in rows are preserved — the same rule
			// applyRegisterResponseInPlace applies — while the profile runtimes
			// the drift is actually about still converge. Skipping the whole
			// convergence instead would let one permanently unprobeable CLI
			// keep a disabled profile's runtime online forever.
			return d.convergeWorkspaceRuntimesToZero(ctx, workspaceID, profileSig, preserve)
		}
		return err
	}

	newIDs, droppedIDs, ok := d.applyRegisterResponseInPlace(workspaceID, regResp, profileSig, preserve)
	if !ok {
		return fmt.Errorf("workspace %s no longer tracked", workspaceID)
	}

	for _, rid := range newIDs {
		d.logger.Info("re-registered runtime after profile drift",
			"workspace_id", workspaceID, "runtime_id", rid)
	}
	d.notifyRuntimeSetChanged()

	// Drift may have shrunk the runtime set (a profile got disabled while
	// other runtimes survive). Eagerly mark those server-side rows offline
	// so the runtime list reflects reality immediately; a 5xx blip here is
	// fine because the server's stale-heartbeat sweep will pick them up
	// within ~150 s as a backstop.
	d.deregisterDroppedRuntimes(ctx, workspaceID, droppedIDs, "profile drift", nil)

	// Intentionally NO RecoverOrphans here: see method doc.
	return nil
}

// convergeWorkspaceRuntimesToZero handles the drift-refresh case where
// registerRuntimesForWorkspaceLocked would have short-circuited because the
// daemon has nothing to host on this workspace anymore. It Deregisters the
// previously-tracked runtime IDs (best-effort) and clears the daemon's local
// tracking so taskWakeup / heartbeat / poll loops stop attempting work
// against runtimes that should now be offline.
//
// The caller must hold the workspace's register lock — this is a cleanup, and it
// has the same ordering requirement as every other one (workspaceRegisterLock).
//
// preserveProviders lists the built-in providers this round's absence from the
// payload says nothing authoritative about (see preserveProvidersFromProbe), so
// their existing built-in rows survive — the same preserve rule
// applyRegisterResponseInPlace applies. Everything else (the disabled/deleted
// profiles' runtimes, and any builtin whose probe concluded) converges away.
//
// The workspaceState pointer is preserved: the workspace itself is still a
// valid workspace the user belongs to, just one with no agents on this
// daemon for the moment. If the user re-enables a profile or installs a
// built-in agent, the profile-change notification or the next daemon WS
// reconnect will register it again.
func (d *Daemon) convergeWorkspaceRuntimesToZero(ctx context.Context, workspaceID, profileSig string, preserveProviders map[string]string) error {
	d.mu.Lock()
	ws, ok := d.workspaces[workspaceID]
	if !ok {
		d.mu.Unlock()
		return nil
	}
	// A fresh array, not ws.runtimeIDs[:0]: the health handler copies this
	// slice header under d.mu and serializes it after releasing the lock
	// (removeStaleRuntime keeps the same rule).
	kept := ws.runtimeIDs[:0:0]
	var dropped []string
	for _, rid := range ws.runtimeIDs {
		if rt, tracked := d.runtimeIndex[rid]; tracked && rt.ProfileID == "" {
			if _, preserve := preserveProviders[rt.Provider]; preserve {
				kept = append(kept, rid)
				continue
			}
			// A builtin row converging away here was confirmed gone this
			// round; drop its version record too, mirroring the demote path.
			delete(ws.builtinVersions, rt.Provider)
		}
		delete(d.runtimeIndex, rid)
		dropped = append(dropped, rid)
	}
	ws.runtimeIDs = kept
	if profileSig != "" {
		// Cache the converged signature so we don't loop into re-converging
		// on every subsequent sync tick.
		ws.profileSetSig = profileSig
	}
	d.mu.Unlock()

	d.logger.Info("custom runtime profile drift converged to zero; clearing local tracking",
		"workspace_id", workspaceID, "deregistered_runtime_ids", dropped,
		"preserved_providers", preserveProviders)

	d.deregisterDroppedRuntimes(ctx, workspaceID, dropped, "zero-runtime convergence", nil)
	d.notifyRuntimeSetChanged()
	return nil
}
