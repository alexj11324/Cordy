package daemon

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"sync/atomic"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/cli"
	"github.com/orvilo-ai/orvilo/server/internal/daemon/execenv"
	"github.com/orvilo-ai/orvilo/server/internal/daemon/repocache"
	"github.com/orvilo-ai/orvilo/server/internal/daemon/terminalreport"
	"github.com/orvilo-ai/orvilo/server/internal/selfexec"
	"github.com/orvilo-ai/orvilo/server/pkg/agent"
	"golang.org/x/sync/singleflight"
)

// ErrRepoNotConfigured is returned by ensureRepoReady when the requested repo
// URL is not present in the workspace's repo configuration after a fresh
// server refresh.
var ErrRepoNotConfigured = errors.New("repo is not configured for this workspace")

// ErrNoRuntimesToRegister is returned by registerRuntimesForWorkspace when
// the daemon has nothing to host on a workspace — typically a custom-only
// daemon whose only enabled custom runtime profile was just disabled, leaving
// zero built-in agents and zero resolvable profiles. Callers must
// differentiate by intent: initial registration (syncWorkspacesFromAPI's
// new-workspace branch) treats this as a config error and skips the
// workspace until something changes; the profile-drift refresh path
// (refreshWorkspaceRuntimeProfiles) treats it as a legitimate converged
// state and explicitly deregisters the now-stale local runtime IDs so the
// server marks them offline immediately instead of waiting on the 150 s
// stale-heartbeat sweep.
var ErrNoRuntimesToRegister = errors.New("no agent runtimes could be registered")

// errTaskPrepareTimeout distinguishes the daemon's dispatched -> running
// startup deadline from provider execution timeouts. handleTask maps it to the
// platform-side timeout failure reason so the server's existing retry path can
// recover the task on a fresh attempt.
var errTaskPrepareTimeout = errors.New("task preparation timed out")

// errSkillBundleUnavailable marks a task that died in preparation because the
// daemon could not download one of the agent's skill bundles. Carrying it as a
// sentinel — rather than leaving handleTask to pattern-match the wrapped
// transport error — is what lets the failure land on the platform-side
// skill_bundle_unavailable reason instead of agent_error.unknown, which is not
// on the server's retry allowlist. (MUL-5370)
var errSkillBundleUnavailable = errors.New("skill bundle unavailable")

const (
	taskSlotWaitTimeout      = 2 * time.Second
	taskSlotCapacityBackoff  = 5 * time.Second
	repoCheckoutModeEnv      = "ORVILO_REPO_CHECKOUT_MODE"
	repoCheckoutModeIsolated = "isolated"
	// defaultTaskPrepareTimeout is a hard liveness bound for everything after
	// claim and before StartTask succeeds: runtime resolution, skill bundles,
	// execution-environment setup, and the StartTask request itself. It is
	// intentionally independent from AgentTimeout, which only governs the
	// provider process after the task reaches running.
	defaultTaskPrepareTimeout = 5 * time.Minute
	// pendingWorkHeartbeatTimeout bounds the out-of-band heartbeat a
	// server-pushed daemon:pending_work hint triggers (MUL-5444). Short on
	// purpose: the hint is only a latency optimisation, and the scheduled
	// heartbeat still picks the request up if this attempt fails.
	pendingWorkHeartbeatTimeout = 15 * time.Second
	// pendingWorkHintBookkeepingTTL is how long a runtime's last-hint timestamp
	// is retained before it is swept — purely to keep the map bounded.
	pendingWorkHintBookkeepingTTL = 10 * time.Minute
	// idleWatchdogMaxTick caps the idle watchdog's polling interval. At the
	// base rate of window/2 the overshoot scales with the budget: a 2h window
	// would only be checked hourly, so a genuinely stuck run could hold its
	// slot for 3h. The cap makes worst-case detection window + 5m no matter how
	// large an operator sets the budget.
	//
	// 5m is chosen as the largest overshoot worth tolerating on top of a budget
	// already measured in hours, not for its polling cost — a tick is an atomic
	// load and a channel length check, so it is free at any interval anyone
	// would pick.
	idleWatchdogMaxTick = 5 * time.Minute
)

// pendingWorkHintMinInterval is the floor between two hint-driven heartbeats
// for the same runtime. Keeps an interactive first open instant while stopping a
// caller-triggered hint from becoming a heartbeat amplifier. A var so tests can
// shrink it, same as the other timing knobs in this package.
var pendingWorkHintMinInterval = time.Second

