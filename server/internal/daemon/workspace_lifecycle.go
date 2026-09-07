package daemon

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/daemon/execenv"
	"github.com/orvilo-ai/orvilo/server/internal/daemon/repocache"
)

func newWorkspaceState(workspaceID string, runtimeIDs []string, reposVersion string, repos []RepoData, settings json.RawMessage) *workspaceState {
	return &workspaceState{
		workspaceID:     workspaceID,
		runtimeIDs:      runtimeIDs,
		reposVersion:    reposVersion,
		allowedRepoURLs: repoAllowlist(repos),
		settings:        settings,
	}
}

func repoAllowlist(repos []RepoData) map[string]struct{} {
	allowed := make(map[string]struct{}, len(repos))
	for _, repo := range repos {
		if repo.URL == "" {
			continue
		}
		allowed[repo.URL] = struct{}{}
	}
	return allowed
}

func (d *Daemon) setWorkspaceRepoSyncError(workspaceID, syncErr string) {
	d.mu.Lock()
	defer d.mu.Unlock()
	if ws, ok := d.workspaces[workspaceID]; ok {
		ws.lastRepoSyncErr = syncErr
	}
}

func (d *Daemon) workspaceRepoAllowed(workspaceID, repoURL string) bool {
	d.mu.Lock()
	defer d.mu.Unlock()
	ws, ok := d.workspaces[workspaceID]
	if !ok {
		return false
	}
	if _, allowed := ws.allowedRepoURLs[repoURL]; allowed {
		return true
	}
	if _, allowed := ws.taskRepoURLs[repoURL]; allowed {
		return true
	}
	return false
}

// repoBarePathIsLive reports whether some watched workspace still claims the
// repo cached at barePath, so the GC can refuse to evict it.
//
// Answering per-path rather than materializing the whole set lets the GC ask
// again immediately before it deletes. A snapshot taken once per cycle goes
// stale while the caller runs git and filesystem work on each repo in turn,
// which is exactly the window in which a workspace can re-attach one.
//
// It mirrors workspaceRepoAllowed by unioning both sources: allowedRepoURLs
// (workspace-level bindings) and taskRepoURLs (project repos the server
// surfaced through a task claim, which never appear in GetWorkspaceRepos).
// Missing the second set would make the GC evict repos that tasks actively
// check out.
//
// Read from in-memory state on purpose. The alternative — asking the server
// for each workspace's repo list during GC — would make a transient API
// failure look like "nothing is attached", and this set is what protects
// caches from deletion.
func (d *Daemon) repoBarePathIsLive(barePath string) bool {
	if d.repoCache == nil || barePath == "" {
		return false
	}
	d.mu.Lock()
	defer d.mu.Unlock()
	for workspaceID, ws := range d.workspaces {
		for url := range ws.allowedRepoURLs {
			if d.repoCache.BarePath(workspaceID, url) == barePath {
				return true
			}
		}
		for url := range ws.taskRepoURLs {
			if d.repoCache.BarePath(workspaceID, url) == barePath {
				return true
			}
		}
	}
	return false
}

func (d *Daemon) workspaceLastRepoSyncErr(workspaceID string) string {
	d.mu.Lock()
	defer d.mu.Unlock()
	ws, ok := d.workspaces[workspaceID]
	if !ok {
		return ""
	}
	return ws.lastRepoSyncErr
}

// workspaceCoAuthoredByEnabled returns whether the Co-authored-by hook should
// be installed for the given workspace. Defaults to true when either setting
// is absent (new workspaces, older servers that don't send settings).
//
// The hook is gated by BOTH the GitHub master switch (`github_enabled`) and
// the dedicated co-author switch (`co_authored_by_enabled`) so flipping the
// workspace's master GitHub toggle off also stops new trailers from landing
// in commits, matching the contract documented in RFC MUL-2414 §4.8.
func (d *Daemon) workspaceCoAuthoredByEnabled(workspaceID string) bool {
	d.mu.Lock()
	defer d.mu.Unlock()
	ws, ok := d.workspaces[workspaceID]
	if !ok || len(ws.settings) == 0 {
		return true // default: enabled
	}
	var s struct {
		GitHubEnabled       *bool `json:"github_enabled"`
		CoAuthoredByEnabled *bool `json:"co_authored_by_enabled"`
	}
	if err := json.Unmarshal(ws.settings, &s); err != nil {
		return true // default: enabled when payload is malformed
	}
	if s.GitHubEnabled != nil && !*s.GitHubEnabled {
		return false
	}
	if s.CoAuthoredByEnabled == nil {
		return true // default: enabled
	}
	return *s.CoAuthoredByEnabled
}

