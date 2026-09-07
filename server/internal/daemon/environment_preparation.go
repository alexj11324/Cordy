package daemon

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/cli"
	"github.com/orvilo-ai/orvilo/server/internal/daemon/execenv"
	"github.com/orvilo-ai/orvilo/server/internal/daemon/repocache"
	"github.com/orvilo-ai/orvilo/server/pkg/skillbundle"
	"github.com/orvilo-ai/orvilo/server/pkg/taskfailure"
)

// gcMetaForTask classifies a finished task and produces a GCMeta of the right
// kind. The discriminator order matters: a task carrying both an issue_id
// and a chat_session_id (theoretical, not produced today) should be treated
// as a chat task because the chat session is the longer-lived parent record.
//
// Returns ok=false when the task has no recognizable parent (e.g. an
// internal task with no IDs at all). The caller skips writing a meta file
// in that case so the directory falls back to mtime-based orphan cleanup.
func gcMetaForTask(task Task) (execenv.GCMeta, bool) {
	meta := execenv.GCMeta{WorkspaceID: task.WorkspaceID, TaskID: task.ID}
	switch {
	case task.ChatSessionID != "":
		meta.Kind = execenv.GCKindChat
		meta.ChatSessionID = task.ChatSessionID
	case task.AutomationRunID != "":
		meta.Kind = execenv.GCKindAutomationRun
		meta.AutomationRunID = task.AutomationRunID
	case task.IssueID != "":
		meta.Kind = execenv.GCKindIssue
		meta.IssueID = task.IssueID
	case task.QuickCreatePrompt != "":
		// Quick-create tasks reach WriteGCMeta before the server runs
		// LinkTaskToIssue, so IssueID is always empty here. Persist the
		// task ID instead and let the GC loop ask the server for terminal
		// state via the task gc-check endpoint.
		meta.Kind = execenv.GCKindQuickCreate
		meta.TaskID = task.ID
	default:
		return execenv.GCMeta{}, false
	}
	return meta, true
}

func taskRootDirParams(workspacesRoot string, task Task) execenv.RootDirParams {
	return execenv.RootDirParams{
		WorkspacesRoot:  workspacesRoot,
		WorkspaceID:     task.WorkspaceID,
		WorkspaceSlug:   task.WorkspaceSlug,
		TaskID:          task.ID,
		IssueIdentifier: task.IssueIdentifier,
	}
}

func (d *Daemon) ensureTaskSkillBundles(ctx context.Context, task *Task) error {
	if task == nil || task.Agent == nil || len(task.Agent.SkillRefs) == 0 {
		return nil
	}
	resolved := make(map[string]SkillData, len(task.Agent.SkillRefs))
	misses := make([]SkillRefData, 0)
	for _, ref := range task.Agent.SkillRefs {
		ref := ref
		var bundle SkillData
		if err := d.skillCache.WithRefLock(task.WorkspaceID, ref, func() error {
			if cached, ok := d.skillCache.Load(task.WorkspaceID, ref); ok {
				bundle = cached
				return nil
			}
			misses = append(misses, ref)
			return nil
		}); err != nil {
			return fmt.Errorf("load skill bundle cache: %w", err)
		}
		if bundle.ID != "" {
			resolved[skillRefKey(ref.Source, ref.ID)] = bundle
		}
	}

	// Resolve each missing bundle in its own request, caching it the moment it
	// arrives. The download is the slow part on jittery links, so fetching the
	// whole set in one atomic body read meant a single timeout discarded all
	// progress and the cache never converged — every dispatch re-downloaded
	// everything and timed out again. Per-skill, each download fits its own
	// size-scaled deadline and is persisted independently, so even a dispatch
	// that ultimately fails leaves the skills it did fetch cached for the next
	// one. (GitHub #4505 / MUL-3650)
	for _, ref := range misses {
		started := time.Now()
		bundle, stats, err := d.resolveSkillBundle(ctx, task, ref)
		if err != nil {
			if isSkillBundleTransferFailure(err) {
				// Only transport failures and incomplete 2xx bodies get the
				// network diagnosis. HTTP error responses and invalid complete
				// payloads retain their server semantics instead of being
				// relabelled as connectivity problems (GitHub #7386).
				return fmt.Errorf("%w: %s: %w",
					errSkillBundleUnavailable,
					describeSkillBundleFailure(ref, stats, time.Since(started)),
					err)
			}
			return fmt.Errorf("%w: %w", errSkillBundleUnavailable, err)
		}
		resolved[skillRefKey(bundle.Source, bundle.ID)] = bundle
	}

	skills := make([]SkillData, 0, len(task.Agent.SkillRefs))
	for _, ref := range task.Agent.SkillRefs {
		bundle, ok := resolved[skillRefKey(ref.Source, ref.ID)]
		if !ok {
			return fmt.Errorf("skill bundle missing after resolve: skill_id=%s source=%s hash=%s", ref.ID, ref.Source, ref.Hash)
		}
		skills = append(skills, bundle)
	}
	task.Agent.Skills = skills
	return nil
}