var (
	taskPrepareLeaseRefresh = 15 * time.Second
	taskPrepareLeaseTimeout = 10 * time.Second
	errInvalidTaskIdentity  = errors.New("invalid task identity")
)

// taskRunner executes a single agent task and returns the result.
// Extracted as an interface so tests can inject a fake without spawning real
// agent processes, while keeping test scaffolding out of the production struct.
type taskRunner interface {
	run(ctx context.Context, task Task, provider string, slot int, log *slog.Logger) (TaskResult, error)
}

// taskRunnerFunc adapts a plain function to the taskRunner interface.
type taskRunnerFunc func(context.Context, Task, string, int, *slog.Logger) (TaskResult, error)

func (f taskRunnerFunc) run(ctx context.Context, task Task, provider string, slot int, log *slog.Logger) (TaskResult, error) {
	return f(ctx, task, provider, slot, log)
}

type executionEnvironmentCommand func() ([]string, error)

func defaultExecutionEnvironmentCommand() ([]string, error) {
	executable, err := resolveSelfExecutable()
	if err != nil {
		return nil, fmt.Errorf("resolve execution-environment helper: %w", err)
	}
	return []string{executable, execenv.PreparationHelperArg}, nil
}

var (
	isBrewInstall         = cli.IsBrewInstall
	getBrewPrefix         = cli.GetBrewPrefix
	matchKnownBrewPrefix  = cli.MatchKnownBrewPrefix
	resolveSelfExecutable = selfexec.Resolve

	// detectAgentVersion / checkAgentMinVersion are indirections over the
	// real agent helpers so tests can run the registration path without
	// shelling out to a real CLI. Mirrors the pattern used for the brew
	// helpers above.
	detectAgentVersion   = agent.DetectVersion
	checkAgentMinVersion = agent.CheckMinVersion

	// listModels is an indirection over agent.ListModels so model-discovery
	// tests can assert which executable path the daemon enumerates without
	// shelling out to a real CLI. Mirrors the detectAgentVersion hook above.
	listModels = agent.ListModels

	// lookPath is an indirection over exec.LookPath so registration tests can
	// resolve custom runtime-profile commands without manipulating the
	// process PATH. Mirrors the detectAgentVersion hook above.
	lookPath = exec.LookPath

	// profilePathExecutable reports whether path points at an existing,
	// non-directory file with at least one executable bit set. It is the
	// gate appendProfileRuntimes uses before trusting a per-machine command
	// path override (MUL-3284) — a stale or mistyped override must fall back
	// to the PATH lookup rather than register a runtime that can't launch.
	// Indirected as a package var so tests can assert override preference
	// without staging a real executable on disk.
	profilePathExecutable = func(path string) bool {
		info, err := os.Stat(path)
		if err != nil || info.IsDir() {
			return false
		}
		return info.Mode().Perm()&0o111 != 0
	}
)

// workspaceState tracks registered runtimes for a single workspace.
//
// allowedRepoURLs covers the workspace-level repo bindings; it gets rebuilt on
// every refresh from the server. taskRepoURLs covers repos that the server
// surfaced through a per-task claim (project github_repo resources today,
// possibly other typed sources later) — those don't show up in
// GetWorkspaceRepos, so they would be wiped on refresh if we shared one map.
// taskRepoRefs tracks optional checkout refs for the specific task that
// surfaced each project repo so two projects using the same URL don't leak refs
// into each other.
type workspaceState struct {
	workspaceID     string
	runtimeIDs      []string
	reposVersion    string // stored for future use: skip refresh when version unchanged
	allowedRepoURLs map[string]struct{}
	taskRepoURLs    map[string]struct{}
	taskRepoRefs    map[string]map[string]string // taskID -> repo URL -> checkout ref
	settings        json.RawMessage              // workspace settings (JSONB)
	lastRepoSyncErr string
	repoRefreshMu   contextLock
	// coAuthorPublishMu serializes publication of the Co-authored-by verdict
	// for this workspace. Unlike the fields above it is NOT guarded by
	// Daemon.mu: it exists precisely so the verdict can be read and written as
	// one step without holding the daemon's central lock across file I/O.
	coAuthorPublishMu sync.Mutex
	// profileSetSig is a content hash of the workspace's custom runtime
	// profile list (MUL-3332) as last seen from the server. An on-demand
	// refresh compares the live signature with this cached value; any drift
	// triggers a re-register so newly-added (or edited / disabled) custom
	// runtimes appear without a daemon restart. Empty before the first
	// successful profile fetch (older server / network blip); guarded by
	// Daemon.mu like every other field on this struct.
	profileSetSig string
	// builtinVersions records, per built-in provider, the version carried by
	// the last register call the server ACCEPTED for this workspace. This is
	// the daemon's per-workspace record of what the server knows — which the
	// shared agentVersions cache deliberately is not: every probing path
	// writes that cache, but each register call covers only the workspaces it
	// was invoked for. refreshAgentVersions compares this record against the
	// current probe round and re-registers exactly the workspaces that are
	// behind. Scoping the acknowledgement to the workspace is what makes a
	// concurrent older-generation registration safe: a register that probed
	// before an upgrade and lands after everyone else was refreshed simply
	// re-creates the mismatch for its own workspace, and the next round
	// revisits it. A failed register records nothing, so the workspace stays
	// behind and is retried. Guarded by Daemon.mu.
	builtinVersions map[string]string
}