// registerTaskRepos merges task-scoped repos (e.g. project github_repo
// resources lifted into resp.Repos by the claim handler) into the workspace's
// allowlist and kicks off a cache sync for any URLs that aren't yet cached.
//
// It's safe to call with the workspace's own repos — duplicates are
// idempotent. Called from runTask before the agent spawns so
// `patchbay repo checkout` accepts project-only URLs without an extra round
// trip back to GetWorkspaceRepos (which doesn't carry project resources).
func (d *Daemon) registerTaskRepos(workspaceID, taskID string, repos []RepoData) {
	if len(repos) == 0 {
		return
	}

	type repoCandidate struct {
		url     string
		tracked bool
	}

	d.mu.Lock()
	ws, ok := d.workspaces[workspaceID]
	if !ok {
		d.mu.Unlock()
		return
	}
	if ws.taskRepoURLs == nil {
		ws.taskRepoURLs = make(map[string]struct{}, len(repos))
	}
	if taskID != "" && ws.taskRepoRefs == nil {
		ws.taskRepoRefs = make(map[string]map[string]string)
	}
	candidates := make([]repoCandidate, 0, len(repos))
	for _, repo := range repos {
		url := strings.TrimSpace(repo.URL)
		if url == "" {
			continue
		}
		// Don't re-sync if the URL is already tracked (workspace or task-scoped)
		// AND the cache already has it.
		_, inWorkspace := ws.allowedRepoURLs[url]
		_, inTask := ws.taskRepoURLs[url]
		ws.taskRepoURLs[url] = struct{}{}
		if taskID != "" {
			if ws.taskRepoRefs[taskID] == nil {
				ws.taskRepoRefs[taskID] = make(map[string]string, len(repos))
			}
			if _, exists := ws.taskRepoRefs[taskID][url]; !exists {
				ws.taskRepoRefs[taskID][url] = strings.TrimSpace(repo.Ref)
			}
		}
		candidates = append(candidates, repoCandidate{
			url:     url,
			tracked: inWorkspace || inTask,
		})
	}
	d.mu.Unlock()

	toSync := make([]RepoData, 0, len(candidates))
	for _, candidate := range candidates {
		if candidate.tracked && d.repoCache != nil && d.repoCache.Lookup(workspaceID, candidate.url) != "" {
			continue
		}
		toSync = append(toSync, RepoData{URL: candidate.url})
	}

	if d.repoCache != nil && len(toSync) > 0 {
		// Sync in the background — same shape used at workspace registration.
		// `ensureRepoReady` reports a meaningful error if the cache isn't ready
		// yet, so the agent's first checkout will surface a sync failure
		// without silently treating it as a config bug.
		d.bgSyncs.Add(1)
		go func() {
			defer d.bgSyncs.Done()
			d.syncWorkspaceRepos(workspaceID, toSync)
		}()
	}
}

func (d *Daemon) taskRepoDefaultRef(workspaceID, taskID, repoURL string) string {
	taskID = strings.TrimSpace(taskID)
	repoURL = strings.TrimSpace(repoURL)
	if taskID == "" || repoURL == "" {
		return ""
	}
	d.mu.Lock()
	defer d.mu.Unlock()
	ws, ok := d.workspaces[workspaceID]
	if !ok || ws.taskRepoRefs == nil {
		return ""
	}
	return strings.TrimSpace(ws.taskRepoRefs[taskID][repoURL])
}

func (d *Daemon) clearTaskRepoRefs(workspaceID, taskID string) {
	taskID = strings.TrimSpace(taskID)
	if taskID == "" {
		return
	}
	d.mu.Lock()
	defer d.mu.Unlock()
	if ws, ok := d.workspaces[workspaceID]; ok && ws.taskRepoRefs != nil {
		delete(ws.taskRepoRefs, taskID)
	}
}

// waitBackgroundSyncs blocks until every background sync started by
// registerTaskRepos has finished. Intended for test teardown: tests that
// hand the daemon a t.TempDir-backed repo cache must call this before
// returning, otherwise an in-flight clone/fetch can race against TempDir
// cleanup and surface as an unrelated "directory not empty" failure.
func (d *Daemon) waitBackgroundSyncs() {
	d.bgSyncs.Wait()
}

func (d *Daemon) syncWorkspaceRepos(workspaceID string, repos []RepoData) {
	d.syncWorkspaceReposContext(context.Background(), workspaceID, repos)
}

func (d *Daemon) syncWorkspaceReposContext(ctx context.Context, workspaceID string, repos []RepoData) {
	if d.repoCache == nil {
		return
	}
	var err error
	if cache, ok := d.repoCache.(interface {
		SyncContext(context.Context, string, []repocache.RepoInfo) error
	}); ok {
		err = cache.SyncContext(ctx, workspaceID, repoDataToInfo(repos))
	} else {
		err = d.repoCache.Sync(workspaceID, repoDataToInfo(repos))
	}
	if err != nil {
		d.setWorkspaceRepoSyncError(workspaceID, err.Error())
		d.logger.Warn("repo cache sync failed", "workspace_id", workspaceID, "error", err)
		return
	}
	d.setWorkspaceRepoSyncError(workspaceID, "")
}

