package daemon

import (
	"context"
	"encoding/json"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/daemon/execenv"
	"github.com/orvilo-ai/orvilo/server/pkg/agent"
	"github.com/orvilo-ai/orvilo/server/pkg/taskfailure"
)

// gateResumeToReachableSession clears the task's prior session unless this run
// can actually reach the store the session lives in, and reports whether that
// held. Most CLI backends key their session stores to the cwd (Claude Code
// looks sessions up under ~/.claude/projects/<encoded-cwd>/), so a session id
// from a different workdir can never resolve: the CLI exits within a second
// and the run fails before doing any work — permanently, because the failed
// run records no session and the next claim serves the same stale pointer
// again. This fires whenever the prior workdir no longer exists (GC'd after
// the issue went done, daemon reinstall, manual cleanup) and execenv.Reuse fell
// back to a fresh Prepare (GitHub #3854).
//
// Pi and OMP are the exception. Their opaque session id is an absolute JSONL
// path under ~/.patchbay/pi-sessions, and the backend passes that path directly
// to --session. The transcript remains resumable when only the task workdir
// changes, so binding it to workdir reuse discards healthy conversation history
// and forces the model to reconstruct it through `patchbay chat history`.
// Antigravity's --conversation id is likewise resolved from its user-home
// transcript store, independent of the task workdir.
//
// A matching workdir is not sufficient on its own. Hermes keys its sessions to
// HERMES_HOME — the per-task overlay under envRoot — not to the cwd, and the
// two keys come apart precisely in the local_directory flow: reuse is disabled
// there (shouldReusePriorWorkdir), so every task builds a fresh overlay with an
// empty state.db, while envWorkDir stays the user's own directory and therefore
// still equals PriorWorkDir. The gate read "reused" and forwarded a session id
// that could not possibly resolve, and Hermes answers an unresolvable resume by
// silently starting over (GH #6806). sessionHomeReachable is the provider's own
// answer to "can a prior session still be found here?" — for Hermes, whether
// the conversation's session store got mounted (execenv.Environment
// HermesSessionStore) — and false drops the resume with the same disclosure as
// a workdir mismatch.
// sameExistingDir reports whether two paths name the same existing directory.
// False when either cannot be stat'd, which is the safe answer for cwd-keyed
// providers: an absent prior workdir means there is nothing to resume from.
func sameExistingDir(a, b string) bool {
	if a == "" || b == "" {
		return false
	}
	ai, err := os.Stat(a)
	if err != nil {
		return false
	}
	bi, err := os.Stat(b)
	if err != nil {
		return false
	}
	return os.SameFile(ai, bi)
}

// isAgentThreadContinuation identifies the source-neutral task-level
// conversation surface. Both fields are server-derived: the message is the
// user turn and the root id is the validated lineage scope. Ordinary issue,
// chat and root tasks must keep their existing best-effort resume behavior.
func isAgentThreadContinuation(task Task) bool {
	return strings.TrimSpace(task.AgentThreadMessage) != "" && strings.TrimSpace(task.AgentThreadRootTaskID) != ""
}

const agentThreadContinuationResumeUnavailableMessage = "Agent thread continuation is unavailable because the previous provider session could not be restored. Start a new task instead."