// resolveSkillBundle downloads one skill bundle and writes it to the on-disk
// cache before returning. The request runs under its own deadline, scaled to
// the bundle's declared size rather than the daemon's fixed 30s control-plane
// timeout, so a large bundle on a slow link is given room to finish instead of
// being cut off mid-body. Caching on success is what lets the resolve converge
// across dispatches. (GitHub #4505 / MUL-3650)
func (d *Daemon) resolveSkillBundle(ctx context.Context, task *Task, ref SkillRefData) (SkillData, TransferStats, error) {
	reqCtx, cancel := context.WithTimeout(ctx, skillBundleResolveTimeout(ref.SizeBytes))
	defer cancel()

	bundle, stats, err := d.client.ResolveSkillBundle(reqCtx, task.RuntimeID, task.ID, ref)
	if err != nil {
		return SkillData{}, stats, err
	}
	// The resolve endpoint serves the agent's *current* bundle and hash, which
	// may differ from the claim-time ref when the skill was edited between
	// claim and prepare (see ResolveTaskSkillBundles). So confirm only that the
	// server returned the skill we asked for (source/id), then validate the
	// bundle for self-consistency against a ref derived from itself — pinning
	// it to the possibly-stale requested hash would reject a legitimate update.
	if bundle.Source != ref.Source || bundle.ID != ref.ID {
		return SkillData{}, stats, fmt.Errorf("resolve skill bundle returned wrong skill: requested source=%s id=%s, got source=%s id=%s", ref.Source, ref.ID, bundle.Source, bundle.ID)
	}
	bundleRef := skillRefFromBundle(bundle)
	validationRef := bundleRef
	if ref.Source == skillbundle.SourcePlugin {
		validationRef = ref
	}
	if !validateSkillBundle(validationRef, bundle) {
		return SkillData{}, stats, fmt.Errorf("resolve skill bundle returned invalid bundle: skill_id=%s source=%s hash=%s", bundle.ID, bundle.Source, bundle.Hash)
	}
	if err := d.skillCache.WithRefLock(task.WorkspaceID, validationRef, func() error {
		return d.skillCache.Store(task.WorkspaceID, bundle)
	}); err != nil {
		if d.logger != nil {
			d.logger.Warn("skill bundle cache store failed; continuing with downloaded bundle",
				"workspace_id", task.WorkspaceID,
				"skill_id", bundle.ID,
				"source", bundle.Source,
				"hash", bundle.Hash,
				"error", err,
			)
		}
	}
	return bundle, stats, nil
}

// isSkillBundleTransferFailure identifies errors for which byte-level network
// diagnostics are meaningful. A requestError is an explicit server response;
// malformed but complete JSON and bundle-validation errors are server payload
// problems. Neither should be presented as a slow or dead network link. In
// particular, json.Decoder can synthesize io.ErrUnexpectedEOF after its source
// ended with a clean EOF, so that decoder error alone is not transport proof.
func isSkillBundleTransferFailure(err error) bool {
	var reqErr *requestError
	if errors.As(err, &reqErr) {
		return false
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return true
	}
	var netErr net.Error
	return errors.As(err, &netErr)
}

// describeSkillBundleFailure renders the diagnostic half of a failed bundle
// download: what was being fetched, how many HTTP response-body bytes arrived,
// the separately declared decoded content size, and whether this host has a
// proxy at all.
//
// The three facts answer the three wrong turns the old text invited. "network
// error downloading skill X" instead of "skill X unavailable" stops the reader
// blaming the skill. The response byte count distinguishes a dead link from a
// partial response without pretending JSON wire bytes and decoded skill-content
// bytes form one progress ratio. The proxy note catches the case behind GitHub
// #7386, where a daemon that inherited no proxy on a cross-border link never got
// a byte through while the same host succeeded through a local proxy.
func describeSkillBundleFailure(ref SkillRefData, stats TransferStats, elapsed time.Duration) string {
	elapsed = elapsed.Round(time.Millisecond)
	var transfer string
	switch {
	case !stats.ResponseStarted:
		transfer = fmt.Sprintf("no successful response from server after %s overall", elapsed)
	case stats.BytesRead == 0:
		transfer = fmt.Sprintf("a successful response started but delivered no body bytes before failing after %s overall", elapsed)
	default:
		// BytesRead is a single-attempt high-water mark, while elapsed covers
		// the logical call including retries and backoff. State both scopes
		// rather than deriving a rate from mismatched measurements.
		transfer = fmt.Sprintf("received up to %s of response body data in one attempt; failed after %s overall",
			formatBytes(stats.BytesRead), elapsed)
	}
	return fmt.Sprintf("network error downloading skill %q (id=%s): %s; declared skill content size %s; %s; the skill content is not at fault",
		ref.Name, ref.ID, transfer, formatBytes(ref.SizeBytes), proxyEnvSummary())
}

// proxyEnvSummary reports whether the daemon inherited any proxy setting. It
// names the variable but never its value: proxy URLs routinely embed
// credentials, and this string lands in task failure text the whole workspace
// can read.
//
// Go's transport only consults these variables — it does not read the Windows
// system proxy — and it reads them at process start, so a daemon that was
// already running when they were set still shows none (GitHub #7386).
func proxyEnvSummary() string {
	for _, key := range []string{"HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"} {
		if strings.TrimSpace(os.Getenv(key)) != "" {
			return fmt.Sprintf("proxy configured (%s)", key)
		}
	}
	return "no proxy configured (HTTPS_PROXY unset)"
}

