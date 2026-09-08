package daemon

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync/atomic"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/daemon/execenv"
	"github.com/orvilo-ai/orvilo/server/pkg/agent"
	"github.com/orvilo-ai/orvilo/server/pkg/taskfailure"
)

// taskExecutionInput names the claim and local execution context for one
// run. The result is returned to supervision, which owns terminal reporting.
type taskExecutionInput struct {
	Task     Task
	Provider string
	Slot     int
	Log      *slog.Logger
}

// taskExecutionConfig contains only launch policy and task-local path settings.
// Registration, authentication state and the daemon's lifecycle are not visible.
type taskExecutionConfig struct {
	AgentIdleWatchdog               time.Duration
	AgentTimeout                    time.Duration
	CLIVersion                      string
	CodexFirstTurnNoProgressTimeout time.Duration
	CodexHandshakeTimeout           time.Duration
	CodexSemanticInactivityTimeout  time.Duration
	CodexThreadHandshakeTimeout     time.Duration
	DaemonID                        string
	HealthPort                      int
	OpenCodeIdleWatchdog            time.Duration
	Profile                         string
	ServerBaseURL                   string
	WorkspacesRoot                  string
}

// taskExecutionClient exposes task preparation and task-scoped runtime services.
// It cannot claim work, recover orphans, or submit a terminal result.
type taskExecutionClient interface {
	InvokeAgentPluginHook(context.Context, string, string, string, string, json.RawMessage) (json.RawMessage, error)
	MarkTaskWaitingLocalDirectory(context.Context, string, string) error
	RecordExecutionProvenance(context.Context, string, ExecutionProvenanceReport, bool) error
	ReportProgress(context.Context, string, string, int, int) error
	ResolveRemoteMCPCredential(context.Context, string, string, string) (http.Header, error)
	StartTask(context.Context, string) error
}

type localPathAcquirer interface {
	Acquire(context.Context, string, string, func(string)) (func(), error)
}

// taskExecution receives capabilities for one run, never the Daemon object.
// Shared ownership stays with supervision: callbacks operate the original locks
// and registries, and counters are pointers to their original atomic values.
type taskExecution struct {
	cfg                            taskExecutionConfig
	client                         taskExecutionClient
	logger                         *slog.Logger
	provider                       *providerExecution
	agentEntry                     func(string) (AgentEntry, bool)
	agentVersion                   func(string) string
	customProfileLaunchForRuntime  func(string) (profileLaunchSpec, bool)
	resolveAgentEntryForLaunch     func(context.Context, string, AgentEntry) (AgentEntry, string, error)
	defaultArgs                    func(string) []string
	registerTaskRepos              func(string, string, []RepoData)
	clearTaskRepoRefs              func(string, string)
	startTaskPrepareLeaseExtender  func(context.Context, Task, *slog.Logger) func()
	ensureTaskSkillBundles         func(context.Context, *Task) error
	markActiveEnvRoot              func(string)
	unmarkActiveEnvRoot            func(string)
	markActiveStore                func(string)
	unmarkActiveStore              func(string)
	lockReusablePriorEnvRoot       func(context.Context, Task, *localDirectoryAssignment, string) (*execenv.EnvRootClaim, string, os.FileInfo, bool, error)
	prepareExecutionEnvironment    func(context.Context, execenv.PrepareParams) (*execenv.Environment, error)
	reuseExecutionEnvironment      func(context.Context, execenv.ReuseParams) (*execenv.Environment, error)
	effectiveTaskPrepareTimeout    func() time.Duration
	watchTaskCancellation          func(context.Context, string, time.Duration, *slog.Logger) <-chan struct{}
	cancelPollInterval             time.Duration
	registerActiveRepoCheckoutTask func(string, activeRepoCheckoutTask)
	clearActiveRepoCheckoutTask    func(string)
	localPathLocks                 localPathAcquirer
	resourceWaitTasks              *atomic.Int64
}