func gateResumeToReachableSession(task *Task, taskCtx *execenv.TaskContextForEnv, provider, envWorkDir string, sessionHomeReachable bool, taskLog *slog.Logger) bool {
	var reachable bool
	if providerUsesPiSessionFile(provider) {
		reachable = piSessionFilePresent(task.PriorSessionID)
	} else if provider == "antigravity" {
		// Antigravity's --conversation id addresses the transcript under its
		// user-home app-data store. It is independent of the task cwd, so a GC'd
		// workdir must not make a valid conversation look unreachable.
		reachable = task.PriorSessionID != "" && sessionHomeReachable
	} else if provider == "codex" && task.AgentThreadRootTaskID != "" {
		// Source-neutral Agent continuations use a root-scoped Codex session
		// store that survives task workdir GC. The old workdir is therefore not
		// an ownership proof for this provider; rollout presence is checked by
		// gateCodexResumeToRolloutPresence immediately after this gate.
		reachable = task.PriorSessionID != "" && sessionHomeReachable
	} else {
		// Compare the directories, not the spelling. Reuse runs in the canonical
		// path it validated and locked, which need not be character-identical to
		// the PriorWorkDir the server sent — a symlinked workspaces root is enough
		// to make them differ. A string compare would then silently drop the prior
		// session on every follow-up task in that installation.
		reachable = task.PriorWorkDir != "" && sameExistingDir(envWorkDir, task.PriorWorkDir) && sessionHomeReachable
	}
	if !reachable && task.PriorSessionID != "" {
		taskLog.Info("dropping prior session: session store not reachable from this run",
			"provider", provider,
			"session_id", task.PriorSessionID,
			"prior_workdir", task.PriorWorkDir,
			"workdir", envWorkDir,
			"session_home_reachable", sessionHomeReachable,
		)
		task.PriorSessionID = ""
		taskCtx.PriorSessionResumed = false
		// The user expected this run to continue the prior conversation; surface
		// the loss instead of silently restarting (MUL-4424). Set it on BOTH
		// carriers: the notice is rendered from `task` by BuildPrompt (MUL-5377
		// moved it out of the brief), while taskCtx still drives execenv.
		taskCtx.PriorSessionResumeUnavailable = true
		task.PriorSessionResumeUnavailable = true
	}
	return reachable
}

func providerUsesPiSessionFile(provider string) bool {
	if provider == "pi" {
		return true
	}
	desc, ok := agent.BuiltinRuntimeByID(provider)
	return ok && desc.ProtocolFamily == "pi"
}

// piSessionFilePresent proves there is persisted history to resume. It does
// not claim the transcript is idle: the Pi backend takes an exclusive lock for
// the complete child-process lifetime and reports a busy resume as rejected so
// runTask falls back to its existing one-shot fresh-session path.
func piSessionFilePresent(sessionID string) bool {
	if sessionID == "" {
		return false
	}
	info, err := os.Stat(sessionID)
	return err == nil && info.Mode().IsRegular() && info.Size() > 0
}

// sessionHomeReachable reports whether a session recorded by a prior task on
// this conversation can still be found from THIS run's environment, for
// providers that key their sessions somewhere other than the cwd.
//
// Only Hermes does today: its transcripts live in `<HERMES_HOME>/state.db`,
// which is the per-task overlay under envRoot. Forwarding a session id into a
// database that does not hold it is what produced a conversation restarting
// from zero every turn (GH #6806), so the question has to be about the
// database, not about the plumbing:
//
//   - With the conversation's session store mounted, the answer is whether that
//     store actually holds a transcript. A mount onto an empty store is the
//     normal shape of a first turn — and also of a store the GC reclaimed
//     between turns, a switched Hermes profile, or a dangling link left by an
//     older overlay. Reading "mounted" as "resumable" would forward a dead id
//     into every one of those.
//   - With no store, the transcript is the overlay's own task-local file, which
//     survives exactly when this run reused the prior task's env root.
//
// Every other provider is keyed by cwd or handles its independently addressed
// store in gateResumeToReachableSession, so this predicate returns true.
func sessionHomeReachable(provider string, env *execenv.Environment, envReused bool) bool {
	if provider != "hermes" {
		return true
	}
	if env.HermesSessionStore != "" {
		return env.HermesSessionHistoryPresent
	}
	return envReused
}

