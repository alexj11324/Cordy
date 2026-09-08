package main

import (
	"context"
	"log/slog"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/handler"
	"github.com/orvilo-ai/orvilo/server/internal/hostedcapacity"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/channel/engine"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/ghsnapshot"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/slack"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/telegram"
	obsmetrics "github.com/orvilo-ai/orvilo/server/internal/metrics"
	"github.com/orvilo-ai/orvilo/server/internal/scheduler"
	"github.com/orvilo-ai/orvilo/server/internal/seatcapacity"
	"github.com/orvilo-ai/orvilo/server/internal/service"
)

// Background work is assembled from business capabilities. It never receives
// the HTTP Handler, and all dispatch paths share the same TaskService/caches.
type applicationWorkers struct {
	Tasks          *service.TaskService
	Automations    *service.AutomationService
	Coordination   *service.AgentCoordinationService
	Plugins        *service.PluginService
	Liveness       handler.LivenessStore
	Heartbeats     heartbeatLifecycle
	RuntimeGC      runtimeGCEventPublisher
	Webhooks       *handler.WebhookDeliveryWorker
	SeatCapacity   *seatcapacity.Worker
	HostedCapacity *hostedcapacity.Worker
	SlackTokens    *slack.ManagedTokenWorker
	Linear         *handler.LinearWorker
	Telegram       *telegram.Outbound
	PRRefresh      *ghsnapshot.Manager
	WorkProducts   *handler.WorkProductDiscoveryRuntime
	Channels       *engine.Supervisor
	ChannelRouter  *engine.Router
	ChannelMedia   *service.ChannelMediaReconciler
}

type heartbeatLifecycle interface {
	Run(context.Context)
	Stop()
}

type backgroundLifecycle struct {
	workers          *applicationWorkers
	cancelAutomation context.CancelFunc
	cancelWorkers    context.CancelFunc
}

func (app *application) startBackground(channelMediaMetrics *obsmetrics.ChannelMediaReconcilerMetrics) *backgroundLifecycle {
	workers := &app.Workers
	queries, pool, bus := app.queries, app.pool, app.bus
	// Start background workers.
	sweepCtx, sweepCancel := context.WithCancel(context.Background())
	automationCtx, automationCancel := context.WithCancel(context.Background())
	// Reuse the application's services here. Assembly wires the
	// EmptyClaim cache into TaskService; constructing a second TaskService for
	// scheduled Automation dispatch would send the daemon wakeup without bumping
	// that cache's version, so an idle runtime could keep returning an empty
	// claim until the cache TTL expires.
	taskSvc, automationSvc := workers.Tasks, workers.Automations
	coordinationSvc := workers.Coordination
	coordinationSvc.Start(sweepCtx)
	registerAutomationListeners(bus, automationSvc)

	liveness := workers.Liveness

	// Start background sweeper to mark stale runtimes as offline.
	runtimeReconnectGrace := envDuration("ORVILO_RUNTIME_RECONNECT_GRACE", defaultRuntimeReconnectGrace)
	if runtimeReconnectGrace < minimumRuntimeReconnectGrace {
		slog.Warn("runtime reconnect grace is shorter than heartbeat freshness; clamping",
			"configured", runtimeReconnectGrace,
			"minimum", minimumRuntimeReconnectGrace,
		)
		runtimeReconnectGrace = minimumRuntimeReconnectGrace
	}
	// Queued work now expires on the same runtime-liveness signal as in-flight
	// work, so there is no separate queue TTL to tune: a busy runtime keeps its
	// backlog, and a departed one retires everything it owned at once.
	go runRuntimeSweeper(sweepCtx, queries, liveness, taskSvc, bus, runtimeReconnectGrace)
	// Seven-day runtime retention does not share the 30-second liveness tick:
	// its bounded transactions run independently once per hour, so a slow GC
	// round cannot delay offline detection or task recovery.
	go runRuntimeGCSweeper(sweepCtx, pool, queries, taskSvc.Metrics, workers.RuntimeGC)
	// Source-context cleanup is object-store work, so it gets its own goroutine
	// instead of a slot in the runtime sweep tick.
	go runSourceContextSweeper(sweepCtx, taskSvc)
	if workers.Heartbeats != nil {
		go workers.Heartbeats.Run(sweepCtx)
	}
	go runAutomationFailureMonitor(automationCtx, queries, bus, envFailureMonitorConfig())
	if automationSvc.QuotaEnabled() {
		go runAutomationQuotaReconciler(automationCtx, automationSvc)
	}
	go runDBStatsLogger(sweepCtx, pool)
	if workers.Webhooks != nil {
		go workers.Webhooks.Run(sweepCtx)
	}
	if workers.SeatCapacity != nil {
		go workers.SeatCapacity.Run(sweepCtx)
	}
	// Hosted IM installation capacity sweep: re-aligns durable pause markers
	// with the Cloud policy every interval. Nil (and never started) unless
	// ORVILO_HOSTED_IM_CAPACITY is on.
	if workers.HostedCapacity != nil {
		go workers.HostedCapacity.Run(sweepCtx)
	}
	if workers.SlackTokens != nil {
		go workers.SlackTokens.Run(sweepCtx)
	}
	if workers.Linear != nil {
		go workers.Linear.Run(sweepCtx)
	}
	if workers.Telegram != nil {
		workers.Telegram.Start(sweepCtx)
	}
	// GitHub PR-card API snapshot pipeline (MUL-5265): worker pool + TTL sweeper.
	// No-op when unconfigured (no App private key).
	workers.PRRefresh.Start(sweepCtx)
	// Consume the durable execution-provenance handoff and converge each row to
	// an explicit discovery result. The worker itself also fails closed when the
	// GitHub App is unavailable.
	if workers.WorkProducts != nil {
		workers.WorkProducts.Start(sweepCtx)
	}

	// Channel inbound supervisor (MUL-3620): holds the §4.4 WS lease per
	// installation and drives each channel.Channel. It is channel-agnostic,
	// not Lark-specific, but remains nil when lease startup validation fails
	// (notably Redis fail-closed readiness). With no platform registered or no
	// installation rows it simply idles. Lifecycle is bound to sweepCtx so it winds down
	// alongside the other long-running workers, AFTER the HTTP server has
	// drained.
	if workers.Channels != nil {
		go workers.Channels.Run(sweepCtx)
	}

	// Media intent-ledger reconciler (PR #5580): settles uploaded-but-unbound
	// channel media objects. An independent worker so object-storage latency
	// spikes cannot starve any other sweeper's cadence.
	if workers.ChannelMedia != nil {
		workers.ChannelMedia.Metrics = channelMediaMetrics
		go workers.ChannelMedia.Run(sweepCtx)
	}

	// MUL-2957: DB-backed execution scheduler. The scheduler turns the
	// `sys_cron_executions` table into the distributed lease + audit
	// log for internal periodic jobs. The first job is
	// `rollup_task_usage_hourly`, which replaces the previously
	// operator-registered `pg_cron` entry (still safe to run
	// concurrently — the SQL function holds advisory lock 4246).
	//
	// A failure to register the job is treated as fatal here only at
	// the registration step (a duplicate name is the only realistic
	// cause and indicates a code bug). Once running, the manager
	// surfaces transient errors — DB unreachable, sys_cron_executions
	// missing because of an unusual partial-migration state — by
	// logging them on the tick that fails and retrying on the next
	// cycle, so a temporary outage does not crash the server.
	schedulerMgr := scheduler.NewManager(pool, scheduler.Options{})
	if err := schedulerMgr.Register(scheduler.TaskUsageHourlyJob(pool)); err != nil {
		slog.Warn("scheduler: failed to register task_usage_hourly rollup job", "error", err)
	}
	// MUL-3551: scheduled-Automation dispatch runs on the same DB-backed
	// scheduler. The job owns its plan_times via PlansForScope (each
	// trigger has its own cron expression, so the Cadence planner does
	// not fit). Crash recovery, occurrence-level idempotency, lease
	// theft, and retry are all reused from the manager + sys_cron_executions
	// — there is no separate goroutine for scheduled Automation anymore.
	if err := schedulerMgr.Register(scheduler.AutomationScheduleDispatchJob(pool, queries, automationSvc)); err != nil {
		slog.Warn("scheduler: failed to register automation_schedule_dispatch job", "error", err)
	}
	// Manifest-declared Plugin schedules share the same durable lease and retry
	// machinery. The job is inert while plugins_v1 is disabled.
	if err := schedulerMgr.Register(scheduler.PluginHookScheduleDispatchJob(queries, workers.Plugins)); err != nil {
		slog.Warn("scheduler: failed to register plugin_hook_schedule_dispatch job", "error", err)
	}
	go func() {
		_ = schedulerMgr.Run(sweepCtx)
	}()

	return &backgroundLifecycle{workers: workers, cancelAutomation: automationCancel, cancelWorkers: sweepCancel}
}