func (e *taskExecution) run(ctx context.Context, input taskExecutionInput) (taskResult TaskResult, returnErr error) {
	task, provider, slot, taskLog := input.Task, input.Provider, input.Slot, input.Log
	// A claim carries the task-row agent id both at the top level and inside
	// the expanded agent configuration. The top-level id is authoritative
	// because it is also bound into the task-scoped token. Never prepare or
	// reuse a workdir when the two identities disagree.
	if err := validateTaskIdentity(task); err != nil {
		return TaskResult{}, err
	}

	// Refuse to spawn an agent without a workspace. An empty workspace_id
	// here would make ORVILO_WORKSPACE_ID empty in the agent env, and the
	// CLI would otherwise silently fall back to the user-global config — a
	// path that can leak operations into an unrelated workspace when
	// multiple workspaces share a host.
	if task.WorkspaceID == "" {
		return TaskResult{}, fmt.Errorf("refusing to spawn agent: task has no workspace_id (task_id=%s)", task.ID)
	}

	prepareTimeout := e.effectiveTaskPrepareTimeout()
	prepareCtx, cancelPrepare := context.WithTimeoutCause(ctx, prepareTimeout, errTaskPrepareTimeout)
	prepareComplete := false
	defer func() {
		cancelPrepare()
		if prepareComplete || returnErr == nil || !errors.Is(context.Cause(prepareCtx), errTaskPrepareTimeout) {
			return
		}
		// Collapse every deadline shape (context deadline, HTTP cancellation,
		// or the explicit waitForExecutionEnvironment cause) into one sentinel
		// that handleTask can classify as a retryable platform timeout.
		taskResult = TaskResult{}
		returnErr = fmt.Errorf("%w after %s", errTaskPrepareTimeout, prepareTimeout)
	}()

	// task.Repos is the authoritative repo list for this task — when the
	// claimed task belongs to a project with github_repo resources the server
	// has already narrowed it to project repos only. Make sure those URLs are
	// in the per-workspace allowlist and the local cache, otherwise
	// `patchbay repo checkout` would reject project-only URLs that aren't also
	// bound at the workspace level.
	e.registerTaskRepos(task.WorkspaceID, task.ID, task.Repos)
	defer e.clearTaskRepoRefs(task.WorkspaceID, task.ID)

	entry, ok := e.agentEntry(provider)
	// A custom runtime profile (MUL-3284) overrides the executable path: the
	// runtime's protocol_family is the provider (so agent.New still selects
	// the right backend), but the actual binary on PATH is the profile's
	// command_name, resolved at registration time and keyed by RuntimeID here.
	// Critically, a custom runtime can live on a host that has NO built-in
	// agent of the same provider installed, so when the runtime is custom we
	// synthesize an AgentEntry instead of hard-failing on the !ok lookup.
	var profileFixedArgs []string
	// resolvedVersion is the CLI version of the built-in binary entry.Path
	// resolves to, paired with the path by resolveAgentEntry so a just-upgraded
	// codex is never launched under the previous version's policy (MUL-4486).
	var resolvedVersion string
	// usesCustomProfileCommand distinguishes "this provider's own binary" from
	// "some other binary speaking this provider's protocol". Backends need it
	// for compatibility exceptions verified against a specific vendor's CLI,
	// which must not extend to arbitrary commands sharing a protocol family.
	var usesCustomProfileCommand bool
	if customSpec, isCustom := e.customProfileLaunchForRuntime(task.RuntimeID); isCustom {
		usesCustomProfileCommand = true
		entry.Path = customSpec.path
		resolvedVersion = customSpec.version
		// Filter here rather than relying on agent.New doing it, so that the
		// launch and the catalog lookups below agree on one prefix. They share
		// a discovery memo keyed on the command, and two spellings of the same
		// runtime would key two entries.
		profileFixedArgs = agent.FilterLaunchPrefix(provider, customSpec.fixedArgs, e.logger)
		ok = true
		e.logger.Info("task uses custom runtime profile command",
			"task_id", task.ID, "runtime_id", task.RuntimeID,
			"provider", provider, "command_path", customSpec.path,
			"fixed_args", len(profileFixedArgs))
	} else if ok {
		// Built-in provider: self-heal a pinned executable path that an in-place
		// upgrade deleted (MUL-4486). Only reached when no custom profile owns
		// the launch, so a custom runtime's path is never second-guessed and a
		// custom-only host pays no wasted re-resolution.
		var resolveErr error
		entry, resolvedVersion, resolveErr = e.resolveAgentEntryForLaunch(prepareCtx, provider, entry)
		if resolveErr != nil {
			return TaskResult{}, resolveErr
		}
	}
	if !ok {
		return TaskResult{}, fmt.Errorf("no agent configured for provider %q", provider)
	}

	stopPrepareLease := e.startTaskPrepareLeaseExtender(prepareCtx, task, taskLog)
	defer stopPrepareLease()

	if err := e.ensureTaskSkillBundles(prepareCtx, &task); err != nil {
		return TaskResult{}, err
	}

	agentName := "agent"
	var skills []SkillData
	var instructions string
	agentName = task.Agent.Name
	skills = task.Agent.Skills
	instructions = task.Agent.Instructions

	// Prepare isolated execution environment.
	// Repos are passed as metadata only — the agent checks them out on demand
	// via `patchbay repo checkout <url>`.
	taskCtx := execenv.TaskContextForEnv{
		IssueID:                   task.IssueID,
		IsAgentThreadContinuation: isAgentThreadContinuation(task),
		RuntimeID:                 task.RuntimeID,
		AgentThreadRootTaskID:     task.AgentThreadRootTaskID,
		TriggerCommentID:          task.TriggerCommentID,
		TriggerThreadID:           task.TriggerThreadID,
		CommentReplyTargets:       commentReplyThreads(task),
		NewCommentCount:           task.NewCommentCount,
		NewCommentsSince:          task.NewCommentsSince,
		PriorSessionResumed:       task.PriorSessionID != "",
		// MUL-5305: the server sets this when a more recent Codex session was
		// withheld (rollout missing) and PriorSessionID is an older fallback (or
		// absent). Seed the brief's continuity disclosure from it; the local
		// resume gates below only ever OR it to true, so the signal is monotonic.
		PriorSessionResumeUnavailable:    task.PriorSessionResumeUnavailable,
		AgentID:                          task.AgentID,
		AgentName:                        agentName,
		AgentInstructions:                instructions,
		AgentSkills:                      convertSkillsForEnv(skills),
		DisabledRuntimeSkills:            convertDisabledRuntimeSkillsForEnv(task.Agent, task.RuntimeID, provider),
		Repos:                            convertReposForEnv(task.Repos),
		ProjectID:                        task.ProjectID,
		ProjectTitle:                     task.ProjectTitle,
		ProjectDescription:               task.ProjectDescription,
		ProjectResources:                 convertProjectResourcesForEnv(task.ProjectResources),
		ChatSessionID:                    task.ChatSessionID,
		ChatChannelType:                  task.ChatChannelType,
		ChatChannelDeliversFiles:         task.ChatChannelDeliversFiles,
		AutomationRunID:                  task.AutomationRunID,
		AutomationID:                     task.AutomationID,
		AutomationTitle:                  task.AutomationTitle,
		AutomationDescription:            task.AutomationDescription,
		AutomationSource:                 task.AutomationSource,
		AutomationTriggerPayload:         strings.TrimSpace(string(task.AutomationTriggerPayload)),
		QuickCreatePrompt:                task.QuickCreatePrompt,
		HandoffNote:                      task.HandoffNote,
		IsTeamLeader:                     taskIsTeamLeader(task),
		RequestingUserName:               task.RequestingUserName,
		RequestingUserProfileDescription: task.RequestingUserProfileDescription,
		InitiatorType:                    task.InitiatorType,
		InitiatorID:                      task.InitiatorID,
		InitiatorName:                    task.InitiatorName,
		InitiatorEmail:                   task.InitiatorEmail,
		WorkspaceContext:                 task.WorkspaceContext,
		IssueStatuses:                    convertIssueStatusesForEnv(task.IssueStatuses),
		IssueStatusesOmitted:             task.IssueStatusesOmitted,
		ConnectedApps:                    task.ConnectedApps,
	}

	// Mark candidate env roots as active before any env work so the GC loop
	// can't reclaim artifacts inside them mid-execution. We mark both the
	// stable root for a fresh Prepare and the prior root for Reuse — they
	// usually differ (Reuse keeps the original task's directory).
	resolvedRoot, err := execenv.ResolveRootDir(taskRootDirParams(e.cfg.WorkspacesRoot, task))
	if err != nil {
		return TaskResult{}, fmt.Errorf("resolve stable task env root: %w", err)
	}
	e.markActiveEnvRoot(resolvedRoot)
	defer e.unmarkActiveEnvRoot(resolvedRoot)
	if task.PriorWorkDir != "" {
		priorRoot := filepath.Dir(task.PriorWorkDir)
		if priorRoot != resolvedRoot {
			e.markActiveEnvRoot(priorRoot)
			defer e.unmarkActiveEnvRoot(priorRoot)
		}
	}

	// Claim the env root HERE, in the daemon parent, and hold it for the whole
	// task run — the same lifetime as unmarkActiveEnvRoot above.
	//
	// It cannot be claimed inside preparation: production preparation runs in a
	// short-lived helper process (prepareExecutionEnvironment ->
	// PrepareIsolated), so a lock taken there dies with the helper and the
	// *os.File cannot cross its JSON response back to us. Claiming there would
	// leave the agent running with no protection at all — which is exactly the
	// re-dispatch window this guards.
	envClaim, err := execenv.ClaimEnvRoot(taskRootDirParams(e.cfg.WorkspacesRoot, task))
	if err != nil {
		return TaskResult{}, fmt.Errorf("claim execution environment: %w", err)
	}
	defer envClaim.Release()

	// Try to reuse the workdir from a previous task on the same (agent, issue) pair.
	var env *execenv.Environment
	// For a built-in codex task, use the version paired with the resolved path
	// so an in-place upgrade can't leave the sandbox policy on the old version
	// (MUL-4486). A custom codex runtime skips the self-heal, so resolvedVersion
	// is empty and it keeps the existing cached-version fallback — its binary is
	// the profile's own command, which the daemon never pins or version-detects.
	// Non-codex providers carry the value through without consuming it.
	codexVersion := e.agentVersion("codex")
	if provider == "codex" && resolvedVersion != "" {
		codexVersion = resolvedVersion
	}
	openclawBin := ""
	if provider == "openclaw" {
		openclawBin = entry.Path
	}
	// Resolve any local_directory assignment again here so runTask can plumb
	// LocalWorkDir into execenv. handleTask already validated + locked the
	// path for worker tasks; leader tasks intentionally skip the assignment.
	localAssignment, _ := localDirectoryAssignmentForTask(task, e.cfg.DaemonID)
	// Reuse intentionally skipped for local_directory tasks: the prior
	// WorkDir is the user's own path (always present) but the reuse path
	// loses the envRoot association the GC loop needs, and re-running
	// Prepare against a stable user path is cheap (no clone, no copy).
	// Leader tasks have no localAssignment; shouldReusePriorWorkdir separately
	// requires Prepare-time managed-env provenance and a daemon-owned marker
	// before allowing reuse, so a pre-fix leader session recorded against
	// local_directory still fails closed.
	var agentMcpConfig json.RawMessage
	var effectiveMcpConfig json.RawMessage
	var cursorMcpAuthSource string
	remoteMCPConfig, remoteMCPDiagnostics, remoteMCPBrokers, remoteMCPErr := startTaskRemoteMCPBrokers(
		prepareCtx, ctx, task.ID, provider, task.RemoteMCPConnections,
		func(resolveCtx context.Context, contributionID string) (http.Header, error) {
			return e.client.ResolveRemoteMCPCredential(resolveCtx, task.RemoteMCPDaemonToken, task.ID, contributionID)
		},
		taskLog,
	)
	if remoteMCPErr != nil {
		return TaskResult{}, fmt.Errorf("prepare Remote MCP broker: %w", remoteMCPErr)
	}
	if remoteMCPBrokers != nil {
		defer remoteMCPBrokers.Close()
	}
	for _, diagnostic := range remoteMCPDiagnostics {
		taskLog.Warn("Remote MCP degraded", "reason", diagnostic)
	}

	// Agent-trigger plugin hooks, as a second local MCP server beside the
	// broker. A failure to start it degrades to no plugin tools rather than
	// failing the task: an agent that cannot reach a plugin should still work
	// on the issue, which is the same rule that makes a failing tool call a
	// tool error rather than a task error.
	pluginHookConfig, pluginHookServer, pluginHookErr := startTaskPluginHookMCP(
		ctx, task.ID, task.PluginHookTools,
		func(callCtx context.Context, taskID, installationID, hookKey string, input json.RawMessage) (json.RawMessage, error) {
			return e.client.InvokeAgentPluginHook(callCtx, task.RemoteMCPDaemonToken, taskID, installationID, hookKey, input)
		},
		taskLog,
	)
	if pluginHookErr != nil {
		taskLog.Warn("plugin hook tools unavailable", "error", pluginHookErr)
	}
	if pluginHookServer != nil {
		defer pluginHookServer.Close()
	}
	if len(pluginHookConfig) > 0 {
		merged, mergeErr := mergeTaskRemoteMCPConfig(remoteMCPConfig, pluginHookConfig)
		if mergeErr != nil {
			taskLog.Warn("could not merge plugin hook MCP config", "error", mergeErr)
		} else {
			remoteMCPConfig = merged
		}
	}
	if task.Agent != nil {
		agentMcpConfig = task.Agent.McpConfig
		effectiveMcpConfig = agentMcpConfig
		if merged, mergeErr := mergeRuntimeAndAgentMcpConfig(provider, agentMcpConfig); mergeErr != nil {
			taskLog.Warn("mcp_config: runtime merge failed; using agent configuration only",
				"provider", provider,
				"error", mergeErr,
			)
		} else {
			effectiveMcpConfig = merged
		}
		if len(remoteMCPConfig) > 0 {
			merged, mergeErr := mergeTaskRemoteMCPConfig(effectiveMcpConfig, remoteMCPConfig)
			if mergeErr != nil {
				return TaskResult{}, fmt.Errorf("merge Remote MCP broker configuration: %w", mergeErr)
			}
			effectiveMcpConfig = merged
		}
		if provider == "cursor" {
			cursorMcpAuthSource = strings.TrimSpace(task.Agent.CustomEnv[execenv.CursorMcpAuthSourceEnv])
		}
	}
	// Decode openclaw-specific runtime_config knobs once so reuse / prepare /
	// ExecOptions all see the same mode + gateway pin (issue #3260). Parse
	// failures fail soft to local mode — a broken JSON blob must never block
	// task dispatch.
	var openclawMode string
	var openclawGateway execenv.OpenclawGatewayPin
	if task.Agent != nil && provider == "openclaw" {
		openclawMode, openclawGateway = decodeOpenclawRuntimeConfig(task.Agent.RuntimeConfig, e.logger)
	}
	var agentEnvOverrides map[string]string
	var agentCustomArgs []string
	if task.Agent != nil {
		agentEnvOverrides = task.Agent.CustomEnv
		agentCustomArgs = task.Agent.CustomArgs
	}
	// Effective Codex CLI args the task will launch with, normalized through the
	// same agent.NormalizeCodexLaunchArgs pipeline buildCodexArgs uses (shell
	// unquoting + blocked-flag filtering), preserving its ExtraArgs
	// (profile-fixed + daemon defaults) vs CustomArgs (per-agent custom_args)
	// split so the filtering matches launch exactly. Threaded into execenv so
	// the Windows sandbox decision can honor a `-c windows.sandbox=...` override
	// that never lands in config.toml — even when it arrives shell-quoted —
	// instead of silently downgrading a user's isolation opt-in (MUL-4957).
	var codexSandboxArgs []string
	if provider == "codex" {
		// profileFixedArgs still belongs in this reconstruction even though it
		// no longer travels via ExtraArgs: it is a launch prefix now, so it is
		// still on codex's argv, and a `-c windows.sandbox=...` written there
		// must still be visible to the sandbox decision.
		extraArgs := append(append([]string{}, profileFixedArgs...), e.defaultArgs(provider)...)
		codexSandboxArgs = agent.NormalizeCodexLaunchArgs(extraArgs, agentCustomArgs, effectiveMcpConfig, e.logger)
	}
	// Hermes: resolve the overlay source home through one resolver contract —
	// the selection parsed from custom_args (agent.ParseHermesProfileArgs) plus
	// the agent's custom_env HERMES_HOME feed execenv.ResolveHermesProfile, which
	// reproduces Hermes' own profile semantics (root derivation, explicit vs.
	// sticky selection, reserved/invalid failure). A reserved/invalid selection
	// fails the task closed, matching Hermes' sys.exit(1). The selected source
	// home is exported to hermesEnv["HERMES_HOME"] so ${HERMES_HOME} in a
	// profile's skills.external_dirs expands against the selected profile home,
	// as native Hermes does before loading config.yaml. The parsed occurrence is
	// stripped from the acp argv at launch (only when the overlay is built) so
	// the flag can't re-point HERMES_HOME past the overlay.
	var hermesSourceHome string
	var hermesSourceMustExist bool
	var hermesEnv map[string]string
	var hermesMemoryStore string
	var hermesSessionStore string
	if provider == "hermes" {
		// Resolve from the argv hermes will actually parse — launch prefix,
		// `acp`, then the filtered custom args — which agent.HermesLaunchArgv
		// assembles the same way the backend does. A custom runtime profile's
		// fixed_args are the launch prefix now, so they are scanned before
		// custom_args, and the backend's own `acp` token sits between them and
		// participates in the scan. Approximating that argv reads a different
		// profile than the process does, and the overlay ends up seeded from
		// the wrong home (GH #7046).
		sel := agent.ParseHermesProfileArgs(agent.HermesLaunchArgv(profileFixedArgs, agentCustomArgs, e.logger))
		res := execenv.ResolveHermesProfile(agentEnvOverrides["HERMES_HOME"], sel.Name, sel.Found, sel.Inline)
		if res.Err != nil {
			return TaskResult{}, fmt.Errorf("resolve hermes profile: %w", res.Err)
		}
		hermesSourceHome = res.SourceHome
		hermesSourceMustExist = res.MustExist
		// Which home the overlay is seeded from decides whether the task sees
		// the user's provider config at all, and it is derived from the daemon
		// PROCESS environment — invisible from the shell the user tests
		// `hermes acp` in, which is why a mismatch reads as "works by hand,
		// fails under Patchbay" (GH #6872). One line, at Info, so the answer is
		// in the daemon log before anything fails rather than reconstructed
		// afterwards.
		taskLog.Info("hermes home resolved",
			"source_home", hermesSourceHome,
			"from_custom_env", strings.TrimSpace(agentEnvOverrides["HERMES_HOME"]) != "",
			"must_exist", hermesSourceMustExist,
		)
		hermesEnv = sanitizeAgentEnv(agentEnvOverrides)
		if hermesEnv == nil {
			hermesEnv = map[string]string{}
		}
		hermesEnv["HERMES_HOME"] = res.SourceHome
		// The overlay links memories/ here so the agent's long-term memory
		// survives the task instead of being reset by every run (#6638). Keyed on
		// the resolved source home so switching an agent's profile switches its
		// memory line, matching Hermes' own "a profile is an isolated instance"
		// model. Guarded from the GC for the whole task, as the Codex store below.
		if store := execenv.HermesMemoryStorePath(e.cfg.Profile, task.AgentID, res.SourceHome); store != "" {
			hermesMemoryStore = store
			e.markActiveStore(store)
			defer e.unmarkActiveStore(store)
		}
		// The overlay links state.db here so the conversation transcript
		// survives the task and a follow-up turn can actually resume it
		// (GH #6806). Keyed on (agent, resolved source home, conversation):
		// tasks of one conversation are serial, so the shard has a single
		// writer, while two issues never share a database. Guarded from the GC
		// for the whole task, as the stores above and below.
		if store := execenv.HermesSessionStorePath(e.cfg.Profile, task.AgentID, res.SourceHome, taskCtx); store != "" {
			hermesSessionStore = store
			e.markActiveStore(store)
			defer e.unmarkActiveStore(store)
		}
	}
	// Reasonix locates its user config from the environment (REASONIX_HOME, and
	// the platform config dirs behind it), which an agent's custom_env may
	// re-point or clear. The per-task reasonix.toml has to restate the
	// permissions from whichever config the child ends up loading, so the deny
	// rules the runtime owner set there survive the task-scoped config that
	// overrides them — hence the same sanitized env the child is launched with.
	var reasonixEnv map[string]string
	if provider == "reasonix" {
		reasonixEnv = sanitizeAgentEnv(agentEnvOverrides)
	}
	// Guard this task's per-issue Codex session store from the GC for the whole
	// task, starting before Prepare/Reuse mounts it — so a prune that samples the
	// store's stale (pre-remount) mtime cannot reclaim it out from under a resume
	// of a long-idle issue (MUL-4424). No-op for non-Codex tasks / no stable key.
	if provider == "codex" {
		if store := execenv.CodexSessionStorePath(e.cfg.Profile, taskCtx); store != "" {
			e.markActiveStore(store)
			defer e.unmarkActiveStore(store)
		}
	}
	envReused := false
	priorClaim, priorWorkDir, lockedPriorInfo, reusable, reuseErr := e.lockReusablePriorEnvRoot(ctx, task, localAssignment, envClaim.RootDir())
	if reuseErr != nil {
		// Cancelled while waiting for the previous run to let go of its
		// directory. Ending here IS the behaviour: falling through would
		// prepare a whole environment — repo checkout included — for a task
		// nobody is waiting for any more.
		return TaskResult{}, reuseErr
	}
	if reusable {
		defer priorClaim.Release()
		// Deterministic seam for the last-window regression: tests swap the
		// directory here, after the claim is settled and before Reuse resolves
		// the path by name.
		if reuseBeforeUseTestHook != nil {
			reuseBeforeUseTestHook()
		}
		var err error
		env, err = e.reuseExecutionEnvironment(prepareCtx, execenv.ReuseParams{
			WorkspacesRoot: e.cfg.WorkspacesRoot,
			Profile:        e.cfg.Profile,
			// The canonical path the lock was taken on. Handing Reuse the raw
			// PriorWorkDir instead would re-resolve it, so the directory we
			// locked and the directory we use could differ.
			WorkDir:               priorWorkDir,
			Provider:              provider,
			CodexVersion:          codexVersion,
			ResumeSessionID:       task.PriorSessionID,
			OpenclawBin:           openclawBin,
			McpConfig:             effectiveMcpConfig,
			CursorMcpAuthSource:   cursorMcpAuthSource,
			OpenclawGateway:       openclawGateway,
			HermesSourceHome:      hermesSourceHome,
			HermesSourceMustExist: hermesSourceMustExist,
			HermesEnv:             hermesEnv,
			HermesMemoryStore:     hermesMemoryStore,
			HermesSessionStore:    hermesSessionStore,
			ReasonixEnv:           reasonixEnv,
			CodexCustomArgs:       codexSandboxArgs,
			Task:                  taskCtx,
		})
		if err != nil {
			return TaskResult{}, fmt.Errorf("reuse execution environment: %w", err)
		}
		// Reuse resolves priorWorkDir by name, so confirm what it actually
		// opened is still the directory we hold the lock on. An fd cannot cross
		// into the preparation helper process, so the name is the only thing
		// that can be handed over; this turns "silently ran somewhere else"
		// into "declined and started clean". See lockReusablePriorEnvRoot for
		// what remains uncovered.
		if env != nil && lockedPriorInfo != nil {
			usedInfo, statErr := os.Stat(filepath.Dir(env.WorkDir))
			if statErr != nil || !os.SameFile(lockedPriorInfo, usedInfo) {
				taskLog.Info("reused workdir is not the directory that was claimed; starting a fresh environment",
					"task", shortID(task.ID))
				env = nil
			}
		}
		// Reuse can decline (nil) and fall through to a fresh Prepare below.
		// Whether it did decides whether an env-root-scoped session store — the
		// Hermes overlay's task-local state.db — carried over from the prior task.
		envReused = env != nil
	}
	if env == nil {
		var err error
		prepParams := execenv.PrepareParams{
			WorkspacesRoot:  e.cfg.WorkspacesRoot,
			Profile:         e.cfg.Profile,
			WorkspaceID:     task.WorkspaceID,
			WorkspaceSlug:   task.WorkspaceSlug,
			TaskID:          task.ID,
			IssueIdentifier: task.IssueIdentifier,
			AgentName:       agentName,
			// This run already holds the claim (envClaim above) and the reset
			// it implies; preparation must not try to take it again.
			EnvRootPreclaimed:     true,
			Provider:              provider,
			CodexVersion:          codexVersion,
			OpenclawBin:           openclawBin,
			McpConfig:             effectiveMcpConfig,
			CursorMcpAuthSource:   cursorMcpAuthSource,
			OpenclawGateway:       openclawGateway,
			HermesSourceHome:      hermesSourceHome,
			HermesSourceMustExist: hermesSourceMustExist,
			HermesEnv:             hermesEnv,
			HermesMemoryStore:     hermesMemoryStore,
			HermesSessionStore:    hermesSessionStore,
			ReasonixEnv:           reasonixEnv,
			CodexCustomArgs:       codexSandboxArgs,
			Task:                  taskCtx,
		}
		if localAssignment.UsesWorktree() {
			prepParams.LocalWorktree = &execenv.LocalWorktreeParams{LocalPath: localAssignment.AbsPath, CommittedOnly: localAssignment.Ref.WorktreeBase == "head"}
			// Take the per-path mutex for the snapshot alone, then hand it
			// straight back — long enough to read a consistent tree, short
			// enough that worktree tasks still overlap for the run itself.
			//
			// A worktree task skips this lock for its execution, but the
			// snapshot is the one moment it READS the user's directory, and the
			// same real path can be attached to another project as an in_place
			// resource (each project may attach it once, so several can).
			// Snapshotting underneath a running in_place task would capture a
			// half-written tree plus that task's in-flight sidecars.
			//
			// The wait gets the same visibility plumbing as the in-place
			// acquire in acquireLocalDirectoryLockIfNeeded, because the holder
			// can be an in-place task that runs for hours: without the status
			// update the user sees a bare "preparing" with no hint the task is
			// queued behind the directory, and without the poller a task the
			// user cancels keeps its daemon slot pinned until the prepare
			// timeout — the run-phase cancellation watcher only starts after
			// launch. The prepare-lease extender is already running for this
			// whole phase, so only status, accounting, and cancellation are
			// mirrored here.
			waitCtx, waitCancel := context.WithCancel(prepareCtx)
			defer waitCancel()
			pollInterval := e.cancelPollInterval
			if pollInterval == 0 {
				pollInterval = 5 * time.Second
			}
			// LocalPathLocker invokes onWait synchronously, in this goroutine,
			// at most once per Acquire — see the in-place call site.
			waitCounted := false
			release, lockErr := e.localPathLocks.Acquire(waitCtx, localAssignment.RealPath, task.ID, func(holder string) {
				e.resourceWaitTasks.Add(1)
				waitCounted = true
				reason := fmt.Sprintf("local_directory %s", localAssignment.AbsPath)
				if holder != "" {
					reason = fmt.Sprintf("%s (held by task %s)", reason, shortID(holder))
				}
				taskLog.Info("local_directory: worktree snapshot waiting for holder",
					"holder", shortID(holder))
				if waitErr := e.client.MarkTaskWaitingLocalDirectory(waitCtx, task.ID, reason); waitErr != nil {
					// Non-fatal: the wait still happens, the UI just won't
					// show the explicit "waiting" badge.
					taskLog.Warn("local_directory: mark waiting status failed", "error", waitErr)
				}
				cancelled := e.watchTaskCancellation(waitCtx, task.ID, pollInterval, taskLog)
				go func() {
					select {
					case <-cancelled:
						waitCancel()
					case <-waitCtx.Done():
					}
				}()
			})
			if waitCounted {
				e.resourceWaitTasks.Add(-1)
			}
			if lockErr != nil {
				return TaskResult{}, fmt.Errorf("local_directory worktree: wait for a consistent snapshot of %s: %w",
					localAssignment.AbsPath, lockErr)
			}
			env, err = e.prepareExecutionEnvironment(prepareCtx, prepParams)
			release()
			if err != nil {
				return TaskResult{}, fmt.Errorf("prepare execution environment: %w", err)
			}
		} else {
			if localAssignment != nil {
				prepParams.LocalWorkDir = localAssignment.AbsPath
			}
			env, err = e.prepareExecutionEnvironment(prepareCtx, prepParams)
			if err != nil {
				return TaskResult{}, fmt.Errorf("prepare execution environment: %w", err)
			}
		}
	}
	// Belt-and-suspenders: also mark whatever root we ended up with, in case
	// future changes diverge from ResolveRootDir.
	if env.RootDir != resolvedRoot && env.RootDir != "" {
		e.markActiveEnvRoot(env.RootDir)
		defer e.unmarkActiveEnvRoot(env.RootDir)
	}
	executionWorkspace := env.WorkDir
	if env.LocalWorktree != nil {
		executionWorkspace = env.LocalWorktree.Path
	}
	// Finalize the worktree on EVERY exit path, success or failure: commit
	// whatever the agent left uncommitted, then unregister the worktree from
	// the user's repo. Deferred against the named return so a task that fails
	// mid-run still hands back the branch holding its partial work instead of
	// letting `git worktree remove --force` delete it. A failing task is
	// exactly when the user most wants to see how far the agent got.
	//
	// In-place local_directory runs never enter this block: their WorkDir is
	// already durable, so DurableWorkDir deliberately stays absent instead of
	// duplicating the same path under two lifecycle meanings.
	if env.LocalWorktree != nil {
		defer func() {
			if taskResult.WorkDir == "" {
				taskResult.WorkDir = env.WorkDir
			}
			if taskResult.EnvRoot == "" {
				taskResult.EnvRoot = env.RootDir
			}
			outcome, finalizeErr := env.LocalWorktree.Finalize(taskLog)
			if outcome.Branch != "" {
				taskResult.BranchName = outcome.Branch
			}
			if finalizeErr == nil {
				// Finalize may auto-commit the agent's edits and then remove the
				// worktree. Read the branch ref from the owning repository now so
				// provenance points at the commit that was actually delivered.
				if outcome.Branch != "" {
					facts, captureErr := execenv.ReadFinalizedExecutionProvenance(
						env.LocalWorktree.GitRoot, executionWorkspace, outcome.Branch)
					if captureErr != nil {
						taskLog.Debug("execution provenance capture after worktree finalization unavailable", "error", captureErr)
					} else {
						taskResult.ExecutionRepoIdentity = facts.RepoIdentity
						taskResult.ExecutionWorkspace = facts.ExecutionWorkspace
						taskResult.ExecutionHeadBranch = facts.HeadBranch
						taskResult.ExecutionHeadSHA = facts.HeadSHA
						taskResult.ExecutionHeadState = facts.HeadState
					}
				}
				// The configured local_directory becomes authoritative only after
				// Finalize confirms the disposable task worktree is actually gone.
				if localAssignment != nil {
					taskResult.DurableWorkDir = localAssignment.AbsPath
				}
				return
			}
			// Finalize could not complete its delivery contract, so the task
			// worktree remains authoritative. This covers both an uncommitted
			// change set and a committed branch whose worktree removal could not
			// be confirmed. Fail the task: reporting success or a durable project
			// directory here would hide the path that still needs attention.
			//
			// Wrapped in worktreePreservedError so the cancel path can
			// recognise it: a cancelled task discards its result and error, but
			// THIS error names the preserved worktree holding the agent's work
			// and must ride the cancel ack instead of vanishing into a log.
			// Joined rather than replacing an earlier failure — that one is
			// usually the more useful primary cause, but the preserved path
			// must not be displaced by it.
			taskLog.Error("local_directory: worktree finalize incomplete; keeping the task worktree authoritative",
				"error", finalizeErr, "preserved_path", outcome.PreservedPath)
			wrapped := &worktreePreservedError{err: fmt.Errorf("local_directory worktree: %w", finalizeErr)}
			if returnErr == nil {
				returnErr = wrapped
			} else {
				returnErr = errors.Join(returnErr, wrapped)
			}
		}()
	}
	// Workdir is preserved for reuse by future tasks on the same (agent,
	// issue) pair in cloud mode; the work_dir path is stored in DB on task
	// completion and passed back via PriorWorkDir on the next claim, so
	// rewriting the marker block in place is the right behavior.
	//
	// In local_directory mode the workdir is the user's own repo, reuse is
	// already disabled above (see localAssignment == nil), and the brief
	// would otherwise live on inside the user's repository — a subsequent
	// manual `claude` / `codex` run in that directory would pick
	// up stale Patchbay instructions (issue id, trigger comment id, reply
	// rules) and start acting on the previous task's context. Excise the
	// marker block on the way out instead.
	//
	// Worktree mode runs the same pass for a different reason: the worktree is
	// disposable, but its branch is the deliverable, and Finalize commits
	// whatever is still on disk. Without this the sidecars would land in every
	// task's diff. The .git/info/exclude trick repocache uses for github_repo
	// worktrees is not available here — a linked worktree resolves info/exclude
	// to the user's own common git dir, so using it would silently change what
	// `git status` hides in the user's checkout. Removing the files we wrote is
	// both narrower and exact; it also leaves a genuine agent edit to a tracked
	// CLAUDE.md intact, since CleanupRuntimeConfig only excises our marker block.
	//
	// Ordering: registered immediately after the Finalize defer above, so LIFO
	// runs cleanup first and Finalize commits an already-clean worktree. It must
	// also precede every early return between here and provider launch
	// (temp-dir setup, StartTask): those paths still run Finalize, and without
	// this pass Finalize would auto-commit the sidecars Prepare just wrote and
	// deliver a branch whose only content is Patchbay's own runtime files — or,
	// in place, leave them behind in the user's tree.
	if env.LocalDirectory || env.LocalWorktree != nil {
		defer func() {
			var cleanupErr error
			if cerr := execenv.CleanupRuntimeConfig(env.WorkDir, provider); cerr != nil {
				cleanupErr = cerr
				e.logger.Warn("execenv: cleanup runtime config failed", "error", cerr)
			}
			// Excise the sidecar tree (.agent_context/, .patchbay/,
			// provider-specific .claude/skills/ etc.) that Prepare wrote
			// into the user's repo. Without this pass the user's tree
			// accumulates one directory layer per task — see MUL-2784.
			// CleanupRuntimeConfig handles the runtime brief inside
			// CLAUDE.md / AGENTS.md; CleanupSidecars handles
			// every other file Prepare placed under WorkDir. Together
			// they round-trip the workdir to its exact pre-task bytes.
			if cerr := execenv.CleanupSidecars(env.RootDir); cerr != nil {
				if cleanupErr == nil {
					cleanupErr = cerr
				}
				e.logger.Warn("execenv: cleanup sidecars failed", "error", cerr)
			}
			// In worktree mode a failed cleanup is NOT survivable: Finalize is
			// about to `git add -A`, so whatever the cleanup could not remove
			// gets committed and delivered as the task's branch — a diff whose
			// content is Patchbay's own runtime files, which is precisely what
			// this mode promises never to produce. Tell Finalize to abort
			// instead, so nothing is committed and the worktree is kept for
			// inspection. (In place there is no commit and no branch, so a
			// cleanup failure stays a warning: the leftover files are visible
			// in the user's own tree and removable by hand.)
			if cleanupErr != nil && env.LocalWorktree != nil {
				env.LocalWorktree.AbortWithReason(fmt.Errorf(
					"could not remove the runtime's own files from the worktree before committing: %w", cleanupErr))
			}
		}()
	}
	// In-place workspaces survive terminal cleanup, so their checkout facts can
	// be captured in a defer. Worktree mode captures after Finalize above; a
	// pre-finalization HEAD would be stale whenever Finalize auto-commits.
	if env.LocalWorktree == nil {
		defer func() {
			facts, captureErr := execenv.ReadExecutionProvenance(executionWorkspace)
			if captureErr != nil {
				taskLog.Debug("execution provenance capture unavailable", "error", captureErr)
				return
			}
			taskResult.ExecutionRepoIdentity = facts.RepoIdentity
			taskResult.ExecutionWorkspace = facts.ExecutionWorkspace
			taskResult.ExecutionHeadBranch = facts.HeadBranch
			taskResult.ExecutionHeadSHA = facts.HeadSHA
			taskResult.ExecutionHeadState = facts.HeadState
		}()
	}
	taskTempDir, taskTempLock, err := ensureTaskTempDir(env.RootDir, task.WorkspaceID, task.ID)
	if err != nil {
		return TaskResult{}, fmt.Errorf("prepare task temp dir: %w", err)
	}
	defer func() {
		// Drop the execution lock before removing the directory: while it is
		// held the GC sweep correctly refuses to touch this directory, so a
		// removal that fails here (a file inside still open — the Windows case
		// in #7364) would otherwise leave the directory pinned until the daemon
		// exits. Released first, the next GC cycle acquires the lock, sees the
		// owner is gone and reclaims it. Nothing waits on the task path.
		//
		// RemoveTaskTempDir rather than os.RemoveAll so that a cleanup which
		// fails here leaves the .task_lock marker intact — without it the next
		// sweep could not tell this directory from a pre-lock leftover.
		execenv.ReleaseTaskTempLock(taskTempLock)
		if cerr := execenv.RemoveTaskTempDir(taskTempDir); cerr != nil {
			taskLog.Warn("task temp dir cleanup failed", "path", taskTempDir, "error", cerr)
		}
	}()

	// Issue #3999 race A: now that env.WorkDir is on disk, transition the
	// server-side state machine dispatched (or waiting_local_directory) →
	// running. Calling StartTask before Prepare/Reuse let any consumer
	// that read status==running and resolved
	// /patchbay_workspaces/{ws}/{short-id}/workdir hit FileNotFoundError in
	// the microsecond window before os.MkdirAll ran.
	//
	// On error we return early so handleTask's existing FailTask +
	// taskfailure.Classify path records the failure with the same
	// "start task failed: <…>" string and the same failure_reason
	// taxonomy as before — see MUL-2946 for the classifier contract.
	if err := e.client.StartTask(prepareCtx, task.ID); err != nil {
		stopPrepareLease()
		return TaskResult{}, fmt.Errorf("start task failed: %w", err)
	}
	stopPrepareLease()
	prepareComplete = true
	cancelPrepare()
	// This early report lets the server retain the initial branch/workspace
	// facts while the agent is running. The terminal callback carries a fresh
	// snapshot as well, which is authoritative if the branch was force-pushed.
	if facts, captureErr := execenv.ReadExecutionProvenance(executionWorkspace); captureErr == nil && facts.RepoIdentity != "" {
		provenance := ExecutionProvenanceReport{
			RepoIdentity:       facts.RepoIdentity,
			ExecutionWorkspace: facts.ExecutionWorkspace,
			HeadBranch:         facts.HeadBranch,
			HeadSHA:            facts.HeadSHA,
			HeadState:          facts.HeadState,
		}
		provenanceCtx, provenanceCancel := context.WithTimeout(context.WithoutCancel(ctx), 10*time.Second)
		if reportErr := e.client.RecordExecutionProvenance(provenanceCtx, task.ID, provenance, false); reportErr != nil {
			taskLog.Debug("initial execution provenance report failed", "error", reportErr)
		}
		provenanceCancel()
	}
	_ = e.client.ReportProgress(ctx, task.ID, fmt.Sprintf("Launching %s", provider), 1, 2)

	resumeReachable := gateResumeToReachableSession(&task, &taskCtx, provider, env.WorkDir, sessionHomeReachable(provider, env, envReused), taskLog)
	// A reused workdir is necessary but not sufficient for a Codex resume: the
	// prior thread's rollout must actually be present in this task's CODEX_HOME
	// sessions (MUL-4424 isolates them). Drop the resume before the brief is
	// generated below if it isn't, so we never tell the agent it is continuing a
	// conversation Codex will silently restart from scratch.
	if resumeReachable {
		gateCodexResumeToRolloutPresence(&task, &taskCtx, provider, env.CodexHome, taskLog)
	}
	// A source-neutral Agent continuation is an explicit request to continue
	// the provider conversation. Once the reachability gates have cleared its
	// only session pointer, starting a fresh provider thread would silently
	// turn that follow-up into an unrelated run. Persist the unavailable state
	// through the normal terminal failure callback instead of invoking a
	// backend at all.
	strictContinuation := isAgentThreadContinuation(task)
	if strictContinuation && task.PriorSessionID == "" {
		taskLog.Warn("agent thread continuation: prior provider session unavailable; refusing fresh launch")
		return TaskResult{
			Status:                "blocked",
			Comment:               agentThreadContinuationResumeUnavailableMessage,
			WorkDir:               env.WorkDir,
			EnvRoot:               env.RootDir,
			FailureReason:         taskfailure.ReasonAgentUnknown.String(),
			SessionRolloutMissing: true,
		}, nil
	}

	// Inject runtime-specific config (meta skill) so the agent discovers .agent_context/.
	runtimeBrief, err := execenv.InjectRuntimeConfig(env.WorkDir, provider, taskCtx)
	if err != nil {
		e.logger.Warn("execenv: inject runtime config failed (non-fatal)", "error", err)
	}
	// An exempt turn runs in the user's directory without having queued for it,
	// so a sibling coding task may be writing to the same tree right now. That
	// is the one thing it cannot work out from its own context — tell it.
	// Worktree mode is excluded: there the tree is this task's private checkout.
	var promptOptions []PromptOption
	if localAssignment != nil && !localAssignment.UsesWorktree() && localDirectoryLockExempt(task) {
		promptOptions = append(promptOptions, WithSharedLocalDirectory())
	}
	// Worktree mode hands this turn a tree that is mid-merge when the user's
	// edits since the previous turn collided with the branch's own work. The
	// conflict is deliberately left in place for the agent to resolve, so the
	// prompt has to be the thing that tells it (MUL-6881).
	if env.LocalWorktree != nil && len(env.LocalWorktree.ReplayConflicts) > 0 {
		promptOptions = append(promptOptions, WithWorktreeReplayConflicts(env.LocalWorktree.ReplayConflicts))
	}
	prompt := BuildPrompt(task, provider, promptOptions...)

	// Pass task-scoped auth credentials and context so the spawned agent CLI
	// can call the Patchbay API and the local daemon (e.g. `patchbay repo checkout`).
	// ORVILO_TASK_SLOT is allocated from the daemon-wide concurrency pool, not
	// per-agent. When one daemon hosts multiple agents, slots index shared
	// daemon-level resources such as GPUs.
	// ORVILO_TOKEN is bound to (agent, task) by the server. Never fall back
	// to the daemon's own credential here: doing so lets agent CLI writes land
	// as the runtime owner's member actor and can retrigger the same agent.
	agentToken, err := taskScopedAuthToken(task)
	if err != nil {
		taskLog.Error("task auth token invalid; refusing to start agent", "error", err)
		return TaskResult{}, err
	}
	agentEnv := taskOrviloEnvironment(task, agentName, agentToken, env.OrviloConfigRoot, e.cfg.WorkspacesRoot, e.cfg.ServerBaseURL, e.cfg.HealthPort, slot, taskTempDir)
	if checkoutMode := repoCheckoutModeFor(provider, runtime.GOOS); checkoutMode != "" {
		agentEnv[repoCheckoutModeEnv] = checkoutMode
	}
	if task.AutomationRunID != "" {
		agentEnv["ORVILO_AUTOMATION_RUN_ID"] = task.AutomationRunID
	}
	if task.AutomationID != "" {
		agentEnv["ORVILO_AUTOMATION_ID"] = task.AutomationID
	}
	// Quick-create marker — when set, the patchbay CLI's `issue create`
	// command stamps the new issue with origin_type=quick_create +
	// origin_id=<task_id> so the completion handler can find it
	// deterministically (see GetIssueByOrigin).
	if task.QuickCreatePrompt != "" {
		agentEnv["ORVILO_QUICK_CREATE_TASK_ID"] = task.ID
		if len(task.QuickCreateAttachmentIDs) > 0 {
			if raw, err := json.Marshal(task.QuickCreateAttachmentIDs); err == nil {
				agentEnv["ORVILO_QUICK_CREATE_ATTACHMENT_IDS"] = string(raw)
			} else {
				taskLog.Warn("quick-create attachment ids: marshal failed; skipping env injection", "error", err)
			}
		}
	}
	// Ensure the patchbay CLI is on PATH inside the agent's environment.
	// Some runtimes (e.g. Codex) run in an isolated sandbox that may not
	// inherit the daemon's PATH. Prepend the directory of the running
	// patchbay binary so that `patchbay` commands in the agent always resolve.
	if selfBin, err := resolveSelfExecutable(); err == nil {
		binDir := filepath.Dir(selfBin)
		agentEnv["PATH"] = binDir + string(os.PathListSeparator) + os.Getenv("PATH")
	}
	// Point Codex to the per-task CODEX_HOME so it discovers skills natively
	// without polluting the system ~/.codex/skills/.
	if env.CodexHome != "" {
		agentEnv["CODEX_HOME"] = env.CodexHome
	}
	// HOME and the XDG base dirs are deliberately not touched here: provider
	// tools such as gh, aws, kubectl, and npm continue resolving the daemon
	// user's existing state (MUL-5578). The Patchbay CLI is the exception:
	// ORVILO_TASK_CONFIG_ROOT above redirects its implicit profile lookup to
	// private task-local state and prevents Owner-profile fallback.
	// (Hermes HERMES_HOME is applied after custom_env below so the per-task
	// overlay can win over a user-set HERMES_HOME; see
	// layerCustomEnvAndHermesHome.)
	// Point Cursor at per-task project state when managed MCP is present.
	// The workdir .cursor/mcp.json carries the managed server list, while
	// CURSOR_DATA_DIR isolates the matching project approvals from the user's
	// persistent ~/.cursor/projects state.
	if env.CursorDataDir != "" {
		agentEnv["CURSOR_DATA_DIR"] = env.CursorDataDir
	}
	// Point OpenClaw at the per-task synthesized config. The config pins
	// agents.defaults.workspace (and any agents.list[].workspace) to the
	// task workdir, so the CLI's native skill scanner picks up the per-task
	// skills written under {workDir}/skills/. Falls back silently when the
	// preparer didn't run (non-openclaw provider, or write failure).
	if env.OpenclawConfigPath != "" {
		agentEnv["OPENCLAW_CONFIG_PATH"] = env.OpenclawConfigPath
	}
	// Grant the wrapper config permission to $include the user's active
	// config across directories. OpenClaw's $include defaults to confining
	// resolution to the wrapper's own directory; without this, the
	// wrapper-out-of-envRoot $include into ~/.openclaw/openclaw.json is
	// rejected and the run boots with no user-registered agents.
	if rootsValue, ok := composeOpenclawIncludeRoots(env.OpenclawIncludeRoot, os.Getenv("OPENCLAW_INCLUDE_ROOTS")); ok {
		agentEnv["OPENCLAW_INCLUDE_ROOTS"] = rootsValue
	}
	// Inject user-configured custom environment variables (e.g. ANTHROPIC_API_KEY,
	// ANTHROPIC_BASE_URL for router/proxy mode, or CLAUDE_CODE_USE_BEDROCK for
	// Bedrock). These are set per-agent via the agent settings UI.
	// Critical internal variables are blocklisted to prevent accidental or
	// malicious override of daemon-set values.
	var agentCustomEnv map[string]string
	if task.Agent != nil {
		agentCustomEnv = task.Agent.CustomEnv
	}
	layerCustomEnvAndHermesHome(agentEnv, agentCustomEnv, env.HermesHome, e.logger)
	if provider == "reasonix" {
		reasonixStateHome, err := prepareReasonixTaskStateHome(e.cfg.Profile, task.RuntimeID, task.AgentID)
		if err != nil {
			return TaskResult{}, fmt.Errorf("prepare reasonix state home: %w", err)
		}
		agentEnv["REASONIX_STATE_HOME"] = reasonixStateHome
	}
	if provider == "dsh" {
		dshSessionRoot, err := prepareDshTaskSessionRoot(e.cfg.Profile, task.RuntimeID, task.AgentID)
		if err != nil {
			return TaskResult{}, fmt.Errorf("prepare dsh session root: %w", err)
		}
		agentEnv["ORVILO_DSH_SESSION_ROOT"] = dshSessionRoot
		agentEnv["DSH_TELEMETRY_DISABLED"] = "1"
	}
	if err := configureCodexTaskShellEnvironment(provider, env.CodexHome, os.Environ(), agentEnv, agentCustomEnv, e.logger); err != nil {
		return TaskResult{}, err
	}
	// The overlay is authoritative once built, so nothing on the command line
	// may re-point HERMES_HOME out of it. Both argv regions are stripped
	// together, against the same assembled argv the resolver read: a selection
	// can straddle them (a prefix ending in a bare `-p` captures the backend's
	// `acp`), which per-region stripping cannot see.
	var hermesOverlayCustomArgs []string
	hermesOverlayActive := provider == "hermes" && env != nil && env.HermesHome != ""
	if hermesOverlayActive {
		var rawCustomArgs []string
		if task.Agent != nil {
			rawCustomArgs = task.Agent.CustomArgs
		}
		profileFixedArgs, hermesOverlayCustomArgs = agent.StripHermesProfileSelectors(
			profileFixedArgs, rawCustomArgs, e.logger)
	}
	// Resolve the backend through the unified runtime resolver: built-in
	// runtime identities (e.g. "omp") dispatch through NewRuntime, protocol
	// families go through New. This is the single production boundary — the
	// daemon never calls agent.New or agent.NewRuntime directly, so the two
	// factories stay meaning exactly one thing each.
	backend, err := agent.ResolveBackend(provider, agent.Config{
		ExecutablePath: entry.Path,
		LaunchPrefix:   profileFixedArgs,
		CLIVersion:     resolvedVersion,
		Env:            agentEnv,
		Logger:         e.logger,
		TaskID:         task.ID,
		RuntimeID:      task.RuntimeID,
		DaemonVersion:  e.cfg.CLIVersion,
		CodexVersion:   codexVersion,
		BuiltinRuntime: !usesCustomProfileCommand,
	})
	if err != nil {
		return TaskResult{}, fmt.Errorf("create agent backend: %w", err)
	}

	// Two-tier model resolution: an explicit agent.model wins,
	// then the daemon-wide ORVILO_<PROVIDER>_MODEL env var. If
	// both are empty we deliberately pass "" through — each
	// backend omits `--model` from the CLI invocation, so the
	// provider picks its own default (Claude Code's shipped
	// default, codex app-server's account-scoped default, etc.).
	// Baking a Go-side "recommended default" here is how the
	// cursor regression happened — static guesses drift from
	// whatever the upstream CLI actually accepts.
	//
	// Resolved before the start log rather than at first use: logging
	// entry.Model there reported the env-var tier alone, so every task whose
	// model came from agent.model — the common case — announced itself with an
	// empty model and looked like the selection had been dropped (GH #7300).
	model := ""
	if task.Agent != nil && task.Agent.Model != "" {
		model = task.Agent.Model
	}
	if model == "" {
		model = entry.Model
	}

	taskLog.Info("starting agent",
		"provider", provider,
		"workdir", env.WorkDir,
		"model", model,
		"resume_reachable", resumeReachable,
	)
	if task.PriorSessionID != "" {
		taskLog.Info("resuming session", "session_id", task.PriorSessionID)
	}

	taskStart := time.Now()

	var customArgs []string
	// profileFixedArgs deliberately does NOT go here. It travels as
	// agent.Config.LaunchPrefix instead, because ExtraArgs is honoured by only
	// six of the twenty-one backends and lands *after* the protocol flags in
	// the ones that do — so a wrapper's subcommand was either dropped on the
	// floor or spliced in behind `-p` (GH #7046).
	extraArgs := e.defaultArgs(provider)
	var mcpConfig json.RawMessage
	if task.Agent != nil {
		customArgs = task.Agent.CustomArgs
		mcpConfig = effectiveMcpConfig
	}
	if hermesOverlayActive {
		// Stripped above, alongside the launch prefix. A skill-less hermes task
		// has no overlay to protect and keeps its flags untouched.
		customArgs = hermesOverlayCustomArgs
	}
	thinkingLevel := ""
	serviceTier := ""
	if task.Agent != nil {
		thinkingLevel = task.Agent.ThinkingLevel
		serviceTier = task.Agent.ServiceTier
	}
	selection := resolveTaskModelSelection(ctx, provider, agent.NewCommand(entry.Path, profileFixedArgs),
		taskModelSelection{Model: model, ThinkingLevel: thinkingLevel, ServiceTier: serviceTier}, taskLog)
	model, thinkingLevel, serviceTier = selection.Model, selection.ThinkingLevel, selection.ServiceTier

	var idleWatchdogTimeout time.Duration
	if provider == "opencode" || provider == "codearts" {
		idleWatchdogTimeout = e.cfg.OpenCodeIdleWatchdog
	}
	execOpts := agent.ExecOptions{
		Cwd:                        env.WorkDir,
		Model:                      model,
		ThreadName:                 deriveTaskThreadName(task),
		Timeout:                    e.cfg.AgentTimeout,
		SemanticInactivityTimeout:  e.cfg.CodexSemanticInactivityTimeout,
		FirstTurnNoProgressTimeout: e.cfg.CodexFirstTurnNoProgressTimeout,
		IdleWatchdogTimeout:        idleWatchdogTimeout,
		HandshakeTimeout:           e.cfg.CodexHandshakeTimeout,
		ThreadHandshakeTimeout:     e.cfg.CodexThreadHandshakeTimeout,
		ResumeSessionID:            task.PriorSessionID,
		// Post-gate intent: PriorSessionID here already reflects the pre-flight
		// resume gates (a dropped resume is surfaced via the prompt instead). If it
		// survived to here, the backend must disclose the loss when the live
		// resume still fails — even across the fresh-session retry below, which
		// clears ResumeSessionID but not this (MUL-4424).
		//
		// What that disclosure SAYS, and whether it addresses the user at all,
		// depends on whether this surface's conversation is still readable, which
		// only the daemon knows — hence handing the backend finished text rather
		// than a flag. Empty when the prompt already carries the notice, so a turn
		// can never pay for it twice (MUL-5722).
		ResumeExpected:         task.PriorSessionID != "",
		RequireResume:          strictContinuation,
		ResumeContinuityNotice: backendResumeContinuityNotice(task),
		ExtraArgs:              extraArgs,
		CustomArgs:             customArgs,
		McpConfig:              mcpConfig,
		ThinkingLevel:          thinkingLevel,
		ServiceTier:            serviceTier,
		OpenclawMode:           openclawMode,
		ClaudeSettingsPath:     env.ClaudeSettingsPath,
		QwenpawWorkspace:       env.QwenpawWorkspace,
	}
	// Some providers do not reliably load the per-task runtime config files we
	// write into the task workdir:
	//   - openclaw is pinned to the task workdir via the per-task config we
	//     synthesize (see prepareOpenclawConfig), so AGENTS.md / .agent_context/
	//     in the workdir ARE picked up by the CLI. Inline injection is retained
	//     as a belt-and-suspenders for older openclaw releases until that load
	//     path stabilises in production; remove this once a release tracks the
	//     workdir bootstrap reliably end-to-end.
	//   - kimi is wrapped through its own CLI whose cwd handling is opaque
	//     enough that we can't trust the file-based path either.
	// Pass the full runtime brief inline (CLI catalog + workflow steps + agent
	// identity/persona + skills + project context) so the backend prepends the
	// same payload that file-based runtimes pick up from disk. Without this,
	// these providers silently miss the workflow section and never call
	// `patchbay issue status` / `patchbay issue comment add`, leaving issues
	// stuck in `todo`.
	//
	// Hermes and Kiro are intentionally excluded: their ACP sessions start in
	// the task cwd and load AGENTS.md themselves. Kiro documents root AGENTS.md
	// as always included, and a real kiro-cli 2.13.0 ACP smoke confirms it.
	// Prepending the full runtime brief into the ACP user prompt duplicates that
	// context and bloats every turn.
	if providerNeedsInlineSystemPrompt(provider) {
		execOpts.SystemPrompt = runtimeBrief
	}

	// A quick-actions refresh task from a server that predates server-side
	// generation (MUL-5573). This daemon no longer has a suggestion pass to run
	// it with, and it must NOT fall through to the ordinary chat path below:
	// the task carries no user message, so the agent would answer a prompt
	// nobody wrote and that server would persist the result as a real assistant
	// reply. Complete it empty instead — the same shape the retired pass
	// produced on this task, which that server writes no row for. The user's
	// refresh spinner resolves via the client's own timeout.
	if task.RegenerateQuickActionsFor != "" {
		taskLog.Warn("refusing quick-actions refresh task from an older server; complete the daemon upgrade by updating the server",
			"target_task", shortID(task.RegenerateQuickActionsFor),
		)
		return TaskResult{Status: "completed", Comment: "", WorkDir: env.WorkDir, EnvRoot: env.RootDir}, nil
	}

	// Authenticate the localhost repo-checkout endpoint with the same
	// task-scoped token the child receives. The endpoint derives identity and
	// branch ownership from this in-memory record instead of trusting request
	// fields or ambient process environment. Register only for the provider
	// execution window and always remove the credential afterwards.
	e.registerActiveRepoCheckoutTask(agentToken, activeRepoCheckoutTask{
		WorkspaceID: task.WorkspaceID,
		TaskID:      task.ID,
		AgentID:     task.AgentID,
		AgentName:   task.Agent.Name,
		WorkDir:     env.WorkDir,
	})
	defer e.clearActiveRepoCheckoutTask(agentToken)

	taskLog.Debug("invoking backend",
		"provider", provider,
		"model", model,
		"prompt_bytes", len(prompt),
		"custom_args", len(customArgs),
		"extra_args", len(extraArgs),
		"mcp_config", len(mcpConfig) > 0,
		"inline_system_prompt", execOpts.SystemPrompt != "",
		"resume_session", execOpts.ResumeSessionID != "",
		"timeout", execOpts.Timeout,
		"idle_watchdog", execOpts.IdleWatchdogTimeout,
	)

	// Shared across the resume-retry below so the retry's transcript rows
	// keep ascending seq values for the same task.
	var msgSeq atomic.Int32
	result, tools, err := e.provider.executeAndDrain(ctx, backend, prompt, execOpts, taskLog, task.ID, env.CodexHome, &msgSeq)
	if err != nil {
		return TaskResult{}, err
	}

	// retiredSessionID is the session this run was told to resume and then
	// abandoned. Captured before the retry clears task.PriorSessionID, and
	// reported on EVERY terminal path — the retry succeeding is exactly when
	// the abandoned id would otherwise survive, unreferenced by this task's
	// row but still reachable through an older completed row on the issue or
	// through the chat_session pointer (GH #6066).
	var retiredSessionID string
	defer func() { taskResult.RetiredSessionID = retiredSessionID }()
	// A permanent resume rejection on an explicit Agent continuation cannot be
	// repaired by a fresh thread. Mark the task's provider session unavailable
	// while preserving the original no-fresh-session invariant. Transient busy
	// rejections keep the session pointer for a later retry.
	continuationSessionUnavailable := strictContinuation && result.Status == "failed" &&
		result.ResumeRejected && !result.ResumeRejectedTransient
	if continuationSessionUnavailable {
		taskLog.Warn("agent thread continuation: provider permanently rejected the prior session")
		result.SessionID = ""
	}

	if shouldRetryTaskWithFreshSession(task, result, task.PriorSessionID, tools, provider) {
		firstResult := result
		firstUsage := result.Usage
		firstTools := tools
		if !result.ResumeRejectedTransient {
			retiredSessionID = task.PriorSessionID
		}
		taskLog.Warn("session resume failed, retrying with fresh session", "error", result.Error)

		// Rebuild cold-session context before the single retry. The prior
		// provider transcript is gone (missing, account-mismatched, or —
		// GH #5975 — carrying history the provider now refuses), so the
		// fresh process must NOT be told it is resuming a conversation:
		//   - taskCtx.PriorSessionResumed=false + re-injecting the runtime
		//     brief rewrites the on-disk AGENTS.md so it no longer claims
		//     "You're resuming the prior session" (which file-based backends
		//     like Kiro load themselves).
		//   - clearing task.PriorSessionID rebuilds the prompt on the cold
		//     comment-reading path instead of the warm resumed one.
		//   - PriorSessionResumeUnavailable=true makes BuildPrompt append the
		//     continuity notice for this surface, so the agent knows not to
		//     assume continuity it no longer has. This is now the ONLY injector
		//     on the retry path: the backend's own copy is suppressed below,
		//     because before MUL-5722 both fired and the turn carried the same
		//     paragraph twice.
		// task and taskCtx are local (runTask takes task by value), so these
		// mutations only affect the retry.
		execOpts.ResumeSessionID = ""
		task.PriorSessionID = ""
		task.PriorSessionResumeUnavailable = true
		execOpts.ResumeContinuityNotice = ""
		taskCtx.PriorSessionResumed = false
		if freshBrief, briefErr := execenv.InjectRuntimeConfig(env.WorkDir, provider, taskCtx); briefErr != nil {
			taskLog.Warn("execenv: re-inject cold runtime config for fresh retry failed (non-fatal)", "error", briefErr)
		} else {
			runtimeBrief = freshBrief
			if providerNeedsInlineSystemPrompt(provider) {
				execOpts.SystemPrompt = runtimeBrief
			}
		}
		freshPrompt := BuildPrompt(task, provider, promptOptions...)

		retryResult, retryTools, retryErr := e.provider.executeAndDrain(ctx, backend, freshPrompt, execOpts, taskLog, task.ID, env.CodexHome, &msgSeq)
		if retryErr != nil {
			taskLog.Error("fresh session also failed to start; keeping the original poisoned result", "error", retryErr)
		} else if retryResult.Status != "completed" && retryResult.SessionID == "" {
			taskLog.Warn("fresh session retry also failed without establishing a new session; keeping the original poisoned result",
				"retry_status", retryResult.Status,
				"retry_error", retryResult.Error,
			)
		}
		// The poisoned prior session id lives ONLY on firstResult (classified
		// unrecoverable, so GetLastTaskSession excludes it). reconcile never
		// grafts it onto the retry result: a retry that establishes a new
		// session wins with its own id; a retry that fails without a new
		// session keeps firstResult so the bad session stays excluded rather
		// than being relabeled resumable by a benign-looking second error.
		result, tools = reconcileFreshRetryResult(firstResult, firstUsage, firstTools, retryResult, retryTools, retryErr)
	}

	if provider == "codex" || provider == "claude" || provider == "cursor" {
		result, tools, err = e.provider.recoverCapacity(ctx, backend, result, tools, execOpts, taskLog, task.ID, env.CodexHome, &msgSeq, sleepWithContext)
		if err != nil {
			return TaskResult{}, err
		}
	}

	elapsed := time.Since(taskStart).Round(time.Second)
	taskLog.Info("agent finished",
		"status", result.Status,
		"duration", elapsed.String(),
		"tools", tools,
	)
	taskLog.Debug("agent result detail",
		"status", result.Status,
		"output_bytes", len(result.Output),
		"session_id", result.SessionID,
		"models_with_usage", len(result.Usage),
		"agent_error", result.Error,
	)

	// Convert agent usage map to task usage entries.
	var usageEntries []TaskUsageEntry
	for model, u := range result.Usage {
		if u.InputTokens == 0 && u.OutputTokens == 0 && u.CacheReadTokens == 0 && u.CacheWriteTokens == 0 {
			continue
		}
		usageEntries = append(usageEntries, TaskUsageEntry{
			Provider:         provider,
			Model:            model,
			InputTokens:      u.InputTokens,
			OutputTokens:     u.OutputTokens,
			CacheReadTokens:  u.CacheReadTokens,
			CacheWriteTokens: u.CacheWriteTokens,
			CostUSDTicks:     u.CostUSDTicks,
		})
	}

	// MUL-5305: withhold a Codex session whose rollout never reached the per-issue
	// store, for ANY terminal state — including `completed`, since a completed
	// turn whose rollout is missing is exactly the #5934 case and must not be
	// recorded as a resume pointer the next follow-up would only drop. Blanking
	// the id keeps GetLastTaskSession falling back to the last session whose
	// rollout is real; SessionRolloutMissing tells the server to clear the row's
	// session and record a continuity gap, so the next claim still discloses the
	// loss (PriorSessionResumeUnavailable, MUL-4424 transparency) even while
	// resuming that older good session. No-op for non-Codex providers
	// (env.CodexHome == "") and when there is no session.
	sessionRolloutMissing := continuationSessionUnavailable
	if result.SessionID != "" && !codexSessionResumable(env.CodexHome, result.SessionID, codexRolloutFlushWait) {
		taskLog.Warn("codex session rollout not present in task CODEX_HOME; withholding resume pointer and flagging continuity gap",
			"session_id", result.SessionID, "codex_home", env.CodexHome, "status", result.Status)
		result.SessionID = ""
		sessionRolloutMissing = true
	}
	// Stamp the withhold flag onto whichever TaskResult the status switch below
	// returns (SessionID is already blanked above); reportTaskResult forwards it
	// as session_rollout_missing on the terminal callback (MUL-5305).
	defer func() { taskResult.SessionRolloutMissing = sessionRolloutMissing }()

	switch result.Status {
	case "completed":
		if result.Output == "" {
			// The agent completed successfully but produced no text output.
			// This is valid — the agent may have done all its work via tool
			// calls (e.g. posting comments via CLI, pushing code). Treat as
			// a normal completion so the task is not incorrectly marked as
			// blocked.
			return TaskResult{
				Status:    "completed",
				Comment:   "",
				SessionID: result.SessionID,
				WorkDir:   env.WorkDir,
				EnvRoot:   env.RootDir,
				Usage:     usageEntries,
			}, nil
		}
		// Detect "poisoned" terminal output: the agent didn't reach a real
		// conclusion but emitted a known fallback marker (iteration limit,
		// fallback meta message). Route through the blocked path with a
		// specific failure_reason so the server can exclude this session
		// from the (agent_id, issue_id) resume lookup — otherwise a manual
		// rerun would inherit the same poisoned session and reproduce the
		// same bad output.
		if reason, ok := classifyPoisonedOutput(result.Output); ok {
			taskLog.Warn("agent finished with poisoned fallback output, classifying as blocked",
				"failure_reason", reason,
			)
			return TaskResult{
				Status:        "blocked",
				Comment:       result.Output,
				SessionID:     result.SessionID,
				WorkDir:       env.WorkDir,
				EnvRoot:       env.RootDir,
				Usage:         usageEntries,
				FailureReason: reason,
			}, nil
		}
		taskResult = TaskResult{
			Status:    "completed",
			Comment:   result.Output,
			SessionID: result.SessionID,
			WorkDir:   env.WorkDir,
			EnvRoot:   env.RootDir,
			Usage:     usageEntries,
		}
		return taskResult, nil
	case "timeout":
		// Surface session_id/work_dir so the chat resume pointer is kept
		// in sync even when the agent times out after building a session.
		// We mark as "blocked" (not a hard error return) so handleTask
		// goes through the FailTask path that forwards session info.
		comment := result.Error
		if comment == "" {
			comment = fmt.Sprintf("%s timed out after %s", provider, e.cfg.AgentTimeout)
		}
		failureReason := "timeout"
		if reason, ok := classifyResumeUnsafeTimeout(provider, comment); ok {
			taskLog.Warn("agent timed out with resume-unsafe session, classifying as blocked",
				"failure_reason", reason,
			)
			failureReason = reason
		}
		return TaskResult{
			Status:        "blocked",
			Comment:       comment,
			SessionID:     result.SessionID,
			WorkDir:       env.WorkDir,
			EnvRoot:       env.RootDir,
			FailureReason: failureReason,
			Usage:         usageEntries,
		}, nil
	case "idle_watchdog":
		// The idle watchdog force-stopped the run because the backend
		// went silent (e.g. claude blocked on a tool call against a
		// frozen child process). Route through the blocked path with a
		// dedicated failure_reason so the run leaves "running" state and
		// operators can tell idle-stop apart from a real timeout.
		comment := result.Error
		if comment == "" {
			comment = idleWatchdogReason(e.cfg.AgentIdleWatchdog)
		}
		return TaskResult{
			Status:        "blocked",
			Comment:       comment,
			SessionID:     result.SessionID,
			WorkDir:       env.WorkDir,
			EnvRoot:       env.RootDir,
			FailureReason: "idle_watchdog",
			Usage:         usageEntries,
		}, nil
	case "cancelled":
		// Server cancelled the task (e.g. issue reassignment, user cancel).
		// handleTask's cancelledByPoll branch already discards this result,
		// so this case is mainly defensive — and preserves the "cancelled"
		// status string for the "agent finished" log line so operators can
		// distinguish "task cancelled by server" from a real timeout.
		return TaskResult{
			Status:    "cancelled",
			Comment:   "task cancelled by server",
			SessionID: result.SessionID,
			WorkDir:   env.WorkDir,
			EnvRoot:   env.RootDir,
			Usage:     usageEntries,
		}, nil
	default:
		errMsg := result.Error
		if errMsg == "" {
			errMsg = fmt.Sprintf("%s execution %s", provider, result.Status)
		}
		// Forward SessionID/WorkDir on the blocked path: backends commonly
		// emit a real session_id before failing (rate-limit, tool error,
		// model reject, …). Without this the chat_session resume pointer
		// would either be left stale or overwritten with NULL on the
		// server, causing the next chat turn to lose context.
		//
		// Classify upstream API 400 invalid_request_error failures with a
		// dedicated failure_reason so GetLastTaskSession excludes the
		// task from the (agent_id, issue_id) resume lookup. Without this
		// classifier a corrupt image or oversized payload baked into the
		// conversation permanently blocks the issue: every follow-up
		// task resumes the same poisoned session and hits the same 400.
		failureReason, _ := classifyPoisonedError(errMsg)
		if failureReason == "" {
			// A resume we could not read back leaves the same oversized thread
			// recorded as this issue's resume pointer. Reaching here means the
			// in-turn fresh-session retry did not save the run (it is gated on
			// tools == 0, and can fail on its own), so classify it to keep the
			// NEXT task off that thread rather than replaying the overflow
			// forever (MUL-5722).
			failureReason, _ = classifyResumeUnsafeTransport(provider, errMsg)
			if failureReason != "" && retiredSessionID == "" && task.PriorSessionID != "" {
				// Name the thread explicitly. The failure happens before the
				// turn starts, so the backend has no session id to report and
				// this row lands with session_id NULL — which means neither
				// the reason above nor any error-text filter on this row can
				// identify WHICH session to avoid. retired_session_id is the
				// one channel that does not depend on the failed row carrying
				// the session, and it is what the resume lookups and the chat
				// pointer cleanup both key off.
				//
				// Belt-and-braces, not the live path: an overflowed resume
				// fails before any tool runs, so shouldRetryWithFreshSession's
				// tools == 0 gate is always satisfied and the retry above has
				// already recorded the same id. This covers the case where a
				// future condition stops the retry from firing, so the session
				// is still retired rather than silently kept.
				retiredSessionID = task.PriorSessionID
			}
		}
		if failureReason != "" {
			taskLog.Warn("agent failed with a resume-unsafe error, retiring the session",
				"failure_reason", failureReason,
			)
		} else {
			// MUL-2946: classifyPoisonedError only matches the
			// session-poisoning Anthropic 400 shape. Everything else
			// falls through to taskfailure.Classify, which maps the
			// raw error string to one of the 14 agent_error.*
			// sub-reasons (provider auth, capacity, context overflow,
			// runner crash, …) or to ReasonAgentUnknown. This keeps
			// the failure_reason column in the canonical refined
			// taxonomy at write time instead of waiting on the
			// MUL-1949 offline backfill to re-classify after the
			// fact.
			failureReason = taskfailure.Classify(errMsg).String()
			if result.ProviderErrorCode != "" {
				failureReason = providerFailureReason(result).String()
			}
		}
		// After the classifiers above have read errMsg. The hint is fixed
		// prose chosen to match none of the resume guards (see its const), so
		// ordering is not what makes it safe — but it keeps the machine
		// decisions reading exactly what the runtime reported, and leaves the
		// annotation on the outside where a future edit is visibly a change to
		// human-facing text rather than to classifier input.
		errMsg = annotateHermesProviderUnconfigured(errMsg, provider, env.HermesHome != "")
		return TaskResult{
			Status:        "blocked",
			Comment:       errMsg,
			SessionID:     result.SessionID,
			WorkDir:       env.WorkDir,
			EnvRoot:       env.RootDir,
			Usage:         usageEntries,
			FailureReason: failureReason,
		}, nil
	}
}