// shouldReusePriorWorkdir keeps the local_directory lock and cross-agent
// isolation invariants without forcing managed follow-ups onto a fresh
// provider session. Every managed issue or chat task may reuse only directories
// that resolve to the two-segment managed root shape, carry Prepare-time
// managed-env provenance for the same workspace/scope/agent, and carry a
// matching daemon task-context marker. Other task kinds have no durable scope
// with which to prove ownership and therefore start fresh.
//
// Reuse eligibility is deliberately keyed off .managed_env.json (written by
// execenv.Prepare) and NOT .gc_meta.json (written only after the task reaches
// terminal state). The server's task-complete handler reconciles a follow-up
// and wakes the runtime before the prior task's daemon handler writes the GC
// file, so a successor can be claimed inside that window; keying off the
// terminal file raced and dropped the session (MUL-4886). Both proofs this
// function reads — the env-root provenance and the workdir task-context marker
// — are written at Prepare time, so neither depends on completion ordering.
func shouldReusePriorWorkdir(task Task, localAssignment *localDirectoryAssignment, workspacesRoot string) (string, bool) {
	if task.PriorWorkDir == "" || localAssignment != nil {
		return "", false
	}

	root, err := filepath.EvalSymlinks(workspacesRoot)
	if err != nil {
		return "", false
	}
	workdir, err := filepath.EvalSymlinks(task.PriorWorkDir)
	if err != nil {
		return "", false
	}
	info, err := os.Stat(workdir)
	if err != nil || !info.IsDir() {
		return "", false
	}
	rel, err := filepath.Rel(root, workdir)
	if err != nil || !filepath.IsLocal(rel) {
		return "", false
	}
	parts := strings.Split(rel, string(filepath.Separator))
	if len(parts) != 3 || parts[0] == "" || parts[1] == "" || parts[2] != "workdir" {
		return "", false
	}
	if task.AgentID == "" || (task.IssueID == "" && task.ChatSessionID == "" && task.AgentThreadRootTaskID == "") {
		return "", false
	}
	// Managed-env provenance is written only for non-local resumable envs, so
	// its presence (plus the workspace/scope/agent match) proves this is a
	// safe daemon-managed reuse target and not a residual local_directory path.
	prov, err := execenv.ReadManagedEnvProvenance(filepath.Dir(workdir))
	if err != nil || prov.ManagedBy != execenv.ManagedEnvProvenanceManagedBy ||
		prov.WorkspaceID != task.WorkspaceID ||
		prov.AgentID != task.AgentID {
		return "", false
	}

	data, err := os.ReadFile(filepath.Join(workdir, execenv.TaskContextMarkerRelPath))
	if err != nil {
		return "", false
	}
	var marker struct {
		ManagedBy             string `json:"managed_by"`
		AgentID               string `json:"agent_id"`
		RuntimeID             string `json:"runtime_id"`
		IssueID               string `json:"issue_id"`
		ChatSessionID         string `json:"chat_session_id"`
		AgentThreadRootTaskID string `json:"agent_thread_root_task_id"`
	}
	if json.Unmarshal(data, &marker) != nil {
		return "", false
	}
	if marker.ManagedBy != execenv.TaskContextMarkerManagedBy || marker.AgentID != task.AgentID {
		return "", false
	}
	if task.IssueID != "" {
		if prov.IssueID != task.IssueID || marker.IssueID != task.IssueID {
			return "", false
		}
		return workdir, true
	}
	if task.ChatSessionID != "" {
		if prov.ChatSessionID != task.ChatSessionID || marker.ChatSessionID != task.ChatSessionID {
			return "", false
		}
		return workdir, true
	}
	// Source-neutral Agent conversations have no mutable issue/chat foreign key.
	// Their root task id is the durable scope, and the runtime fence prevents a
	// stale path from being adopted by another provider/runtime.
	if prov.IssueID != "" || prov.ChatSessionID != "" || marker.IssueID != "" || marker.ChatSessionID != "" ||
		prov.AgentThreadRootTaskID != task.AgentThreadRootTaskID || marker.AgentThreadRootTaskID != task.AgentThreadRootTaskID {
		return "", false
	}
	if task.RuntimeID == "" || prov.RuntimeID != task.RuntimeID || marker.RuntimeID != task.RuntimeID {
		return "", false
	}
	return workdir, true
}

// gateCodexResumeToRolloutPresence drops the prior Codex session when its
// rollout is not actually present in the task's CODEX_HOME sessions. A reused
// workdir keeps PriorSessionID (gateResumeToReachableSession), but Codex session
// isolation (MUL-4424) means the rollout may be missing: a migrated legacy home
// that could not locate it, or a local_directory task whose shared history was
// pruned. Codex would then silently thread/start from scratch, so we clear the
// resume claim from both the backend (PriorSessionID) and the brief
// (PriorSessionResumed) instead of pretending the conversation continues.
// No-op for non-Codex providers or when there is nothing to resume.
func gateCodexResumeToRolloutPresence(task *Task, taskCtx *execenv.TaskContextForEnv, provider, codexHome string, taskLog *slog.Logger) {
	if provider != "codex" || task.PriorSessionID == "" || codexHome == "" {
		return
	}
	if execenv.CodexResumeRolloutPresent(codexHome, task.PriorSessionID) {
		return
	}
	taskLog.Warn("dropping prior codex session: rollout not present in task CODEX_HOME; starting a fresh thread",
		"session_id", task.PriorSessionID, "codex_home", codexHome)
	task.PriorSessionID = ""
	taskCtx.PriorSessionResumed = false
	// The user expected this run to continue the prior conversation; surface the
	// loss instead of silently restarting (MUL-4424). Set it on BOTH carriers:
	// the notice is rendered from `task` by BuildPrompt (MUL-5377 moved it out
	// of the brief), while taskCtx still drives execenv.
	taskCtx.PriorSessionResumeUnavailable = true
	task.PriorSessionResumeUnavailable = true
}