// contextLock is a zero-value-ready mutex whose wait can be cancelled. Repo
// checkout requests use it for workspace refresh coalescing so disconnecting a
// client never leaves the handler stuck behind another cold-cache refresh.
type contextLock struct {
	once  sync.Once
	token chan struct{}
}

func (l *contextLock) Lock(ctx context.Context) error {
	if err := ctx.Err(); err != nil {
		return context.Cause(ctx)
	}
	l.once.Do(func() {
		l.token = make(chan struct{}, 1)
		l.token <- struct{}{}
	})
	select {
	case <-ctx.Done():
		return context.Cause(ctx)
	case <-l.token:
		if err := ctx.Err(); err != nil {
			l.token <- struct{}{}
			return context.Cause(ctx)
		}
		return nil
	}
}

func (l *contextLock) Unlock() {
	l.token <- struct{}{}
}

type repoCacheBackend interface {
	Lookup(workspaceID, url string) string
	BarePath(workspaceID, url string) string
	Sync(workspaceID string, repos []repocache.RepoInfo) error
	WithRepoLock(barePath string, fn func() error) error
	CreateWorktree(params repocache.WorktreeParams) (*repocache.WorktreeResult, error)
}

// Daemon is the local agent runtime that polls for and executes tasks.
type Daemon struct {
	cfg        Config
	client     *Client
	repoCache  repoCacheBackend
	skillCache *SkillBundleCache
	logger     *slog.Logger

	mu           sync.Mutex
	workspaces   map[string]*workspaceState
	runtimeIndex map[string]Runtime // runtimeID -> Runtime for provider lookups
	// profileLaunchSpecs maps a custom runtime profile_id -> the absolute
	// executable path plus fixed launch args resolved for that profile
	// (MUL-3284). Populated in registerRuntimesForWorkspace when a profile's
	// command resolves; read by runTask to launch the custom command for a
	// claimed task. Guarded by mu.
	profileLaunchSpecs map[string]profileLaunchSpec
	reloading          sync.Mutex         // prevents concurrent workspace syncs
	runtimeSet         *runtimeSetWatcher // multi-subscriber pub/sub for runtime-set changes

	versionsMu    sync.RWMutex      // guards agentVersions
	agentVersions map[string]string // provider -> detected CLI version (set during registration)

	// registerSerial holds one mutex per workspace (workspace_id ->
	// *sync.Mutex), serializing the "send Register, record what it carried"
	// critical section (workspaceRegisterLock). Entries are never deleted — a
	// daemon tracks a handful of workspaces, and a mutex for a workspace that
	// went away is a few bytes, not a leak worth a lifecycle.
	registerSerial sync.Map

	// agentsAvailable holds the current built-in agent CLI availability set —
	// the same shape as cfg.Agents, which it supersedes as the read path.
	//
	// It is copy-on-write, NOT a mutable map: refreshAgentAvailability swaps in
	// a whole new map, and readers (agents()) take the pointer once and then
	// only read. cfg.Agents used to be read unlocked from task-execution paths
	// (resolveAgentEntry, runTask), so making the discovery set refreshable at
	// runtime (MUL-5439) would otherwise be a data race.
	agentsAvailable atomic.Pointer[map[string]AgentEntry]

	// skippedAgents records why a discovered provider did not make it into the
	// last registration round (version undetectable, below minimum). Purely
	// diagnostic: surfaced on /health so the UI can tell "not installed" apart
	// from "installed but dropped", instead of silently showing nothing
	// (MUL-5439). Guarded by skippedAgentsMu.
	skippedAgentsMu sync.RWMutex
	skippedAgents   map[string]string // provider -> human-readable reason

	// demotedProviders remembers the built-in providers whose version was
	// CONFIRMED below the minimum supported one and whose runtimes
	// demoteBelowMinimumRuntimes has already taken offline.
	//
	// Removing the rows is not enough on its own: a register call that was
	// already in flight when the demotion landed still carries the pre-demotion
	// payload, and its response arrives afterwards. Every apply path treats a
	// response as fresh truth, so it re-indexes the provider locally while the
	// server's upsert puts the row back online — reviving a CLI the daemon has
	// already proven it cannot run, until the next refresh tick notices.
	// Remembering the verdict lets the apply paths reject that late response.
	//
	// Machine-level, because the verdict is a property of the binary on this
	// host rather than of any one workspace: a stale register for workspace A
	// must not revive the provider for workspace B either. Cleared by the next
	// probe round that finds the provider acceptable again, which is also the
	// round that lets converge register it.
	//
	// Guarded by d.mu — deliberately the same lock every register-response
	// apply takes, which is what totally orders "record the verdict" against
	// "apply a response" instead of merely narrowing the window between them.
	demotedProviders map[string]demotionRecord // provider -> the evidence that condemned it and when

	// notExecutableSince is when each provider was FIRST observed to be
	// unrunnable, for the confirmation window in confirmNotExecutable. Entries
	// are cleared by the round that finds the provider healthy again. Guarded
	// by d.mu.
	notExecutableSince map[string]time.Time

	// demotionSeq is a monotonic counter stamped onto each demotion record so a
	// probe round can tell whether its evidence predates a verdict.
	//
	// Ordering the map writes is not enough on its own: version sampling
	// happens outside d.mu, so a round that started earlier, sampled an
	// acceptable version, and returned late could otherwise clear a hold
	// established by a NEWER below-minimum verdict — and once the hold is gone,
	// a stale register response walks through every guard above. A round
	// snapshots this counter before it samples anything and may only clear
	// holds recorded at or before that snapshot, which makes "my evidence is
	// newer than that verdict" a fact rather than a hope. Guarded by d.mu.
	demotionSeq uint64

	// resolvedPathsMu guards concrete executable paths paired with the version
	// detected for each. On POSIX these are self-heals cached after a pinned path
	// vanishes (MUL-4486). On Windows they are launch targets resolved from a
	// stable installer junction; that junction is followed on every launch so a
	// retarget takes effect even while the old release remains installed. Path
	// and version are stored together so no reader can launch a new binary under
	// stale version policy. Keyed by provider.
	resolvedPathsMu sync.RWMutex
	resolvedPaths   map[string]healedAgent
	// healGroup coalesces concurrent self-heal re-resolutions per provider so a
	// just-upgraded agent seen by many queued tasks at once pays for a single
	// login-shell probe + version detection instead of one per task (MUL-4486).
	healGroup singleflight.Group

	wsHBMu      sync.RWMutex         // guards wsHBLastAck
	wsHBLastAck map[string]time.Time // runtime_id -> last successful WS heartbeat ack timestamp

	// reconcile fans out a "re-check server state now" signal to subscribers
	// (watchTaskCancellation, workspaceSyncLoop) so the WS connect/reconnect
	// path can shrink coarse fallback reconciliation gaps to sub-second. See
	// reconcile.go and runTaskWakeupConnection.
	reconcile *reconcileBroadcaster
	// workspaceChanges is the account-scoped server hint for membership-set
	// changes. It stays separate from reconcile because a membership hint only
	// needs the minimal workspace list, while a WS reconnect also reconciles
	// runtime profiles that may have changed during the gap.
	workspaceChanges *workspaceChangeSignal

	// wsRPC carries generic request/response RPCs (e.g. tasks.claim, MUL-4257)
	// over the task-wakeup WS connection. It is attached to the live
	// connection in runTaskWakeupConnection and detached on disconnect; when
	// detached, callers fall back to HTTP.
	wsRPC *wsRPCClient

	// batchClaimUnsupported is set once a batch claim gets a 404 from the
	// server (no /api/daemon/tasks/claim route — an un-upgraded server), so
	// subsequent polls skip WS+batch and use the legacy per-runtime claim
	// directly. Reset when the WS (re)connects, so a server upgrade that
	// bounces the connection re-probes the batch route (MUL-4257).
	batchClaimUnsupported atomic.Bool
	// wsClaimHTTPFallbackAfter is set after an uncertain WS claim outcome. Once
	// the safety delay elapses, the next claim bypasses WS once and uses HTTP so
	// a flaky reconnecting WS cannot starve queued tasks indefinitely.
	wsClaimHTTPFallbackAfter atomic.Int64

	// runtimeGoneMu guards runtimeGoneInflight, reregisterNextAttempt, and
	// reregisterLastCompletedAt. The state lets heartbeat / poller / WS-ack
	// handlers converge on a single recovery path when they each detect that a
	// runtime row was deleted server-side without three of them stampeding
	// registerRuntimesForWorkspace.
	runtimeGoneMu             sync.Mutex
	runtimeGoneInflight       map[string]struct{}  // runtime_id -> currently recovering
	reregisterNextAttempt     map[string]time.Time // workspace_id -> earliest time the next re-register attempt may run
	reregisterLastCompletedAt map[string]time.Time // workspace_id -> wall-clock at which the last SUCCESSFUL re-register call returned (failures intentionally not stamped — see recordRegisterCompletion)

	// pendingWorkMu guards pendingWorkInflight and pendingWorkLastRun, which
	// coalesce and rate-limit server-pushed "heartbeat now" hints (MUL-5444).
	// Several UI surfaces can request the same runtime's model list within
	// milliseconds; without the guard each hint would fire its own out-of-band
	// heartbeat, and an authenticated caller looping the list-models endpoint
	// could turn that into a heartbeat amplifier.
	pendingWorkMu       sync.Mutex
	pendingWorkInflight map[string]struct{}  // runtime_id -> hint-driven heartbeat in flight
	pendingWorkLastRun  map[string]time.Time // runtime_id -> when the last hint-driven heartbeat started

	cancelFunc context.CancelFunc // set by Run(); called by triggerRestart
	rootCtx    context.Context    // set by Run(); used by long-running recoveries that must survive per-runtime ctx cancellation
	// restartMu guards restartBinary. Two goroutines can reach triggerRestart —
	// the server-triggered handleUpdate and the autoUpdateLoop — and
	// trySelfReload reads RestartBinary() from the latter to avoid racing the
	// former into a second handoff.
	restartMu     sync.Mutex
	restartBinary string // non-empty after a successful update; path to the new binary
	// brewTargetOnce caches the brew half of restartTargetBinary. The install
	// method and brew prefix cannot change for the lifetime of the process, and
	// trySelfReload now calls restartTargetBinary every check tick — without
	// the cache that is up to two uncached `brew --prefix` forks per tick.
	brewTargetOnce sync.Once
	brewInstall    bool        // resolved once: was this binary installed via brew?
	brewTarget     string      // "<prefix>/bin/orvilo" when brewInstall and the prefix resolved
	updating       atomic.Bool // prevents concurrent update attempts
	// activeTasks is the ownership-safe count of tasks currently in handleTask.
	// It deliberately includes preparation and local-directory waiters because
	// restart/update barriers must not kill any claimed task.
	activeTasks atomic.Int64
	// runningTasks counts live provider execution sessions, beginning only after
	// backend.Execute returns. It can briefly lag the server-side running state,
	// which starts during preparation before provider launch. resourceWaitTasks
	// counts tasks blocked on a local_directory path mutex. Both are diagnostic
	// /health dimensions and must never replace activeTasks in safety barriers.
	runningTasks      atomic.Int64
	resourceWaitTasks atomic.Int64
	ready             atomic.Bool // false until preflight completes; gates /health status (starting -> running)
	// reloadPendingReason explains why a confirmed orvilo version change hasn't
	// restarted the daemon yet (a task was running at the barrier check). Set
	// and cleared by trySelfReload, read by /health. Diagnostic only.
	reloadPendingReason atomic.Pointer[string]

	// claimMu guards pauseClaims and claimsInFlight. It is held only for the
	// microseconds it takes to make a decision; ClaimTask itself runs without
	// the lock so a slow per-runtime claim cannot stall auto-update or any
	// other poller.
	//
	// The pair is the auto-update path's barrier against the issue's
	// requirement that "升级过程中如果有 task 进来，会延后升级而不是中断 task":
	// runRuntimePoller refuses to call ClaimTask while pauseClaims is set, and
	// tryAutoUpdate refuses to flip pauseClaims while any poller is mid-claim
	// or any task is in handleTask. Together that closes the fetch-then-claim
	// race where a new task slipping in during the release-metadata fetch
	// would be cancelled by triggerRestart's root-ctx cancel.
	claimMu        sync.Mutex
	pauseClaims    bool // when true, the batch poller skips claiming
	claimsInFlight int  // pollers that have decided to claim but haven't yet handed the task off to handleTask

	activeEnvRootsMu   sync.Mutex
	activeEnvRootsCond *sync.Cond      // signalled when an in-flight env-root GC mutation finishes
	activeEnvRoots     map[string]int  // env root path -> reference count (handles reuse paths marked twice)
	deletingEnvRoots   map[string]bool // env roots reserved by GC; new tasks wait until the mutation finishes

	activeStoresMu   sync.Mutex
	activeStoresCond *sync.Cond      // signalled when an in-flight store deletion finishes, so a blocked markActive can proceed
	activeStores     map[string]int  // persistent store path (per-conversation Codex sessions, per-agent Hermes memories) -> live-task refcount; guards the store from GC mid-task (MUL-4424)
	deletingStores   map[string]bool // store paths a GC delete has reserved; markActive waits these out so a task never mounts a store mid-removal

	// repoCheckoutTasks binds the localhost /repo/checkout endpoint to the
	// task-scoped bearer token of a currently running agent. The request body is
	// never an identity source: workspace, task, agent, and allowed workdir all
	// come from this registry.
	repoCheckoutTasksMu sync.RWMutex
	repoCheckoutTasks   map[string]activeRepoCheckoutTask

	// localPathLocks serialises agent tasks whose project resource is a
	// local_directory pinned to this daemon. Two tasks targeting the same
	// on-disk path run sequentially; the second blocks on the lock and is
	// surfaced via the server-side waiting_local_directory status while it
	// waits. See MUL-2663.
	localPathLocks *LocalPathLocker

	// bgSyncs tracks background goroutines started by registerTaskRepos so
	// callers (notably tests using t.TempDir-backed cache roots) can wait for
	// them to drain before tearing the daemon down. Without this the bg
	// goroutine can race against t.TempDir cleanup, leaving a partially
	// deleted bare clone and an unrelated `not empty` cleanup failure.
	bgSyncs sync.WaitGroup

	terminalStore             *terminalreport.Store
	terminalSender            *terminalreport.Sender
	terminalPersistenceFailed atomic.Bool
	terminalRecoveryFailed    atomic.Bool
	terminalDeliveryBlocked   atomic.Bool
	runner                    taskRunner    // executes agent tasks; set to d.runTask by New(), overridable in tests
	cancelPollInterval        time.Duration // how often handleTask polls for server-side cancellation; overridable in tests
	// envRootBusyWait is how long a task that is entitled to a prior env root
	// waits for the previous run to let go of it before giving up and preparing
	// a fresh one. New() sets it; the zero value means "do not wait", which is
	// what focused unit tests want. See lockReusablePriorEnvRoot (MUL-6880).
	envRootBusyWait time.Duration
	// executionEnvironmentCommand resolves the killable helper used for
	// Prepare/Reuse. New always sets it; nil keeps focused unit tests in-process.
	executionEnvironmentCommand executionEnvironmentCommand
	// taskPrepareTimeout is the dispatched -> running hard deadline. New sets
	// the production default; zero-valued test daemons fall back to the same
	// default in effectiveTaskPrepareTimeout.
	taskPrepareTimeout time.Duration
	// runUpdateFn executes the brew-or-download upgrade. Set to d.runUpdate by
	// New() and overridable in tests so the auto-update poller can be exercised
	// without touching the real network or the brew CLI.
	runUpdateFn func(targetVersion string) (string, error)
}