// formatBytes renders a byte count for humans reading a failure message.
func formatBytes(n int64) string {
	switch {
	case n < 0:
		return "unknown size"
	case n < 1024:
		return fmt.Sprintf("%d B", n)
	case n < 1024*1024:
		return fmt.Sprintf("%.0f KB", float64(n)/1024)
	default:
		return fmt.Sprintf("%.2f MB", float64(n)/(1024*1024))
	}
}

const (
	// skillBundleResolveMinTimeout floors the per-skill resolve deadline so a
	// tiny bundle still tolerates connection setup and round-trip latency.
	skillBundleResolveMinTimeout = 30 * time.Second
	// skillBundleResolveMaxTimeout caps it so a wedged download cannot pin a
	// task in prepare indefinitely.
	skillBundleResolveMaxTimeout = 5 * time.Minute
	// skillBundleResolveMinThroughput is the pessimistic floor throughput
	// (bytes/sec) used to scale the deadline to bundle size — deliberately low
	// to cover slow, jittery links rather than ideal bandwidth.
	skillBundleResolveMinThroughput = 50 * 1024
)

// skillBundleResolveTimeout returns the deadline budget for downloading a
// bundle of the given size: at least skillBundleResolveMinTimeout, scaled up at
// skillBundleResolveMinThroughput, and capped at skillBundleResolveMaxTimeout.
func skillBundleResolveTimeout(sizeBytes int64) time.Duration {
	if sizeBytes <= 0 {
		return skillBundleResolveMinTimeout
	}
	scaled := time.Duration(sizeBytes/skillBundleResolveMinThroughput) * time.Second
	if scaled < skillBundleResolveMinTimeout {
		return skillBundleResolveMinTimeout
	}
	if scaled > skillBundleResolveMaxTimeout {
		return skillBundleResolveMaxTimeout
	}
	return scaled
}

func (d *Daemon) startTaskPrepareLeaseExtender(ctx context.Context, task Task, taskLog *slog.Logger) func() {
	leaseCtx, cancel := context.WithCancel(ctx)
	done := make(chan struct{})
	go func() {
		defer close(done)
		ticker := time.NewTicker(taskPrepareLeaseRefresh)
		defer ticker.Stop()
		for {
			select {
			case <-leaseCtx.Done():
				return
			case <-ticker.C:
				reqCtx, reqCancel := context.WithTimeout(leaseCtx, taskPrepareLeaseTimeout)
				err := d.client.ExtendTaskPrepareLease(reqCtx, task.RuntimeID, task.ID)
				reqCancel()
				if err != nil {
					taskLog.Warn("extend task prepare lease failed", "error", err)
				}
			}
		}
	}()

	var once sync.Once
	return func() {
		once.Do(func() {
			cancel()
			<-done
		})
	}
}