const (
	// codexRolloutFlushWait bounds how long the daemon waits for Codex to finish
	// writing a session's rollout into the per-task store before treating that
	// session as unrecoverable. Codex streams the rollout as the thread runs, so
	// in the common case the file already exists and the check returns at once;
	// the wait only adds latency on the rare path where the rollout never lands
	// (an early crash/error) — which is exactly the pointer we must not persist.
	codexRolloutFlushWait    = 2 * time.Second
	codexRolloutPollInterval = 50 * time.Millisecond
)

// codexSessionResumable reports whether a Codex session's rollout is present in
// the task's per-issue session store, so the daemon never records a session
// pointer the next follow-up would only discover is unresumable — and then drop
// via gateCodexResumeToRolloutPresence, losing the conversation (MUL-5305). It
// mirrors, at write time, the presence gate the daemon already applies at resume
// time: only a session whose rollout is on disk is worth persisting as the
// resumable pointer.
//
// Non-Codex providers have no rollout store to check (codexHome == "") and are
// always treated as resumable, preserving their existing behavior. Codex is
// given a brief bounded window to finish flushing before we give up, so ordinary
// flush lag is not mistaken for a lost rollout.
func codexSessionResumable(codexHome, sessionID string, wait time.Duration) bool {
	if codexHome == "" || sessionID == "" {
		return true
	}
	deadline := time.Now().Add(wait)
	for {
		if execenv.CodexResumeRolloutPresent(codexHome, sessionID) {
			return true
		}
		if !time.Now().Before(deadline) {
			return false
		}
		time.Sleep(codexRolloutPollInterval)
	}
}

// waitCodexRolloutPresent blocks until sessionID's rollout appears in codexHome's
// per-issue store or ctx is done, returning whether it became present. It backs
// the mid-flight pin: Codex reveals the session id on a single task_started
// status, so a fixed one-shot check would miss a rollout that flushes a beat
// later; polling for the life of the run (bounded by ctx) catches it while never
// outliving the task. Non-Codex providers (codexHome == "") have no rollout to
// verify and return immediately (MUL-5305).
func waitCodexRolloutPresent(ctx context.Context, codexHome, sessionID string) bool {
	if codexHome == "" || sessionID == "" {
		return true
	}
	if execenv.CodexResumeRolloutPresent(codexHome, sessionID) {
		return true
	}
	ticker := time.NewTicker(codexRolloutPollInterval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			// The rollout may have landed just as the run ended — check once more.
			return execenv.CodexResumeRolloutPresent(codexHome, sessionID)
		case <-ticker.C:
			if execenv.CodexResumeRolloutPresent(codexHome, sessionID) {
				return true
			}
		}
	}
}

func shouldRetryTaskWithFreshSession(task Task, result agent.Result, priorSessionID string, tools int32, provider string) bool {
	if isAgentThreadContinuation(task) {
		return false
	}
	return shouldRetryWithFreshSession(result, priorSessionID, tools, provider)
}