type profileLaunchSpec struct {
	path      string
	version   string
	fixedArgs []string
}

// New creates a new Daemon instance.
func New(cfg Config, logger *slog.Logger) *Daemon {
	cacheRoot := filepath.Join(cfg.WorkspacesRoot, ".repos")
	skillCacheRoot := filepath.Join(cfg.WorkspacesRoot, ".skill-cache", "v1")
	client := NewClient(cfg.ServerBaseURL)
	// Tag every daemon HTTP request with the daemon's CLI version so the
	// server can split logs/metrics by client version (parallel to the CLI).
	client.SetVersion(cfg.CLIVersion)
	d := &Daemon{
		cfg:                       cfg,
		client:                    client,
		repoCache:                 repocache.New(cacheRoot, logger),
		skillCache:                NewSkillBundleCache(skillCacheRoot),
		logger:                    logger,
		workspaces:                make(map[string]*workspaceState),
		runtimeIndex:              make(map[string]Runtime),
		profileLaunchSpecs:        make(map[string]profileLaunchSpec),
		runtimeSet:                newRuntimeSetWatcher(),
		agentVersions:             make(map[string]string),
		skippedAgents:             make(map[string]string),
		resolvedPaths:             make(map[string]healedAgent),
		wsHBLastAck:               make(map[string]time.Time),
		activeEnvRoots:            make(map[string]int),
		deletingEnvRoots:          make(map[string]bool),
		activeStores:              make(map[string]int),
		deletingStores:            make(map[string]bool),
		localPathLocks:            NewLocalPathLocker(),
		runtimeGoneInflight:       make(map[string]struct{}),
		pendingWorkInflight:       make(map[string]struct{}),
		pendingWorkLastRun:        make(map[string]time.Time),
		reregisterNextAttempt:     make(map[string]time.Time),
		reregisterLastCompletedAt: make(map[string]time.Time),
		cancelPollInterval:        5 * time.Second,
		envRootBusyWait:           15 * time.Second,
		taskPrepareTimeout:        defaultTaskPrepareTimeout,
		reconcile:                 newReconcileBroadcaster(),
		workspaceChanges:          newWorkspaceChangeSignal(),
		wsRPC:                     newWSRPCClient(wsRPCResponseGrace),
	}
	d.activeEnvRootsCond = sync.NewCond(&d.activeEnvRootsMu)
	d.activeStoresCond = sync.NewCond(&d.activeStoresMu)
	// Seed the copy-on-write availability set from the startup probe. Callers
	// must go through d.agents() from here on; cfg.Agents is the initial value
	// only and does not track later refreshes.
	initialAgents := make(map[string]AgentEntry, len(cfg.Agents))
	for name, entry := range cfg.Agents {
		initialAgents[name] = entry
	}
	d.agentsAvailable.Store(&initialAgents)
	d.executionEnvironmentCommand = defaultExecutionEnvironmentCommand
	d.runner = taskRunnerFunc(d.runTask)
	d.runUpdateFn = d.runUpdate
	return d
}