// lockReusablePriorEnvRoot decides whether this task may continue in a prior
// task's env root and, if so, takes the exclusion lock on it.
//
// Order matters and is the point of this function. The lock WRITES a file into
// the directory, so eligibility has to be proven first:
// shouldReusePriorWorkdir resolves symlinks, checks the
// {workspace}/{task}/workdir shape and verifies managed provenance, and only
// the canonical path it returns is ever opened. Locking on
// filepath.Dir(task.PriorWorkDir) before that would drop .task_lock into
// whatever the path happened to point at — a symlink target outside the
// workspaces root, or a user's own local_directory.
//
// The re-check under the lock closes the gap between proving eligibility and
// acting on it. Declining is always safe: the caller falls back to a fresh
// Prepare, which costs session continuity, not correctness.
//
// The error return is the one outcome that is NOT "decline and carry on": it is
// non-nil only when the run was cancelled while waiting for the prior env root,
// and it carries the context's cause so the caller can end the task instead of
// preparing an environment for work that no longer exists.
func (d *Daemon) lockReusablePriorEnvRoot(ctx context.Context, task Task, localAssignment *localDirectoryAssignment, heldRoot string) (*execenv.EnvRootClaim, string, os.FileInfo, bool, error) {
	// Pin the workspaces root BEFORE validating anything. Opening it after,
	// from a name validation just approved, would re-resolve that name: rename
	// the root aside, leave a symlink to a look-alike tree, and os.Root
	// faithfully pins the replacement. Root promises you cannot escape the tree
	// it opened — not that it opened the tree you meant. Holding the handle
	// first makes that ordering impossible.
	wsRoot, err := os.OpenRoot(d.cfg.WorkspacesRoot)
	if err != nil {
		return nil, "", nil, false, nil
	}
	defer wsRoot.Close()

	workDir, ok := shouldReusePriorWorkdir(task, localAssignment, d.cfg.WorkspacesRoot)
	if !ok {
		return nil, "", nil, false, nil
	}
	priorRoot := filepath.Dir(workDir)
	// workDir came back through EvalSymlinks, so the root it is measured
	// against has to be resolved the same way — otherwise a symlinked
	// workspaces root (macOS /tmp -> /private/tmp, a home on a linked volume)
	// makes the two look unrelated and every reuse is refused.
	canonicalWorkspacesRoot, err := filepath.EvalSymlinks(d.cfg.WorkspacesRoot)
	if err != nil {
		return nil, "", nil, false, nil
	}
	rel, err := filepath.Rel(canonicalWorkspacesRoot, priorRoot)
	if err != nil || !filepath.IsLocal(rel) {
		return nil, "", nil, false, nil
	}
	// Already covered by this run's own claim (a task re-dispatched onto its
	// own directory); taking a second lock on it would only deadlock against
	// ourselves.
	if priorRoot == heldRoot {
		return nil, workDir, nil, true, nil
	}

	// Pin the identity of the directory that just passed validation. Every
	// later step is checked against THIS, not against whatever the name
	// resolves to next: a path string cannot tell "the directory I validated"
	// apart from "a different directory now answering to that name".
	validatedInfo, err := os.Stat(priorRoot)
	if err != nil {
		return nil, "", nil, false, nil
	}

	// Deterministic seam for the TOCTOU regressions: tests swap the validated
	// directory here, between proving eligibility and taking the lock.
	if reuseLockTestHook != nil {
		reuseLockTestHook()
	}

	lockStartedAt := time.Now()
	claim, lockedInfo, err := d.lockEnvRootForReuseWaitingOutTheBusyWindow(ctx, wsRoot, rel, priorRoot, task)
	switch {
	case errors.Is(err, errPriorEnvRootWaitAborted):
		// The run is over. Declining reuse would hand the caller on to a fresh
		// Prepare, which is the one thing that must not happen here.
		return nil, "", nil, false, context.Cause(ctx)
	case errors.Is(err, execenv.ErrEnvRootBusy):
		d.logger.Info("prior workdir is still in use after waiting for it; starting a fresh environment",
			"task", shortID(task.ID), "prior_root", filepath.Base(priorRoot),
			"waited", time.Since(lockStartedAt).Round(time.Millisecond), "budget", d.envRootBusyWait)
		return nil, "", nil, false, nil
	case err != nil:
		d.logger.Warn("could not lock prior workdir; starting a fresh environment",
			"task", shortID(task.ID), "error", err)
		return nil, "", nil, false, nil
	case claim == nil:
		return nil, "", nil, false, nil
	}
	// The lock has to have landed on the directory validation approved.
	if !os.SameFile(validatedInfo, lockedInfo) {
		d.logger.Info("prior workdir changed identity before it could be claimed; starting a fresh environment",
			"task", shortID(task.ID))
		claim.Release()
		return nil, "", nil, false, nil
	}

	// Re-validate while holding the lock, and require the answer to be the SAME
	// directory. Equality of the canonical path is not enough on its own: the
	// tree can be rearranged so a different-but-equally-valid env root now
	// answers to that name, which would leave us holding a lock on one
	// directory while handing another to Reuse. os.SameFile compares identity,
	// not spelling.
	recheckedWorkDir, stillOK := shouldReusePriorWorkdir(task, localAssignment, d.cfg.WorkspacesRoot)
	if !stillOK || recheckedWorkDir != workDir {
		claim.Release()
		return nil, "", nil, false, nil
	}
	// ...and the directory we are about to hand to Reuse has to be that same
	// one, so the object locked and the object used cannot diverge.
	currentInfo, err := os.Stat(filepath.Dir(recheckedWorkDir))
	if err != nil || !os.SameFile(lockedInfo, currentInfo) {
		d.logger.Info("prior workdir changed identity while being claimed; starting a fresh environment",
			"task", shortID(task.ID))
		claim.Release()
		return nil, "", nil, false, nil
	}
	return claim, workDir, lockedInfo, true, nil
}

// envRootBusyRetryInterval is how often the wait below re-tries the lock. The
// window it is covering is seconds long, so this only has to be small relative
// to that, not tight.
const envRootBusyRetryInterval = 250 * time.Millisecond

// errPriorEnvRootWaitAborted marks the wait as ended by the run itself rather
// than by the lock. It wraps the context's cause so the task can be failed with
// the real reason.
var errPriorEnvRootWaitAborted = errors.New("waiting for the prior env root was aborted")

