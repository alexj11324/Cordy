package handler

import (
	"log/slog"

	"github.com/orvilo-ai/orvilo/server/internal/analytics"
	"github.com/orvilo-ai/orvilo/server/internal/auth"
	"github.com/orvilo-ai/orvilo/server/internal/daemonws"
	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/ghsnapshot"
	"github.com/orvilo-ai/orvilo/server/internal/realtime"
	"github.com/orvilo-ai/orvilo/server/internal/service"
	"github.com/orvilo-ai/orvilo/server/internal/storage"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/llm"
)

func assembleTestHandler(queries *db.Queries, txStarter txStarter, hub *realtime.Hub, bus *events.Bus, emailService *service.EmailService, store storage.Storage, cfSigner *auth.CloudFrontSigner, analyticsClient analytics.Client, cfg Config, daemonHubs ...*daemonws.Hub) *Handler {
	if analyticsClient == nil {
		analyticsClient = analytics.NoopClient{}
	}
	var daemonHub *daemonws.Hub
	if len(daemonHubs) > 0 {
		daemonHub = daemonHubs[0]
	}
	llmClient := llm.New(llm.Config{
		APIKey:       cfg.LLMAPIKey,
		BaseURL:      cfg.LLMBaseURL,
		DefaultModel: cfg.LLMDefaultModel,
		MaxRetries:   cfg.LLMMaxRetries,
	})

	taskSvc := service.NewTaskService(queries, txStarter, hub, bus, daemonHub)
	taskSvc.Analytics = analyticsClient
	taskSvc.SourceContextStorage = store
	// Chat follow-up suggestions run through the same internal LLM layer that
	// backs auto-titling. A deployment with no ORVILO_LLM_* configuration gets
	// a disabled client, which turns the feature off rather than failing.
	taskSvc.QuickActions = llmClient
	coordinationSvc := service.NewAgentCoordinationService(queries, txStarter, bus, taskSvc.PublishQueuedTask)
	taskSvc.Coordination = coordinationSvc
	h := New(queries, txStarter, hub, bus, emailService, store, cfSigner, analyticsClient, cfg, Services{
		Tasks: taskSvc, Coordination: coordinationSvc,
		Issues:                service.NewIssueService(queries, txStarter, bus, analyticsClient, taskSvc),
		Automations:           service.NewAutomationService(queries, txStarter, bus, taskSvc),
		Plugins:               service.NewPluginService(queries, txStarter),
		ProviderAuthorization: service.NewProviderAuthorizationService(queries), LLM: llmClient,
	}, daemonHubs...)
	h.WebhookDeliveryWorker = NewWebhookDeliveryWorker(h.Queries, h.AutomationService, h.WebhookRateLimiter, h.Metrics)

	ghClient, err := ghsnapshot.NewClientFromEnv()
	if err != nil {
		// Malformed key is operator-actionable; the pipeline stays disabled.
		slog.Warn("github: PR snapshot pipeline disabled (invalid App private key)", "err", err)
	}
	h.PRRefresh = ghsnapshot.NewManager(ghClient, queries, txStarter, NewPRSnapshotEventSink(queries, bus, ghClient != nil && ghClient.Enabled()))
	h.WorkProductDiscovery = NewWorkProductDiscoveryRuntime(queries, executorForTests(txStarter), txStarter, h.PRRefresh, bus)

	return h
}

func executorForTests(tx txStarter) dbExecutor { executor, _ := tx.(dbExecutor); return executor }
