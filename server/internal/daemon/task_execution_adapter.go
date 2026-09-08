package daemon

import (
	"context"
	"log/slog"
	"sync/atomic"
	"time"

	"github.com/orvilo-ai/orvilo/server/pkg/agent"
)

var _ taskExecutionClient = (*Client)(nil)
var _ providerExecutionClient = (*Client)(nil)
var _ localPathAcquirer = (*LocalPathLocker)(nil)

// Bind capabilities for each invocation, preserving live runtime/profile lookup
// and shared ownership. No mutex, registry map or atomic value is copied.
func (d *Daemon) newTaskExecution() *taskExecution {
	return &taskExecution{
		cfg: taskExecutionConfig{
			AgentIdleWatchdog: d.cfg.AgentIdleWatchdog, AgentTimeout: d.cfg.AgentTimeout,
			CLIVersion: d.cfg.CLIVersion, CodexFirstTurnNoProgressTimeout: d.cfg.CodexFirstTurnNoProgressTimeout,
			CodexHandshakeTimeout: d.cfg.CodexHandshakeTimeout, CodexSemanticInactivityTimeout: d.cfg.CodexSemanticInactivityTimeout,
			CodexThreadHandshakeTimeout: d.cfg.CodexThreadHandshakeTimeout, DaemonID: d.cfg.DaemonID,
			HealthPort: d.cfg.HealthPort, OpenCodeIdleWatchdog: d.cfg.OpenCodeIdleWatchdog,
			Profile: d.cfg.Profile, ServerBaseURL: d.cfg.ServerBaseURL, WorkspacesRoot: d.cfg.WorkspacesRoot,
		},
		client: d.client, logger: d.logger, provider: d.newProviderExecution(),
		agentEntry:                    func(provider string) (AgentEntry, bool) { entry, ok := d.agents()[provider]; return entry, ok },
		agentVersion:                  d.agentVersion,
		customProfileLaunchForRuntime: d.customProfileLaunchForRuntime,
		resolveAgentEntryForLaunch:    d.resolveAgentEntryForLaunch,
		defaultArgs:                   func(provider string) []string { return defaultArgsForProvider(d.cfg, provider) },
		registerTaskRepos:             d.registerTaskRepos, clearTaskRepoRefs: d.clearTaskRepoRefs,
		startTaskPrepareLeaseExtender: d.startTaskPrepareLeaseExtender,
		ensureTaskSkillBundles:        d.ensureTaskSkillBundles,
		markActiveEnvRoot:             d.markActiveEnvRoot, unmarkActiveEnvRoot: d.unmarkActiveEnvRoot,
		markActiveStore: d.markActiveStore, unmarkActiveStore: d.unmarkActiveStore,
		lockReusablePriorEnvRoot:    d.lockReusablePriorEnvRoot,
		prepareExecutionEnvironment: d.prepareExecutionEnvironment, reuseExecutionEnvironment: d.reuseExecutionEnvironment,
		effectiveTaskPrepareTimeout: d.effectiveTaskPrepareTimeout,
		watchTaskCancellation:       d.watchTaskCancellation, cancelPollInterval: d.cancelPollInterval,
		registerActiveRepoCheckoutTask: d.registerActiveRepoCheckoutTask,
		clearActiveRepoCheckoutTask:    d.clearActiveRepoCheckoutTask,
		localPathLocks:                 d.localPathLocks, resourceWaitTasks: &d.resourceWaitTasks,
	}
}

func (d *Daemon) newProviderExecution() *providerExecution {
	return &providerExecution{client: d.client, idleWatchdog: d.cfg.AgentIdleWatchdog, toolWatchdog: d.cfg.AgentToolWatchdog, runningTasks: &d.runningTasks}
}

// runTask is the supervision adapter. Execution returns a result; supervision
// retains cancellation, terminal delivery and the ownership of the task slot.
func (d *Daemon) runTask(ctx context.Context, task Task, provider string, slot int, taskLog *slog.Logger) (TaskResult, error) {
	return d.newTaskExecution().run(ctx, taskExecutionInput{Task: task, Provider: provider, Slot: slot, Log: taskLog})
}

func (d *Daemon) executeAndDrain(ctx context.Context, backend agent.Backend, prompt string, opts agent.ExecOptions, taskLog *slog.Logger, taskID, codexHome string, seq *atomic.Int32) (agent.Result, int32, error) {
	return d.newProviderExecution().executeAndDrain(ctx, backend, prompt, opts, taskLog, taskID, codexHome, seq)
}

func (d *Daemon) recoverCapacity(ctx context.Context, backend agent.Backend, result agent.Result, tools int32, opts agent.ExecOptions, logger *slog.Logger, taskID, codexHome string, seq *atomic.Int32, wait func(context.Context, time.Duration) error) (agent.Result, int32, error) {
	return d.newProviderExecution().recoverCapacity(ctx, backend, result, tools, opts, logger, taskID, codexHome, seq, wait)
}

func (d *Daemon) runIdleWatchdog(ctx context.Context, window, toolWindow time.Duration, lastActivityAt *atomic.Int64, inFlightTools *atomic.Int32, fired *atomic.Bool, firedThreshold *atomic.Int64, cancel context.CancelFunc, messages <-chan agent.Message, log *slog.Logger, taskID string) {
	d.newProviderExecution().runIdleWatchdog(ctx, window, toolWindow, lastActivityAt, inFlightTools, fired, firedThreshold, cancel, messages, log, taskID)
}