// lockEnvRootForReuseWaitingOutTheBusyWindow takes the reuse lock, waiting out
// a lock the PREVIOUS run of this (issue, agent) has not let go of yet.
//
// A busy lock here has one cause in a healthy system: the server hands a task
// to a daemon only when no other task for the same (issue, agent) is dispatched
// or running (ClaimAgentTask's serialization), so by the time this task exists
// its predecessor is already finished as far as the server is concerned. The
// process is not: it learns of its cancellation from its own poll tick and
// takes a few seconds to exit, and it holds .task_lock until it does. That is
// the whole of the gap — a machine-local fact, visible right here.
//
// Giving up immediately costs the workdir, and with it the provider session
// living in it: the agent restarts with no memory of the conversation the user
// was in the middle of editing (MUL-6880). Waiting costs a few seconds of one
// task slot, which is far less than the fresh Prepare — including a repo
// checkout — that declining forces instead.
//
// The wait is bounded and gives up into exactly the old behaviour, so a
// predecessor that is genuinely wedged, or a lock held by something else on a
// shared workspaces root, still ends in a fresh environment rather than a
// stuck task.
func (d *Daemon) lockEnvRootForReuseWaitingOutTheBusyWindow(
	ctx context.Context,
	wsRoot *os.Root,
	rel, priorRoot string,
	task Task,
) (*execenv.EnvRootClaim, os.FileInfo, error) {
	start := time.Now()
	deadline := start.Add(d.envRootBusyWait)
	waited := false
	for {
		claim, info, err := execenv.LockEnvRootForReuse(wsRoot, rel, priorRoot)
		if !errors.Is(err, execenv.ErrEnvRootBusy) || !time.Now().Before(deadline) {
			// The wait's real duration is logged, not the budget: 15s is a
			// reasoned guess (one cancel-poll interval plus the agent's exit),
			// and these lines are how it gets checked against production
			// instead of staying a guess.
			if waited && err == nil {
				d.logger.Info("prior workdir freed while waiting for the previous run to exit",
					"task", shortID(task.ID), "prior_root", filepath.Base(priorRoot),
					"waited", time.Since(start).Round(time.Millisecond))
			}
			return claim, info, err
		}
		if !waited {
			waited = true
			d.logger.Info("prior workdir is still held by the previous run; waiting for it",
				"task", shortID(task.ID), "prior_root", filepath.Base(priorRoot),
				"budget", d.envRootBusyWait)
		}
		select {
		case <-ctx.Done():
			d.logger.Info("stopped waiting for the prior workdir: the run was cancelled",
				"task", shortID(task.ID), "prior_root", filepath.Base(priorRoot),
				"waited", time.Since(start).Round(time.Millisecond))
			// NOT the busy error the loop was carrying. A cancelled run is a
			// third outcome, and collapsing it into "still busy" would send the
			// caller on to a fresh Prepare for work nobody is waiting for, and
			// would file one cancellation under both "cancelled" and "budget
			// exhausted" in the very logs the budget is meant to be judged by.
			return nil, nil, fmt.Errorf("%w: %w", errPriorEnvRootWaitAborted, context.Cause(ctx))
		case <-time.After(envRootBusyRetryInterval):
		}
	}
}

// reuseLockTestHook runs between reuse eligibility validation and taking the
// prior-root lock. Nil outside tests; the TOCTOU regressions use it to make the
// swap deterministic instead of racing it.
var reuseLockTestHook func()

// reuseBeforeUseTestHook runs after the prior-root claim is settled and before
// Reuse resolves that path by name. Nil outside tests.
var reuseBeforeUseTestHook func()

func (d *Daemon) prepareExecutionEnvironment(ctx context.Context, params execenv.PrepareParams) (*execenv.Environment, error) {
	if d.executionEnvironmentCommand == nil {
		// Focused runTask tests construct a zero-valued Daemon and keep setup
		// in-process. Production Daemons created by New always use isolation.
		return execenv.Prepare(params, d.logger)
	}
	command, err := d.executionEnvironmentCommand()
	if err != nil {
		return nil, err
	}
	return execenv.PrepareIsolated(ctx, command, params, d.logger)
}

func (d *Daemon) reuseExecutionEnvironment(ctx context.Context, params execenv.ReuseParams) (*execenv.Environment, error) {
	if d.executionEnvironmentCommand == nil {
		return execenv.Reuse(params, d.logger), nil
	}
	command, err := d.executionEnvironmentCommand()
	if err != nil {
		return nil, err
	}
	return execenv.ReuseIsolated(ctx, command, params, d.logger)
}

func (d *Daemon) effectiveTaskPrepareTimeout() time.Duration {
	if d.taskPrepareTimeout > 0 {
		return d.taskPrepareTimeout
	}
	return defaultTaskPrepareTimeout
}

func skillRefKey(source, id string) string {
	return source + "\x00" + id
}

func skillRefFromBundle(bundle SkillData) SkillRefData {
	files := make([]skillbundle.File, 0, len(bundle.Files))
	for _, file := range bundle.Files {
		files = append(files, skillbundle.File{Path: file.Path, Content: file.Content})
	}
	manifest := skillbundle.BuildManifest(skillbundle.Skill{
		ID:          bundle.ID,
		Source:      bundle.Source,
		Name:        bundle.Name,
		Description: bundle.Description,
		Content:     bundle.Content,
		Files:       files,
	})
	fileRefs := make([]SkillFileRefData, 0, len(manifest.Files))
	for _, file := range manifest.Files {
		fileRefs = append(fileRefs, SkillFileRefData{Path: file.Path, SHA256: file.SHA256, SizeBytes: file.SizeBytes})
	}
	return SkillRefData{
		ID:        bundle.ID,
		Source:    bundle.Source,
		Hash:      manifest.Hash,
		SizeBytes: manifest.SizeBytes,
		FileCount: manifest.FileCount,
		Files:     fileRefs,
	}
}

// repoDataToInfo converts daemon RepoData to repocache RepoInfo.
func repoDataToInfo(repos []RepoData) []repocache.RepoInfo {
	info := make([]repocache.RepoInfo, len(repos))
	for i, r := range repos {
		info[i] = repocache.RepoInfo{URL: r.URL}
	}
	return info
}