// shouldRetryWithFreshSession reports whether a failed run that requested
// --resume should be retried once from a fresh session.
//
// Two independent questions have to both answer yes, and conflating them is
// how this went wrong before:
//
//  1. Would a fresh session even fix this? Only if the resume itself was
//     refused — permanently because the transcript is gone or belongs to
//     another provider account, or transiently because another live run owns
//     it. result.ResumeRejected and result.ResumeRejectedTransient are the
//     backend's positive evidence of those two cases. Both allow a fresh retry,
//     but only the permanent signal retires the prior session from later
//     lookups.
//     Answering by exclusion alone would invert the burden of proof: the
//     failures a new session cures are a small enumerable set, while the
//     ones it cannot are open-ended. A network drop, a 429, a quota trip or
//     a provider 5xx has nothing to do with the session, so resetting it
//     discards the one recoverable thing — the conversation pointer — and
//     re-runs the task for nothing. provider_network in particular is
//     documented resume-safe in internal/service/task.go (retryableReasons,
//     MUL-4910): the platform's own retry is supposed to inherit the session
//     and continue the truncated conversation.
//
//     Not every backend can answer, though, and a false ResumeRejected means
//     different things depending on who produced it: "checked, not a
//     rejection" from a capable backend, "could not tell" from one of the
//     backends in agent.ResumeRejectionUndetectable. That is why provider is a
//     parameter — without it the compatibility branch below would silently
//     apply to every backend, second-guessing capable ones by exclusion.
//
//  2. Is re-running safe? Only if the agent executed no tool. tools == 0 does
//     not prove the run mutated nothing — it proves we observed no tool use,
//     which is the strongest signal available here — but it is what stands
//     between a retry and a duplicated side effect. Comment creation has no
//     idempotency key and a duplicate re-fires its @mention triggers; the
//     retry also reuses the same workdir, which is never reset between
//     attempts, so a retry after real work re-plans on top of its own
//     half-finished commits.
func shouldRetryWithFreshSession(result agent.Result, priorSessionID string, tools int32, provider string) bool {
	if result.Status != "failed" || priorSessionID == "" || tools > 0 {
		return false
	}
	// Positive evidence: the backend proved the resume was refused.
	if result.ResumeRejected || result.ResumeRejectedTransient {
		return true
	}
	// Positive evidence of a different kind: the resume was NOT refused —
	// the transcript loaded fine — and the provider then refused to replay
	// it because a message in it is empty. ResumeRejected is false for every
	// backend here precisely because nothing rejected the resume, which is
	// why this needs its own branch rather than a phrase added to the
	// rejection list.
	//
	// It applies to all 18 backends, not the ResumeRejectionUndetectable
	// subset below, and that is deliberate: this is the one failure class
	// where dropping the session is provably the fix without the backend
	// having to detect anything. The evidence is in the provider's own error
	// text, which the common layer already has. Leaving it to each adapter
	// is how the same bug got fixed three times for Kiro, Kimi and Anthropic
	// while every other backend stayed broken.
	//
	// The tools == 0 gate above still applies unchanged — a run that already
	// used a tool is never re-run, poisoned history or not. Such a task still
	// gets its session retired, just by classifyPoisonedError at report time
	// rather than by an in-turn retry.
	if taskfailure.UnresumableHistory(result.Error) {
		return true
	}
	// Third form of positive evidence, and the same shape of argument: the
	// resume was not refused — the runtime happily rebuilt the session — but
	// the provider identity it rebuilt can no longer resolve its own
	// credentials, so the turn dies with "Could not resolve authentication
	// method" (GH #6777). The credentials are fine; only the session's copy of
	// the provider is broken, which is precisely what a fresh session
	// re-resolves from current config.
	//
	// This is the exception the Result.ResumeRejected doc calls out: adapters
	// must NOT flag auth errors, because a genuine credential failure keeps the
	// session so the platform's own retry can continue the conversation. The
	// distinction is resume-vs-fresh, not the error text — and priorSessionID
	// above already establishes that this run WAS a resume. On a cold run the
	// same error means the config really is wrong and this gate never sees it.
	//
	// Deciding here rather than in each ACP adapter is what makes it correct
	// for every step of the ACP lifecycle: the failure surfaces at
	// session/resume, at session/set_model (a resumed session whose persisted
	// provider was normalised gets a redundant set_model that re-routes to the
	// wrong provider — MUL-5029) or at session/prompt, and only two of those
	// three carry any resume-failure signal today. The final error text carries
	// the phrase on all three.
	//
	// Worst case, the config genuinely is broken: the fresh attempt fails the
	// same way, the user sees the same error once, and the single-retry budget
	// bounds the cost. That is the same trade the branch above already makes.
	if taskfailure.AuthMethodUnresolved(result.Error) {
		return true
	}
	// Everything below is a bounded compatibility path for the backends that
	// cannot answer question 1 at all. For every other backend a false
	// ResumeRejected is a real answer — it checked and this was not a
	// rejection — so the gate stops here rather than second-guessing it by
	// exclusion.
	if !agent.ResumeRejectionUndetectable(provider) {
		return false
	}
	// antigravity, codearts, copilot, cursor, deveco and opencode scrape SessionID out
	// of stream output and have no rejection string captured anywhere, so an
	// empty SessionID is the only thing they can offer. It proves no session
	// was established this run, which is exactly what the gate relied on for
	// every backend before ResumeRejected existed; keeping it preserves their
	// recovery instead of silently removing it.
	//
	// Inventing rejection phrases for these backends would be the alternative,
	// and it is worse: no real output has been captured for any of them, and
	// a false positive discards a recoverable session pointer.
	return result.SessionID == "" && freshSessionMayHelp(result.Error)
}