func (background *backgroundLifecycle) stopHeartbeats() {
	if background.workers.Heartbeats != nil {
		background.workers.Heartbeats.Stop()
	}
}

func (background *backgroundLifecycle) shutdownSequence() shutdownSequence {
	return shutdownSequence{
		StopAutomation: background.cancelAutomation,
		CancelWorkers: func() {
			background.cancelWorkers()
			if !background.workers.Coordination.WaitWithTimeout(5 * time.Second) {
				slog.Warn("agent coordination worker did not exit within shutdown timeout")
			}
		},
		StopHeartbeats: background.stopHeartbeats,
		JoinSlackTokens: func() {
			if background.workers.SlackTokens != nil && !background.workers.SlackTokens.WaitWithTimeout(5*time.Second) {
				slog.Warn("managed Slack token worker did not exit within shutdown timeout")
			}
		},
		JoinWebhookWorker: func() {
			if background.workers.Webhooks != nil && !background.workers.Webhooks.WaitWithTimeout(5*time.Second) {
				slog.Warn("webhook delivery worker did not exit within shutdown timeout")
			}
		},
		JoinTelegram: func() {
			if background.workers.Telegram != nil && !background.workers.Telegram.WaitWithTimeout(5*time.Second) {
				slog.Warn("telegram outbound workers did not exit within shutdown timeout")
			}
		},
		// Joined so the lease renewer can issue a final release before exit;
		// otherwise the next replica waits out the whole LeaseTTL on the far
		// side of a redeploy. Bounded: a wedged supervisor falls back to the
		// natural expiry rather than holding shutdown open.
		JoinChannelSupervisor: func() {
			if background.workers.Channels == nil {
				return
			}
			if !background.workers.Channels.WaitWithTimeout(background.workers.Channels.ShutdownTimeout()) {
				slog.Warn("channel supervisor: connections did not exit within shutdown timeout; proceeding",
					"timeout", background.workers.Channels.ShutdownTimeout().String(),
				)
			}
		},
		DrainChannelRouter: func() {
			if background.workers.Channels == nil || background.workers.ChannelRouter == nil {
				return
			}
			drainCtx, drainCancel := context.WithTimeout(context.Background(), 10*time.Second)
			if !background.workers.ChannelRouter.Drain(drainCtx) {
				slog.Warn("channel router: drain deadline reached; deferred media fallback remains durable")
			}
			drainCancel()
		},
	}
}