// Run starts the daemon: resolves auth, registers runtimes, then polls for tasks.
func (d *Daemon) Run(ctx context.Context) error {
	// Wrap context so handleUpdate can cancel the daemon for restart.
	ctx, cancel := context.WithCancel(ctx)
	d.cancelFunc = cancel
	d.rootCtx = ctx
	defer cancel()

	// Bind health port early to detect another running daemon.
	healthLn, err := d.listenHealth()
	if err != nil {
		return err
	}

	agentNames := make([]string, 0, len(d.agents()))
	for name := range d.agents() {
		agentNames = append(agentNames, name)
	}
	logFields := []any{"version", d.cfg.CLIVersion, "agents", agentNames, "server", d.cfg.ServerBaseURL}
	if d.cfg.Profile != "" {
		logFields = append(logFields, "profile", d.cfg.Profile)
	}
	d.logger.Info("starting daemon", logFields...)
	d.logger.Debug("daemon config resolved",
		"daemon_id", d.cfg.DaemonID,
		"device_name", d.cfg.DeviceName,
		"workspaces_root", d.cfg.WorkspacesRoot,
		"health_port", d.cfg.HealthPort,
		"poll_interval", d.cfg.PollInterval,
		"heartbeat_interval", d.cfg.HeartbeatInterval,
		"agent_timeout", d.cfg.AgentTimeout,
		"idle_watchdog", d.cfg.AgentIdleWatchdog,
		// Logged explicitly because it is normally derived from idle_watchdog:
		// without it an operator cannot read the tool budget actually in effect.
		"tool_watchdog", d.cfg.AgentToolWatchdog,
		"opencode_idle_watchdog", d.cfg.OpenCodeIdleWatchdog,
		// Derived from the watchdog budget too (Codex's own timer is not
		// tool-aware), so it needs the same treatment as tool_watchdog: without
		// it the effective Codex budget is invisible until a timeout fires.
		"codex_semantic_inactivity", d.cfg.CodexSemanticInactivityTimeout,
		"max_concurrent_tasks", d.cfg.MaxConcurrentTasks,
		"gc_enabled", d.cfg.GCEnabled,
		"auto_update", d.cfg.AutoUpdateEnabled,
		"launched_by", d.cfg.LaunchedBy,
	)

	// Mark the daemon-owned workspaces tree before any task runs. A sandbox
	// fault can strip every ORVILO_* env var from an agent subprocess; the
	// per-workdir marker then only protects cwds inside the workdir, and a
	// subprocess that escaped to the workdir's parent would fall back to the
	// user's config PAT. The root marker makes the CLI fail closed anywhere
	// under the tree. Non-fatal: Prepare re-ensures it per task.
	if err := execenv.EnsureWorkspacesRootMarker(d.cfg.WorkspacesRoot); err != nil {
		d.logger.Warn("workspaces root marker not written; CLI fail-closed guard limited to task workdirs", "error", err)
	}

	// Load auth token from CLI config.
	if err := d.resolveAuth(); err != nil {
		return err
	}

	if err := d.openTerminalReports(); err != nil {
		healthLn.Close()
		return err
	}

	// Bind and serve the health port before the (potentially slow) preflight,
	// so `daemon start` and the desktop see a live "starting" daemon instead
	// of connection-refused while preflightAuth runs. preflightAuth's initial
	// workspace sync detects every configured agent's version by exec'ing it,
	// which on a cold cache with many agents takes ~20s. Liveness (port up) and
	// readiness (status:"running") are reported separately: /health stays
	// "starting" until d.ready is set after preflight, so a slow or *failing*
	// preflight is never misreported as a started daemon. resolveAuth has
	// already run, so a missing token still fails fast before we begin serving.
	go d.serveHealth(ctx, healthLn, time.Now())

	// Renew the PAT before the first API call, then do the initial
	// workspace sync. Both steps live in preflightAuth so the ordering
	// invariant (renew first) is enforced at one site instead of
	// scattered into Run, and tests can exercise the failure paths
	// without the full Run setup.
	if err := d.preflightAuth(ctx); err != nil {
		return err
	}

	// Replay saved results before admitting another provider execution.
	if err := d.terminalSender.Flush(ctx); err != nil {
		d.logger.Error("replay terminal reports", "error", err)
	}
	go d.terminalSender.Run(ctx)

	// Deregister runtimes on shutdown (uses a fresh context since ctx will be cancelled).
	defer d.deregisterRuntimes()

	// Start workspace sync loop to discover newly created workspaces.
	go d.workspaceSyncLoop(ctx)

	// Discover agent CLIs installed after startup (MUL-5439). Separate from the
	// workspace sync loop because that one runs on a thirty-minute consistency
	// interval — far too slow for "install a CLI, see it under Runtimes".
	go d.agentDiscoveryLoop(ctx)

	taskWakeups := make(chan taskWakeup, 256)
	go d.taskWakeupLoop(ctx, taskWakeups)
	go d.heartbeatLoop(ctx)
	go d.gcLoop(ctx)
	go d.autoUpdateLoop(ctx)
	go d.tokenRenewalLoop(ctx)

	// Preflight succeeded and the background loops are up: the daemon has
	// registered its runtimes and can now claim and run tasks. Flip /health
	// from "starting" to "running" — this is the signal `daemon start`'s
	// readiness wait blocks on, so success is reported only after startup
	// actually completed, not merely because the health port came up.
	d.ready.Store(true)
	d.logger.Debug("background loops launched (workspace-sync, task-wakeup, heartbeat, gc, auto-update, token-renewal); health now reporting ready")
	err = d.pollLoop(ctx, taskWakeups)
	d.logger.Debug("daemon main loop returning", "error", err)
	return err
}

// RestartBinary returns the path to the new binary if the daemon needs to restart
// after a successful update, or empty string if no restart is needed.
func (d *Daemon) RestartBinary() string {
	d.restartMu.Lock()
	defer d.restartMu.Unlock()
	return d.restartBinary
}