// reconcileFreshRetryResult picks the authoritative result after the single
// fresh-session retry (see the retry block in runTask). Its one hard invariant:
// the poisoned prior session id — carried only on `first`, whose failure is
// classified unrecoverable so GetLastTaskSession excludes it — must NEVER be
// grafted onto the retry's result, or a later task would resume the bad
// session again and re-form the loop this fix exists to break (GH #5975 review).
//
//   - retryErr != nil: the fresh attempt never produced a result. Keep `first`
//     so the poisoned session stays recorded as unrecoverable.
//   - retry established a new session id: it fully wins, carrying its OWN id.
//     A benign retry failure on a real new session is fine to record — it is
//     not the poisoned one.
//   - retry completed without a session id (e.g. all work via tools): take it,
//     but keep the id EMPTY. Never resurrect the poisoned id as a resumable
//     success.
//   - retry failed AND established no new session: keep `first`. Adopting the
//     second result here is exactly the bug — a second error lacking the
//     oversized-image markers would be classified resume-safe and the poisoned
//     id (were it attached) would be re-selected. We keep the unrecoverable
//     first result and only merge usage.
//
// Usage is merged across both attempts in every branch so billing is complete.
func reconcileFreshRetryResult(first agent.Result, firstUsage map[string]agent.TokenUsage, firstTools int32, retry agent.Result, retryTools int32, retryErr error) (agent.Result, int32) {
	switch {
	case retryErr != nil:
		first.Usage = firstUsage
		return first, firstTools
	case retry.SessionID != "":
		retry.UsageOutsideSession = mergeUsage(firstUsage, retry.UsageOutsideSession)
		retry.Usage = mergeUsage(firstUsage, retry.Usage)
		return retry, retryTools
	case retry.Status == "completed":
		retry.Usage = mergeUsage(firstUsage, retry.Usage)
		return retry, retryTools
	default:
		first.Usage = mergeUsage(firstUsage, retry.Usage)
		return first, firstTools
	}
}

// freshSessionMayHelp reports whether restarting the conversation could
// plausibly fix errText. It answers "is this failure about the session at
// all?", so every reason with a defined non-session remedy — wait, back off,
// top up, re-auth, fix the config, install the binary — is excluded. Those
// keep the session pointer so the platform's own retry can resume the
// truncated conversation.
//
// What is left through is deliberately narrow: unknown, process failure and
// unparseable output. A real resume rejection from one of these backends most
// likely surfaces as exactly that — a non-zero exit or output we cannot
// parse — since none of them reports one explicitly. Context overflow is also
// allowed through, as starting over genuinely can clear it.
func freshSessionMayHelp(errText string) bool {
	switch taskfailure.Classify(errText) {
	case taskfailure.ReasonAgentProviderNetwork,
		taskfailure.ReasonAgentProviderCapacityOrRateLimit,
		taskfailure.ReasonAgentProviderQuotaLimit,
		taskfailure.ReasonAgentProviderServerError,
		taskfailure.ReasonAgentProviderAuthOrAccess,
		taskfailure.ReasonAgentMissingConfig,
		taskfailure.ReasonAgentModelNotFoundOrUnavailable,
		taskfailure.ReasonAgentRuntimeMissingExecutable,
		taskfailure.ReasonAgentRuntimeVersionUnsupported,
		// Defensive: a timeout normally carries its own terminal status and
		// never reaches this gate, but if one is ever classified out of a
		// "failed" result, re-running the whole task is not the answer.
		taskfailure.ReasonAgentTimeout:
		return false
	default:
		return true
	}
}