func convertIssueStatusesForEnv(statuses []IssueStatusData) []execenv.IssueStatusForEnv {
	if len(statuses) == 0 {
		return nil
	}
	result := make([]execenv.IssueStatusForEnv, len(statuses))
	for i, s := range statuses {
		result[i] = execenv.IssueStatusForEnv{Key: s.Key, Name: s.Name, Category: s.Category, Description: s.Description}
	}
	return result
}

func convertReposForEnv(repos []RepoData) []execenv.RepoContextForEnv {
	if len(repos) == 0 {
		return nil
	}
	result := make([]execenv.RepoContextForEnv, len(repos))
	for i, r := range repos {
		result[i] = execenv.RepoContextForEnv{URL: r.URL, Description: r.Description, Ref: r.Ref}
	}
	return result
}

func convertProjectResourcesForEnv(resources []ProjectResourceData) []execenv.ProjectResourceForEnv {
	if len(resources) == 0 {
		return nil
	}
	result := make([]execenv.ProjectResourceForEnv, len(resources))
	for i, r := range resources {
		result[i] = execenv.ProjectResourceForEnv{
			ID:           r.ID,
			ResourceType: r.ResourceType,
			ResourceRef:  r.ResourceRef,
			Label:        r.Label,
		}
	}
	return result
}

func convertSkillsForEnv(skills []SkillData) []execenv.SkillContextForEnv {
	if len(skills) == 0 {
		return nil
	}
	result := make([]execenv.SkillContextForEnv, len(skills))
	for i, s := range skills {
		result[i] = execenv.SkillContextForEnv{
			Name:        s.Name,
			Description: s.Description,
			Content:     s.Content,
		}
		for _, f := range s.Files {
			result[i].Files = append(result[i].Files, execenv.SkillFileContextForEnv{
				Path:    f.Path,
				Content: f.Content,
			})
		}
	}
	return result
}

func convertDisabledRuntimeSkillsForEnv(agentData *AgentData, runtimeID, provider string) []execenv.RuntimeSkillRefForEnv {
	if agentData == nil || len(agentData.DisabledRuntimeSkills) == 0 {
		return nil
	}
	result := make([]execenv.RuntimeSkillRefForEnv, 0, len(agentData.DisabledRuntimeSkills))
	for _, skill := range agentData.DisabledRuntimeSkills {
		if skill.RuntimeID != runtimeID || skill.Provider != provider {
			continue
		}
		result = append(result, execenv.RuntimeSkillRefForEnv{
			Root:   skill.Root,
			Key:    skill.Key,
			Name:   skill.Name,
			Plugin: skill.Plugin,
		})
	}
	return result
}

// composeOpenclawIncludeRoots returns the value the daemon should set for
// OPENCLAW_INCLUDE_ROOTS on the child openclaw process so its `$include`
// loader will follow the wrapper's reference out of envRoot into the
// user's active config directory.
//
// addRoot is the directory we must grant (typically dirname of the user's
// active openclaw.json). userValue is whatever the daemon's own
// environment already has under OPENCLAW_INCLUDE_ROOTS — the user's own
// cross-directory layout. We prepend addRoot, dedupe by string equality,
// drop empty path segments, and return ok=false when there's nothing to
// grant (addRoot is empty — fresh install case), so callers can leave the
// env var alone in that case.
//
// Path separator is the OS-native list separator (`:` on Unix, `;` on
// Windows) to match how OpenClaw splits the env var.
func composeOpenclawIncludeRoots(addRoot, userValue string) (string, bool) {
	if addRoot == "" {
		return "", false
	}
	parts := []string{addRoot}
	seen := map[string]struct{}{addRoot: {}}
	for _, p := range strings.Split(userValue, string(os.PathListSeparator)) {
		if p == "" {
			continue
		}
		if _, dup := seen[p]; dup {
			continue
		}
		seen[p] = struct{}{}
		parts = append(parts, p)
	}
	return strings.Join(parts, string(os.PathListSeparator)), true
}

// ensureTaskTempDir creates this task's private temp directory and returns it
// with its execution lock held. The caller owns the lock for the lifetime of
// the run and must release it before removing the directory — see the cleanup
// defer in runTask, and execenv.PruneTaskTempDirs for what the lock buys.
func ensureTaskTempDir(envRoot string, workspaceID string, taskID string) (string, *os.File, error) {
	envRoot = strings.TrimSpace(envRoot)
	if envRoot == "" {
		return "", nil, errors.New("env root is empty")
	}
	workspaceID = strings.TrimSpace(workspaceID)
	if workspaceID == "" {
		return "", nil, errors.New("workspace id is empty")
	}
	taskID = strings.TrimSpace(taskID)
	if taskID == "" {
		return "", nil, errors.New("task id is empty")
	}
	base, overrideConfigured, err := taskTempBaseDir()
	if err != nil {
		return "", nil, err
	}
	dir, err := os.MkdirTemp(base, execenv.TaskTempDirPrefix)
	if err != nil {
		if overrideConfigured {
			return "", nil, fmt.Errorf("ORVILO_AGENT_TEMP_BASE: create task temp dir: %w", err)
		}
		return "", nil, err
	}
	if err := os.Chmod(dir, 0o700); err != nil {
		_ = os.RemoveAll(dir)
		return "", nil, err
	}
	lock, err := execenv.LockTaskTempDir(dir)
	if err != nil {
		_ = os.RemoveAll(dir)
		return "", nil, err
	}
	return dir, lock, nil
}