func (d *Daemon) refreshWorkspaceRepos(ctx context.Context, workspaceID string) (*WorkspaceReposResponse, error) {
	refreshCtx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()

	resp, err := d.client.GetWorkspaceRepos(refreshCtx, workspaceID)
	if err != nil {
		return nil, err
	}

	d.mu.Lock()
	if ws, ok := d.workspaces[workspaceID]; ok {
		ws.reposVersion = resp.ReposVersion
		ws.allowedRepoURLs = repoAllowlist(resp.Repos)
		// Keep the cached settings in sync with the server. The daemon's
		// feature gates (e.g. workspaceCoAuthoredByEnabled) read directly from
		// this field, so toggling a Setting in the web UI must update it here
		// without requiring a daemon restart. An empty payload from the server
		// clears the override and falls back to defaults.
		ws.settings = resp.Settings
	}
	d.mu.Unlock()

	// Publish the refreshed Co-authored-by verdict to the repo cache. Hooks
	// installed by earlier checkouts read that file at commit time, so this is
	// what makes a toggled-off setting apply to checkouts that already exist
	// instead of only to the next one (MUL-6921).
	d.persistCoAuthoredByState(workspaceID)

	return resp, nil
}

// coAuthoredByPublisher is the repo cache's half of the Co-authored-by wiring:
// the state file every installed hook reads at commit time, and the hooks
// themselves in the workspace's bare caches. Optional so test daemons can run
// with a minimal repoCacheBackend.
type coAuthoredByPublisher interface {
	WriteCoAuthoredByState(workspaceID string, enabled bool) error
	ReconcileCoAuthoredByHooks(workspaceID string, enabled bool) error
	ReconcileCoAuthoredByHookInCheckout(checkoutPath, workspaceID string, enabled bool) error
}

// persistCoAuthoredByState records the workspace's current Co-authored-by
// verdict where the installed prepare-commit-msg hooks can see it, and brings
// hooks written by earlier daemon releases — which read no state at all — in
// line with it. Called after every settings refresh and on every workspace
// sync; best-effort, since a failed write only leaves the previous value in
// place.
//
// The daemon is the only writer of this state, and publishes under a
// per-workspace lock with the verdict read INSIDE that lock. Two publishers
// racing (a settings refresh and a sync tick, say) therefore serialize, and the
// one that writes last is the one that read last — a publisher that started
// before a settings update can never overwrite the value that update produced.
func (d *Daemon) persistCoAuthoredByState(workspaceID string) {
	d.publishCoAuthoredByState(workspaceID, d.workspaceCoAuthoredByEnabled)
}

// publishCoAuthoredByState is persistCoAuthoredByState with the verdict read
// injected. Production always passes workspaceCoAuthoredByEnabled; tests pass a
// wrapper that parks a publisher between taking the lock and reading, which is
// the one ordering the lock exists to guarantee and cannot be observed
// otherwise.
func (d *Daemon) publishCoAuthoredByState(workspaceID string, verdict func(string) bool) {
	if d.repoCache == nil || workspaceID == "" {
		return
	}
	cache, ok := d.repoCache.(coAuthoredByPublisher)
	if !ok {
		return
	}

	d.mu.Lock()
	ws := d.workspaces[workspaceID]
	d.mu.Unlock()
	if ws == nil {
		return
	}

	ws.coAuthorPublishMu.Lock()
	defer ws.coAuthorPublishMu.Unlock()

	enabled := verdict(workspaceID)
	if err := cache.WriteCoAuthoredByState(workspaceID, enabled); err != nil {
		d.logger.Warn("record co-authored-by state failed", "workspace_id", workspaceID, "error", err)
	}
	if err := cache.ReconcileCoAuthoredByHooks(workspaceID, enabled); err != nil {
		d.logger.Warn("reconcile co-authored-by hooks failed", "workspace_id", workspaceID, "error", err)
	}
	d.reconcileIsolatedCoAuthoredByHooks(cache, workspaceID, enabled)
}

// isolatedCheckoutScanDepth bounds how far below an env root the sweep looks
// for a checkout. Repos land at <env root>/<workdir>/<repo>, so two levels
// covers the layout with one to spare and keeps the walk from wandering into
// the checked-out source tree.
const isolatedCheckoutScanDepth = 2