// taskTempBaseDir resolves the parent directory for private per-task temp
// dirs on Linux and macOS. The daemon operator can relocate it with
// ORVILO_AGENT_TEMP_BASE, which must be an absolute path to an existing,
// writable directory; an invalid value fails task startup instead of silently
// falling back. Windows ignores the variable. Unset keeps the platform default
// exactly as before, down to the syscalls made.
// Operators should pick a short path: child tools may bind AF_UNIX sockets
// under $TMPDIR (sun_path is 108 bytes on Linux, 104 on macOS).
func taskTempBaseDir() (string, bool, error) {
	if runtime.GOOS == "windows" {
		return socketSafeTempBaseDir(), false, nil
	}
	base := strings.TrimSpace(os.Getenv("ORVILO_AGENT_TEMP_BASE"))
	if base == "" {
		return socketSafeTempBaseDir(), false, nil
	}
	if !filepath.IsAbs(base) {
		return "", true, fmt.Errorf("ORVILO_AGENT_TEMP_BASE must be an absolute path, got %q", base)
	}
	return base, true, nil
}

func socketSafeTempBaseDir() string {
	if os.PathSeparator == '/' {
		if info, err := os.Stat("/tmp"); err == nil && info.IsDir() {
			return "/tmp"
		}
	}
	return os.TempDir()
}

// isBlockedEnvKey returns true if the key must not be overridden by user-
// configured custom_env. This prevents accidental or malicious override of
// daemon-internal variables and critical system paths.
func isBlockedEnvKey(key string) bool {
	upper := strings.ToUpper(key)
	if strings.HasPrefix(upper, "ORVILO_") {
		return true
	}
	switch upper {
	case "HOME", "PATH", "USER", "SHELL", "TERM", "TMPDIR", "TMP", "TEMP", "CODEX_HOME", "REASONIX_STATE_HOME", "CURSOR_DATA_DIR", execenv.CursorMcpAuthSourceEnv, "OPENCLAW_CONFIG_PATH", "OPENCLAW_INCLUDE_ROOTS":
		return true
	}
	return false
}

// layerCustomEnvAndHermesHome applies the agent's custom_env onto the child env
// (skipping daemon-internal blocklisted keys), then overrides HERMES_HOME with
// the per-task overlay when one was built. HERMES_HOME is intentionally NOT
// blocklisted: with skills bound the overlay is built FROM the user's
// HERMES_HOME and must win here; with no skills bound (overlayHome empty) the
// user's own HERMES_HOME passes through unchanged, so a skill-less Hermes task
// keeps its original behavior.
// sanitizeAgentEnv returns the agent custom_env with daemon-blocklisted keys
// removed — the effective env the Hermes child actually sees, used to expand
// ${VAR} in external_dirs consistently (blocked keys like HOME resolve to the
// daemon process value, not the dropped custom one). Uses the same
// isBlockedEnvKey rule as layerCustomEnvAndHermesHome so the two agree.
func sanitizeAgentEnv(customEnv map[string]string) map[string]string {
	if len(customEnv) == 0 {
		return nil
	}
	out := make(map[string]string, len(customEnv))
	for k, v := range customEnv {
		if isBlockedEnvKey(k) {
			continue
		}
		out[k] = v
	}
	return out
}

// hermesProviderUnconfiguredHint is appended verbatim to a "no LLM provider
// configured" failure. It is a CONSTANT, and that is a correctness property,
// not a style choice — see annotateHermesProviderUnconfigured.
//
// It must stay clear of every phrase the resume guards match, because this text
// is persisted in agent_task_queue.error and re-scanned there indefinitely:
// service.ResumeUnsafeFailure, taskfailure.Classify, and the ILIKE/regex guards
// in pkg/db/queries/agent.sql (GetLastTaskSession / GetLastChatTaskSession).
// TestAnnotationCannotChangeMachineDecisions pins that.
const hermesProviderUnconfiguredHint = " [orvilo] hermes did not read the HERMES_HOME your shell uses: " +
	"this task ran against a per-task overlay, seeded from the home the daemon process resolved. " +
	"The daemon log line \"hermes home resolved\" for this task names that source home — if your hermes " +
	"config lives somewhere else, set HERMES_HOME in the agent's custom_env to point at it."

// annotateHermesProviderUnconfigured explains a "no LLM provider configured"
// failure that Hermes itself cannot explain.
//
// Hermes reports it against whichever HERMES_HOME it was started with and tells
// the user to run `hermes model` — but under Patchbay it was started with a
// per-task overlay, seeded from a source home the daemon resolved from ITS OWN
// process environment. When that disagrees with where the user keeps their
// config, the remedy Hermes names edits a file the task will never read, and
// every attempt fails identically. That is GH #6872: eight documented
// workarounds, none of which could have worked.
//
// The two paths themselves are deliberately NOT interpolated here. They are
// user-controlled (HERMES_HOME comes from the agent's custom_env, the overlay
// root from ORVILO_WORKSPACES_ROOT), and this string is persisted as the
// task's error text, which the resume guards keep matching against for the life
// of the row. A source home under /srv/400-invalid_request_error/ would trip
// ResumeUnsafeFailure and the SQL guard, dropping a healthy session pointer —
// a directory name must never decide whether a session can be resumed. So the
// hint is fixed prose and names the log line that does carry the paths.
//
// Text only: the caller has already classified the failure, and this changes no
// reason, status, or control flow.
func annotateHermesProviderUnconfigured(errMsg, provider string, overlayActive bool) string {
	if provider != "hermes" || !overlayActive || !taskfailure.ProviderUnconfigured(errMsg) {
		return errMsg
	}
	return errMsg + hermesProviderUnconfiguredHint
}

func layerCustomEnvAndHermesHome(agentEnv, customEnv map[string]string, overlayHome string, logger *slog.Logger) {
	for k, v := range customEnv {
		if isBlockedEnvKey(k) {
			if logger != nil {
				logger.Warn("custom_env: blocked key skipped", "key", k)
			}
			continue
		}
		agentEnv[k] = v
	}
	if overlayHome != "" {
		agentEnv["HERMES_HOME"] = overlayHome
	}
}

// prepareReasonixTaskStateHome isolates persisted transcripts and leases per
// (runtime, agent) while leaving REASONIX_HOME untouched. Current Reasonix
// reads credentials/config from REASONIX_HOME and state from
// REASONIX_STATE_HOME, so `reasonix setup` remains the sole credential owner
// and Patchbay never copies API keys into task-managed files.
func prepareReasonixTaskStateHome(profile, runtimeID, agentID string) (string, error) {
	profileDir, err := cli.ProfileDir(profile)
	if err != nil {
		return "", err
	}
	runtimeSegment, err := validateReasonixStateSegment("runtime", runtimeID)
	if err != nil {
		return "", err
	}
	agentSegment, err := validateReasonixStateSegment("agent", agentID)
	if err != nil {
		return "", err
	}
	path := filepath.Join(profileDir, "reasonix-state", runtimeSegment, agentSegment)
	if err := os.MkdirAll(path, 0o700); err != nil {
		return "", err
	}
	if err := os.Chmod(path, 0o700); err != nil {
		return "", err
	}
	return path, nil
}

// prepareDshTaskSessionRoot keeps DSH transcripts private to one Patchbay
// runtime/agent pair. Credentials and the user's DSH profile remain in the
// ordinary DSH_HOME; only session persistence is redirected.
func prepareDshTaskSessionRoot(profile, runtimeID, agentID string) (string, error) {
	profileDir, err := cli.ProfileDir(profile)
	if err != nil {
		return "", err
	}
	runtimeSegment, err := validateReasonixStateSegment("runtime", runtimeID)
	if err != nil {
		return "", err
	}
	agentSegment, err := validateReasonixStateSegment("agent", agentID)
	if err != nil {
		return "", err
	}
	path := filepath.Join(profileDir, "dsh-sessions", runtimeSegment, agentSegment)
	if err := os.MkdirAll(path, 0o700); err != nil {
		return "", err
	}
	if err := os.Chmod(path, 0o700); err != nil {
		return "", err
	}
	return path, nil
}

func validateReasonixStateSegment(name, value string) (string, error) {
	if value == "" {
		return "", fmt.Errorf("%s ID is required", name)
	}
	for _, r := range value {
		switch {
		case r >= 'a' && r <= 'z', r >= 'A' && r <= 'Z', r >= '0' && r <= '9', r == '-', r == '_':
			continue
		default:
			return "", fmt.Errorf("%s ID contains an unsafe path character", name)
		}
	}
	return value, nil
}

// codexShellAuthorizedCustomEnvNames returns names from the current agent's
// custom_env that pass the same daemon blocklist used when assembling the child
// environment. Returning names only keeps credential values out of the managed
// Codex config path.
func codexShellAuthorizedCustomEnvNames(customEnv map[string]string) []string {
	names := make([]string, 0, len(customEnv))
	for key := range customEnv {
		if key == "" || isBlockedEnvKey(key) {
			continue
		}
		names = append(names, key)
	}
	return names
}

// configureCodexTaskShellEnvironment writes the managed shell policy only
// after the task and agent custom environment are fully assembled. This makes
// the allowlist reflect the child environment that will actually be launched,
// including platform-specific essentials and blocklist-checked custom_env
// credentials.
// Failure is fatal: launching with an unowned or malformed policy could either
// drop the task-scoped token again or expose inherited daemon credentials.
func configureCodexTaskShellEnvironment(provider, codexHome string, inherited []string, agentEnv, agentCustomEnv map[string]string, logger *slog.Logger) error {
	if provider != "codex" {
		return nil
	}
	if strings.TrimSpace(codexHome) == "" {
		return errors.New("configure Codex shell environment: task CODEX_HOME is missing")
	}
	authorizedExplicit := codexShellAuthorizedCustomEnvNames(agentCustomEnv)
	includeOnly := execenv.CodexShellEnvAllowlist(inherited, agentEnv, authorizedExplicit)
	configPath := filepath.Join(codexHome, "config.toml")
	if err := execenv.EnsureCodexShellEnvPolicyConfig(configPath, includeOnly, logger); err != nil {
		return fmt.Errorf("configure Codex shell environment: %w", err)
	}
	return nil
}