// reconcileIsolatedCoAuthoredByHooks applies the workspace setting to hooks
// that live inside task workdirs rather than in the shared bare cache.
//
// Codex on Linux and the Windows sandbox check out with isolated git metadata,
// so their hook sits in <checkout>/.git/hooks and the repo cache cannot see it.
// Left alone, a workdir prepared by an earlier release keeps its unconditional
// hook — and its next commit keeps adding the trailer — until that repo happens
// to be checked out again. Env roots are attributed by their owner record, so a
// workspace never rewrites hooks belonging to another one.
func (d *Daemon) reconcileIsolatedCoAuthoredByHooks(cache coAuthoredByPublisher, workspaceID string, enabled bool) {
	root := d.cfg.WorkspacesRoot
	if root == "" || workspaceID == "" {
		return
	}
	wsEntries, err := os.ReadDir(root)
	if err != nil {
		if !os.IsNotExist(err) {
			d.logger.Debug("co-authored-by sweep: read workspaces root failed", "error", err)
		}
		return
	}

	for _, wsEntry := range wsEntries {
		// Dot directories are daemon-internal caches (.repos, .skill-cache),
		// never workspace directories — the same rule runGC walks by.
		if !wsEntry.IsDir() || strings.HasPrefix(wsEntry.Name(), ".") {
			continue
		}
		wsDir := filepath.Join(root, wsEntry.Name())
		envRoots, err := os.ReadDir(wsDir)
		if err != nil {
			continue
		}
		for _, envEntry := range envRoots {
			if !envEntry.IsDir() || strings.HasPrefix(envEntry.Name(), ".") {
				continue
			}
			envRoot := filepath.Join(wsDir, envEntry.Name())
			if !d.envRootBelongsToWorkspace(wsEntry.Name(), envRoot, workspaceID) {
				continue
			}
			for _, checkout := range isolatedCheckoutCandidates(envRoot, isolatedCheckoutScanDepth) {
				if err := cache.ReconcileCoAuthoredByHookInCheckout(checkout, workspaceID, enabled); err != nil {
					d.logger.Warn("reconcile co-authored-by hook in checkout failed",
						"workspace_id", workspaceID, "path", checkout, "error", err)
				}
			}
		}
	}
}

// envRootBelongsToWorkspace reports whether an env root is this workspace's, by
// the strongest evidence the release that created it left behind.
//
// Since v0.4.35 the owner record carries the workspace ID, and an exact match
// is required. Env roots prepared before that recorded only a task ID — or, on
// older releases still, no marker at all — and they live under the layout that
// release used: <workspaces root>/<workspace ID>/<task key>. So for an owner
// record that names no workspace, the directory name is the evidence, and it
// has to BE the workspace ID. The readable layout that replaced it always
// appends a suffix (`<slug>-<id tail>`), so a bare workspace UUID can only be
// one of those older roots and this can never alias a modern one.
//
// An unreadable marker attributes nothing: skipping costs a stale hook in one
// workdir, guessing would rewrite hooks in another workspace's.
func (d *Daemon) envRootBelongsToWorkspace(wsDirName, envRoot, workspaceID string) bool {
	owner, err := execenv.ReadEnvRootOwner(envRoot)
	if err != nil || owner == nil {
		return false
	}
	if owner.WorkspaceID != "" {
		return owner.WorkspaceID == workspaceID
	}
	return wsDirName == workspaceID
}

// isolatedCheckoutCandidates returns directories at or below dir that hold
// their own git metadata. A linked worktree's .git is a file, so only isolated
// checkouts match, and matching stops descending: nothing nested inside a
// checkout is one.
func isolatedCheckoutCandidates(dir string, depth int) []string {
	if depth <= 0 {
		return nil
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	var found []string
	for _, entry := range entries {
		if !entry.IsDir() || strings.HasPrefix(entry.Name(), ".") {
			continue
		}
		child := filepath.Join(dir, entry.Name())
		if info, err := os.Stat(filepath.Join(child, ".git")); err == nil && info.IsDir() {
			found = append(found, child)
			continue
		}
		found = append(found, isolatedCheckoutCandidates(child, depth-1)...)
	}
	return found
}

// publishTrackedCoAuthoredByState writes the current verdict for every tracked
// workspace and reconciles the hooks in their bare caches. Purely local — no
// request — and both halves skip the write when what is on disk already
// matches, so a sync tick on a converged host costs a handful of small reads.
func (d *Daemon) publishTrackedCoAuthoredByState() {
	d.mu.Lock()
	ids := make([]string, 0, len(d.workspaces))
	for id := range d.workspaces {
		ids = append(ids, id)
	}
	d.mu.Unlock()

	for _, id := range ids {
		d.persistCoAuthoredByState(id)
	}
}

// trackedSettingsRefreshTimeout bounds one refreshTrackedWorkspaceSettings
// pass. A workspace it does not reach keeps its cached settings and picks the
// change up on its next repo checkout.
const trackedSettingsRefreshTimeout = 30 * time.Second

// refreshTrackedWorkspaceSettings re-reads repos and settings for workspaces
// this daemon already tracks. Only the server's workspaces-changed hint calls
// it: the periodic sync deliberately makes no repos request for a tracked
// workspace (see syncWorkspacesFromAPI), so without this a settings edit —
// the GitHub master switch, the Co-authored-by toggle — would sit unseen
// until the workspace's next repo checkout.
func (d *Daemon) refreshTrackedWorkspaceSettings(ctx context.Context) {
	// The sync loop is single-threaded and still owes the hint its membership
	// reconcile, so bound the whole pass rather than letting one slow
	// workspace hold every other signal behind it.
	ctx, cancel := context.WithTimeout(ctx, trackedSettingsRefreshTimeout)
	defer cancel()

	d.mu.Lock()
	tracked := make(map[string]*workspaceState, len(d.workspaces))
	for id, ws := range d.workspaces {
		tracked[id] = ws
	}
	d.mu.Unlock()

	for id, ws := range tracked {
		if ctx.Err() != nil {
			return
		}
		// Share ensureRepoReady's lock so a checkout in flight and this
		// refresh cannot interleave their writes to the same workspace.
		if err := ws.repoRefreshMu.Lock(ctx); err != nil {
			return
		}
		_, err := d.refreshWorkspaceRepos(ctx, id)
		ws.repoRefreshMu.Unlock()
		if err != nil {
			d.logger.Debug("workspace settings refresh failed", "workspace_id", id, "error", err)
		}
	}
}

func (d *Daemon) ensureRepoReady(ctx context.Context, workspaceID, repoURL string) error {
	if d.repoCache == nil {
		return fmt.Errorf("repo cache not initialized")
	}

	repoURL = strings.TrimSpace(repoURL)

	d.mu.Lock()
	ws, ok := d.workspaces[workspaceID]
	d.mu.Unlock()
	if !ok {
		return fmt.Errorf("workspace is not watched by this daemon: %s", workspaceID)
	}

	// Record whether the cache already had this repo before we took the
	// per-workspace mutex. The two states behave differently below:
	//
	//   - cacheHitOnEntry=true: the repo is already cloned; we still must
	//     refresh `workspaceState.settings` because the /repo/checkout
	//     handler reads workspaceCoAuthoredByEnabled right after this. The
	//     periodic workspace sync deliberately does not refresh repos or
	//     settings, so this is the point at which a freshly-flipped GitHub
	//     master switch / `co_authored_by_enabled` toggle becomes live.
	//
	//   - cacheHitOnEntry=false but cache hit *after* we acquire the mutex:
	//     a sibling goroutine on a concurrent cold-miss already refreshed
	//     and populated the cache. We can skip the duplicate refresh — the
	//     sibling's refresh is fresh enough for our gate read.
	cacheHitOnEntry := d.workspaceRepoAllowed(workspaceID, repoURL) && d.repoCache.Lookup(workspaceID, repoURL) != ""

	if err := ws.repoRefreshMu.Lock(ctx); err != nil {
		return err
	}
	defer ws.repoRefreshMu.Unlock()

	if !cacheHitOnEntry && d.workspaceRepoAllowed(workspaceID, repoURL) && d.repoCache.Lookup(workspaceID, repoURL) != "" {
		return nil
	}

	resp, err := d.refreshWorkspaceRepos(ctx, workspaceID)
	if err != nil {
		return fmt.Errorf("refresh workspace repos: %w", err)
	}

	if !d.workspaceRepoAllowed(workspaceID, repoURL) {
		return ErrRepoNotConfigured
	}

	if d.repoCache.Lookup(workspaceID, repoURL) != "" {
		return nil
	}

	d.syncWorkspaceReposContext(ctx, workspaceID, resp.Repos)
	if err := ctx.Err(); err != nil {
		return context.Cause(ctx)
	}

	if d.repoCache.Lookup(workspaceID, repoURL) != "" {
		return nil
	}

	if syncErr := d.workspaceLastRepoSyncErr(workspaceID); syncErr != "" {
		return fmt.Errorf("repo is configured but not synced: %s", syncErr)
	}

	return fmt.Errorf("repo is configured but not synced")
}

// workspaceSyncLoop reconciles the user's workspace membership set. Daemons
// with runtimes and account-scoped WS support use a thirty-minute jittered
// consistency check; daemons talking to older servers retain a five-minute
// fallback. Bootstrap daemons without runtimes keep the shorter interval needed
// to discover their first workspace. Account-scoped WS hints trigger an
// immediate minimal sync, while a WS reconnect also reconciles runtime profiles
// changed during the connection gap.
func (d *Daemon) workspaceSyncLoop(ctx context.Context) {
	timer := time.NewTimer(jitterDuration(d.workspaceSyncBaseInterval()))
	defer timer.Stop()

	var reconcileCh <-chan struct{}
	if d.reconcile != nil {
		reconcileCh = d.reconcile.notify()
	}
	var workspaceChangesCh <-chan struct{}
	if d.workspaceChanges != nil {
		workspaceChangesCh = d.workspaceChanges.notify()
	}

	var consecutiveFailures int
	resetTimer := func() {
		interval := workspaceSyncBackoff(d.workspaceSyncBaseInterval(), consecutiveFailures)
		if !timer.Stop() {
			select {
			case <-timer.C:
			default:
			}
		}
		timer.Reset(jitterDuration(interval))
	}

	syncNow := func(reconcileProfiles bool) {
		if err := d.syncWorkspacesFromAPI(ctx, reconcileProfiles); err != nil {
			consecutiveFailures++
			d.logger.Debug("workspace sync failed", "error", err)
		} else {
			consecutiveFailures = 0
		}
		resetTimer()
	}

	for {
		select {
		case <-ctx.Done():
			return
		case <-reconcileCh:
			if d.reconcile != nil {
				reconcileCh = d.reconcile.notify()
			}
			// A settings edit made while the websocket was down produced a
			// workspaces-changed hint nobody received. Reconnecting is the
			// daemon's chance to catch up on it: without this the stale
			// verdict would just get republished by the sync below.
			d.refreshTrackedWorkspaceSettings(ctx)
			syncNow(true)
		case <-workspaceChangesCh:
			// The hint fires for membership changes AND for workspace edits,
			// including settings. Refresh cached settings for the workspaces
			// we already track before reconciling the membership set.
			d.refreshTrackedWorkspaceSettings(ctx)
			syncNow(false)
		case <-timer.C:
			syncNow(false)
		}
	}
}

func (d *Daemon) workspaceSyncBaseInterval() time.Duration {
	if len(d.allRuntimeIDs()) == 0 {
		return DefaultWorkspaceBootstrapSyncInterval
	}
	if d.client.usesLegacyWorkspaceEndpoint() {
		return DefaultWorkspaceLegacySyncInterval
	}
	return DefaultWorkspaceSyncInterval
}

func workspaceSyncBackoff(base time.Duration, consecutiveFailures int) time.Duration {
	maxInterval := DefaultWorkspaceSyncMaxBackoff
	if base == DefaultWorkspaceBootstrapSyncInterval {
		maxInterval = DefaultWorkspaceLegacySyncInterval
	}
	interval := base
	for i := 0; i < consecutiveFailures; i++ {
		if interval >= maxInterval/2 {
			return maxInterval
		}
		interval *= 2
	}
	if interval > maxInterval {
		return maxInterval
	}
	return interval
}

// syncWorkspacesFromAPI fetches all workspaces the user belongs to and
// registers runtimes for any that aren't already tracked. When
// reconcileProfiles is true (after a daemon WS connect/reconnect), tracked
// workspaces reconcile custom runtime profiles once so a change made while the
// WS was unavailable is not lost. Normal timers and workspace-change hints
// pass false and make no runtime-profile requests. Workspaces the user has
// left are cleaned up.
func (d *Daemon) syncWorkspacesFromAPI(ctx context.Context, reconcileProfiles bool) error {
	d.reloading.Lock()
	defer d.reloading.Unlock()

	apiCtx, cancel := context.WithTimeout(ctx, 15*time.Second)
	defer cancel()

	workspaces, err := d.client.ListWorkspaces(apiCtx)
	if err != nil {
		return fmt.Errorf("list workspaces: %w", err)
	}
	d.logger.Debug("workspace sync: fetched workspaces", "count", len(workspaces))

	apiIDs := make(map[string]string, len(workspaces)) // id -> name
	for _, ws := range workspaces {
		apiIDs[ws.ID] = ws.Name
	}

	d.mu.Lock()
	currentIDs := make(map[string]bool, len(d.workspaces))
	for id := range d.workspaces {
		currentIDs[id] = true
	}
	d.mu.Unlock()

	// A previous registration may have reached the server but failed before
	// orphan recovery completed. Retry that recovery before reconciling more
	// workspace state; the failure barrier keeps the poller from claiming while
	// this pass is outstanding.
	if d.terminalRecoveryFailed.Load() {
		if err := d.recoverTrackedOrphans(ctx); err != nil {
			return err
		}
	}

	// Built-in agent CLIs are installed per machine, so one probe round serves
	// every workspace this sync has to register (MUL-5225). Probing is lazy —
	// a sync that finds nothing new to register never shells out at all, which
	// is the common case for the periodic sync — and scoped to this call, so
	// the next sync re-detects and an in-place CLI upgrade is still reported.
	var (
		builtins       []map[string]string
		builtinsProbed bool
	)
	probeBuiltins := func() []map[string]string {
		if !builtinsProbed {
			builtins, _, _ = d.detectBuiltinRuntimes(ctx)
			builtinsProbed = true
		}
		return builtins
	}

	var registered int
	var removed int
	for id, name := range apiIDs {
		if currentIDs[id] {
			if reconcileProfiles {
				if err := d.refreshWorkspaceRuntimeProfiles(ctx, id); err != nil {
					d.logger.Debug("workspace reconcile: profile refresh failed", "workspace_id", id, "error", err)
				}
			}
			// Only intervene further if the workspace lost all of its
			// runtimes (most commonly because handleRuntimeGone pruned them
			// and its inline re-register failed). The pointer is not replaced
			// here either — ensureRepoReady holds repoRefreshMu from the
			// original pointer.
			if !d.workspaceNeedsRuntimeRecovery(id) {
				continue
			}
			d.logger.Info("workspace has no runtimes; retrying registration", "workspace_id", id, "name", name)
			if err := d.reregisterWorkspaceAfterRuntimeGone(ctx, id); err != nil {
				d.logger.Warn("retry register failed", "workspace_id", id, "error", err)
				continue
			}
			registered++
			continue
		}
		payload := probeBuiltins()
		var (
			resp       *RegisterResponse
			runtimeIDs []string
		)
		// Send, publish and clean up as one ordered step — see
		// workspaceRegisterLock.
		err := d.withWorkspaceRegisterLock(id, func() error {
			var profileSig string
			var err error
			resp, profileSig, err = d.registerRuntimesForWorkspaceBatchLocked(ctx, id, payload)
			if err != nil {
				return err
			}
			// First registration is the third path a response reaches local state
			// through — it builds the workspaceState directly instead of going via
			// applyRegisterResponseInPlace / mergeBuiltinRegisterResponse — so it
			// needs the same demotion guard, taken in the same d.mu section that
			// publishes the state. A workspace joining while a provider is held
			// below-minimum must not be the way that provider comes back.
			d.mu.Lock()
			runtimeIDs = make([]string, 0, len(resp.Runtimes))
			var revived revivedRuntimes
			for _, rt := range resp.Runtimes {
				if rt.ProfileID == "" && d.providerDemotedLocked(rt.Provider) {
					revived.add(d, rt.ID, rt.Provider)
					continue
				}
				runtimeIDs = append(runtimeIDs, rt.ID)
				d.logger.Info("registered runtime", "workspace_id", id, "runtime_id", rt.ID, "provider", rt.Provider)
			}
			ws := newWorkspaceState(id, runtimeIDs, resp.ReposVersion, resp.Repos, resp.Settings)
			// Seed the profile signature so later on-demand change notifications can
			// detect drift without re-registering on duplicates (empty sig is the
			// explicit "unknown — keep the previous value" sentinel from
			// appendProfileRuntimes; on first registration there is no previous
			// value, so empty stays empty).
			ws.profileSetSig = profileSig
			// Seed the per-workspace record of what the server was told (the
			// register call above ran before this workspaceState existed, so
			// recordBuiltinVersionsSent inside it had nowhere to write).
			ws.builtinVersions = builtinVersionsFromPayload(payload)
			for provider := range ws.builtinVersions {
				// Same reason recordBuiltinVersionsSent skips these: a version
				// record for a provider whose rows are being refused reads as
				// "this workspace is current" to the next refresh round.
				if d.providerDemotedLocked(provider) {
					delete(ws.builtinVersions, provider)
				}
			}
			d.workspaces[id] = ws
			for _, rt := range resp.Runtimes {
				if rt.ProfileID == "" && d.providerDemotedLocked(rt.Provider) {
					continue
				}
				d.runtimeIndex[rt.ID] = rt
			}
			d.mu.Unlock()

			// The server upserted the refused rows into existence, so it alone
			// would believe they are online and keep routing work to them. Take
			// them offline before anything else touches this workspace — and never
			// RecoverOrphans them below: that reports tasks for a runtime the
			// daemon does not track and will never claim for.
			d.deregisterRevivedRuntimes(ctx, id, revived)
			return nil
		})
		if err != nil {
			d.logger.Error("failed to register runtimes", "workspace_id", id, "name", name, "error", err)
			continue
		}

		if d.repoCache != nil && len(resp.Repos) > 0 {
			go d.syncWorkspaceRepos(id, resp.Repos)
		}

		// Tell the server about any tasks the previous daemon process was
		// running on these runtimes. Without this, an issue can stay stuck
		// at in_progress until the slow heartbeat sweeper or the in-flight
		// task timeout (2.5h) kicks in.
		for _, rid := range runtimeIDs {
			if err := d.recoverOrphans(ctx, rid); err != nil {
				return fmt.Errorf("recover-orphans failed for runtime %s: %w", rid, err)
			}
		}

		d.logger.Info("watching workspace", "workspace_id", id, "name", name, "runtimes", len(runtimeIDs), "repos", len(resp.Repos))
		registered++
	}

	// Remove workspaces the user no longer belongs to.
	for id := range currentIDs {
		if _, ok := apiIDs[id]; !ok {
			d.mu.Lock()
			if ws, exists := d.workspaces[id]; exists {
				for _, rid := range ws.runtimeIDs {
					delete(d.runtimeIndex, rid)
				}
			}
			delete(d.workspaces, id)
			d.mu.Unlock()
			d.logger.Info("stopped watching workspace", "workspace_id", id)
			removed++
		}
	}
	if registered > 0 || removed > 0 {
		d.notifyRuntimeSetChanged()
	}

	// Republish each tracked workspace's Co-authored-by verdict. This costs no
	// request — it writes the daemon's cached verdict where prepare-commit-msg
	// hooks read it, and migrates hooks left by earlier releases — and is what
	// covers the cases no refresh reaches: a toggle flipped while the daemon
	// was down (settings arrive with the register response above), a host that
	// just upgraded, and a state file removed by repo cache GC.
	d.publishTrackedCoAuthoredByState()

	if len(d.allRuntimeIDs()) == 0 && registered == 0 && len(workspaces) > 0 {
		return fmt.Errorf("failed to register runtimes for any of the %d workspace(s)", len(workspaces))
	}
	if registered > 0 || removed > 0 {
		d.logger.Debug("workspace sync done", "registered", registered, "removed", removed, "tracked", len(apiIDs))
	}
	return nil
}

// markActiveEnvRoot records that a task is currently using the given env root,
// so the GC loop won't reclaim its artifacts mid-execution. Calls are
// reference-counted so a reuse path marked twice (predicted + prior) only
// becomes inactive after both unmark calls.
func (d *Daemon) markActiveEnvRoot(envRoot string) {
	if envRoot == "" {
		return
	}
	d.activeEnvRootsMu.Lock()
	defer d.activeEnvRootsMu.Unlock()
	d.ensureActiveEnvRootStateLocked()
	for d.deletingEnvRoots[envRoot] {
		d.activeEnvRootsCond.Wait()
	}
	d.activeEnvRoots[envRoot]++
}

func (d *Daemon) unmarkActiveEnvRoot(envRoot string) {
	if envRoot == "" {
		return
	}
	d.activeEnvRootsMu.Lock()
	defer d.activeEnvRootsMu.Unlock()
	d.ensureActiveEnvRootStateLocked()
	if d.activeEnvRoots[envRoot] <= 1 {
		delete(d.activeEnvRoots, envRoot)
		return
	}
	d.activeEnvRoots[envRoot]--
}

func (d *Daemon) isActiveEnvRoot(envRoot string) bool {
	d.activeEnvRootsMu.Lock()
	defer d.activeEnvRootsMu.Unlock()
	d.ensureActiveEnvRootStateLocked()
	return d.activeEnvRoots[envRoot] > 0
}

func (d *Daemon) ensureActiveEnvRootStateLocked() {
	if d.activeEnvRoots == nil {
		d.activeEnvRoots = make(map[string]int)
	}
	if d.deletingEnvRoots == nil {
		d.deletingEnvRoots = make(map[string]bool)
	}
	if d.activeEnvRootsCond == nil {
		d.activeEnvRootsCond = sync.NewCond(&d.activeEnvRootsMu)
	}
}

// reserveEnvRootForGC atomically confirms that no live task is using envRoot
// and prevents a new task from entering until release runs. This closes the
// check-then-remove race between the GC loop and task startup: either GC sees
// the active task and skips, or task startup waits for the mutation to finish
// and recreates/uses the post-GC environment.
func (d *Daemon) reserveEnvRootForGC(envRoot string) (release func(), ok bool) {
	if envRoot == "" {
		return nil, false
	}
	d.activeEnvRootsMu.Lock()
	defer d.activeEnvRootsMu.Unlock()
	d.ensureActiveEnvRootStateLocked()
	if d.activeEnvRoots[envRoot] > 0 || d.deletingEnvRoots[envRoot] {
		return nil, false
	}
	d.deletingEnvRoots[envRoot] = true
	return func() {
		d.activeEnvRootsMu.Lock()
		delete(d.deletingEnvRoots, envRoot)
		d.activeEnvRootsCond.Broadcast()
		d.activeEnvRootsMu.Unlock()
	}, true
}

// markActiveStore records that a task is about to use the given persistent
// store — a per-issue Codex session store or a per-agent Hermes memory store —
// so the GC never reclaims it mid-task. These stores live outside the env root,
// so isActiveEnvRoot does not cover them (MUL-4424). If a GC
// delete has already reserved this store, we wait for that removal to finish
// before claiming it, so a task never mounts a store mid-removal; the store is
// then recreated fresh by Prepare. Reference-counted like the env-root guard.
func (d *Daemon) markActiveStore(store string) {
	if store == "" {
		return
	}
	d.activeStoresMu.Lock()
	defer d.activeStoresMu.Unlock()
	for d.deletingStores[store] {
		d.activeStoresCond.Wait()
	}
	d.activeStores[store]++
}

func (d *Daemon) unmarkActiveStore(store string) {
	if store == "" {
		return
	}
	d.activeStoresMu.Lock()
	defer d.activeStoresMu.Unlock()
	if d.activeStores[store] <= 1 {
		delete(d.activeStores, store)
		return
	}
	d.activeStores[store]--
}

// reserveStoreForDeletion atomically checks that no live task holds store
// and, if so, marks it reserved so no task can claim it until the caller runs
// the returned commit (after the actual removal). ok=false means a task holds it
// — do not delete. This is the exclusive protocol the store pruners
// (PruneCodexSessionStores, PruneHermesMemoryStores) need:
// the "confirm inactive" and the mark happen under one lock acquisition, so a
// markActiveStore either loses the check (store stays) or blocks on the
// reservation, closing the stat->remove race (MUL-4424).
func (d *Daemon) reserveStoreForDeletion(store string) (commit func(), ok bool) {
	d.activeStoresMu.Lock()
	defer d.activeStoresMu.Unlock()
	if d.activeStores[store] > 0 || d.deletingStores[store] {
		return nil, false
	}
	d.deletingStores[store] = true
	return func() {
		d.activeStoresMu.Lock()
		delete(d.deletingStores, store)
		d.activeStoresCond.Broadcast()
		d.activeStoresMu.Unlock()
	}, true
}
