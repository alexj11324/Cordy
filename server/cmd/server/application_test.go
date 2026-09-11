package main

import (
	"context"
	"errors"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"strings"
	"testing"

	"github.com/orvilo-ai/orvilo/server/internal/analytics"
	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/channel"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/channel/engine"
	"github.com/orvilo-ai/orvilo/server/internal/realtime"
	"github.com/orvilo-ai/orvilo/server/internal/scheduler"
	"github.com/redis/go-redis/v9"
)

// The previous production regression constructed a second TaskService for
// background dispatch, omitting the HTTP instance's EmptyClaim invalidation.
// Assert the real entrypoint uses one application for routes and lifecycle,
// and forbid recreating that service in either consumer.
func TestMainUsesApplicationOwnedBackgroundServices(t *testing.T) {
	fset := token.NewFileSet()
	mainFile, err := parser.ParseFile(fset, "main.go", nil, 0)
	if err != nil {
		t.Fatal(err)
	}
	var appName string
	var assemblies int
	ast.Inspect(mainFile, func(node ast.Node) bool {
		assignment, ok := node.(*ast.AssignStmt)
		if !ok || len(assignment.Rhs) != 1 || len(assignment.Lhs) != 1 {
			return true
		}
		call, ok := assignment.Rhs[0].(*ast.CallExpr)
		if !ok || calleeName(call) != "newApplication" {
			return true
		}
		assemblies++
		if name, ok := assignment.Lhs[0].(*ast.Ident); ok {
			appName = name.Name
		}
		return true
	})
	if assemblies != 1 || appName == "" {
		t.Fatalf("production must assemble one application, found %d", assemblies)
	}
	var routes, lifecycle bool
	ast.Inspect(mainFile, func(node ast.Node) bool {
		call, ok := node.(*ast.CallExpr)
		if !ok {
			return true
		}
		if calleeName(call) == "newRouter" && len(call.Args) == 1 {
			argument, ok := call.Args[0].(*ast.Ident)
			routes = ok && argument.Name == appName
		}
		if calleeName(call) == appName+".startBackground" {
			lifecycle = true
		}
		return true
	})
	if !routes || !lifecycle {
		t.Fatal("HTTP and background lifecycle must use the same assembled application")
	}
	for _, path := range []string{"main.go", "router.go", "application_lifecycle.go", "../../internal/handler/handler.go"} {
		file, err := parser.ParseFile(fset, path, nil, 0)
		if err != nil {
			t.Fatal(err)
		}
		ast.Inspect(file, func(node ast.Node) bool {
			call, ok := node.(*ast.CallExpr)
			if !ok {
				return true
			}
			name := calleeName(call)
			if strings.HasPrefix(name, "service.New") ||
				(path == "router.go" && (name == "os.Getenv" || strings.HasPrefix(name, "env"))) {
				t.Errorf("assembly/configuration escaped its owner: %s at %s", name, fset.Position(call.Pos()))
			}
			return true
		})
	}
}

func TestApplicationSharesDispatchAndLivenessCapabilities(t *testing.T) {
	app := newApplication(testPool, realtime.NewHub(), events.New(), analytics.NoopClient{}, nil, RouterOptions{})
	if app.Services.Tasks != app.HTTP.TaskService || app.Workers.Tasks != app.Services.Tasks ||
		app.HTTP.IssueService.TaskService != app.Services.Tasks || app.Workers.Automations != app.HTTP.AutomationService {
		t.Fatal("HTTP issue creation and background dispatch have different business services")
	}
	if app.Workers.Coordination != app.HTTP.AgentCoordination || app.Services.Tasks.Coordination != app.Workers.Coordination {
		t.Fatal("terminal handoff and coordination worker have different outbox capabilities")
	}
	if app.Workers.Liveness != app.HTTP.LivenessStore {
		t.Fatal("heartbeat writes and runtime sweeps have different liveness stores")
	}
}

func TestApplicationMessagingModeGatesAllSixAdapters(t *testing.T) {
	for _, key := range []string{
		"ORVILO_LARK_SECRET_KEY", "ORVILO_SLACK_SECRET_KEY", "ORVILO_DINGTALK_SECRET_KEY",
		"ORVILO_WECOM_SECRET_KEY", "ORVILO_TELEGRAM_SECRET_KEY", "ORVILO_WEIXIN_SECRET_KEY",
	} {
		t.Setenv(key, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
	}
	for _, test := range []struct {
		name, appURL, mode string
		enabled            bool
	}{
		{"hosted", "https://orvilo.aspectlylabs.com", "managed", true},
		{"self hosted", "https://app.example.com", "server_configured", true},
		{"disabled", "https://orvilo.aspectlylabs.com", "disabled", false},
		{"local", "http://localhost:3000", "managed", false},
	} {
		t.Run(test.name, func(t *testing.T) {
			t.Setenv("ORVILO_APP_URL", test.appURL)
			t.Setenv("ORVILO_MESSAGING_MODE", test.mode)
			app := newApplication(testPool, realtime.NewHub(), events.New(), analytics.NoopClient{}, nil, RouterOptions{})
			for _, kind := range []channel.Type{"feishu", "slack", "dingtalk", "wecom", "telegram", "weixin"} {
				// No installation payload: an enabled adapter stops at local validation,
				// while an unwired adapter returns ErrNoResolverSet before dispatch.
				err := app.HTTP.ChannelRouter.Handle(t.Context(), channel.InboundMessage{Source: channel.Source{ChannelType: kind}})
				if got := !errors.Is(err, engine.ErrNoResolverSet); got != test.enabled {
					t.Errorf("%s wired=%v, want %v (error=%v)", kind, got, test.enabled, err)
				}
			}
		})
	}
}

// The dispatch/cache test below exercises the job itself. Also pin its
// production registration, so a working but unregistered job cannot pass CI.
func TestApplicationRegistersScheduledAutomationDispatch(t *testing.T) {
	file, err := parser.ParseFile(token.NewFileSet(), "application_lifecycle.go", nil, 0)
	if err != nil {
		t.Fatal(err)
	}
	var lifecycle *ast.FuncDecl
	for _, declaration := range file.Decls {
		function, ok := declaration.(*ast.FuncDecl)
		if ok && function.Name.Name == "startBackground" {
			lifecycle = function
			break
		}
	}
	if lifecycle == nil || lifecycle.Body == nil {
		t.Fatal("startBackground lifecycle is missing")
	}
	registered := 0
	ast.Inspect(lifecycle.Body, func(node ast.Node) bool {
		call, ok := node.(*ast.CallExpr)
		if !ok || !strings.HasSuffix(calleeName(call), ".Register") || len(call.Args) != 1 {
			return true
		}
		job, ok := call.Args[0].(*ast.CallExpr)
		if ok && calleeName(job) == "scheduler.AutomationScheduleDispatchJob" {
			registered++
		}
		return true
	})
	if registered != 1 {
		t.Fatalf("production registers %d automation dispatch jobs, want 1", registered)
	}
}

func TestApplicationScheduledDispatchInvalidatesHTTPEmptyClaim(t *testing.T) {
	rawURL := os.Getenv("REDIS_TEST_URL")
	if rawURL == "" {
		t.Skip("REDIS_TEST_URL not set")
	}
	options, err := redis.ParseURL(rawURL)
	if err != nil {
		t.Fatal(err)
	}
	rdb := redis.NewClient(options)
	t.Cleanup(func() { rdb.Close() })
	ctx := context.Background()
	if err := rdb.Ping(ctx).Err(); err != nil {
		t.Fatalf("Redis acceptance dependency unavailable: %v", err)
	}
	trigger, _, _ := setupAutomationScheduleJob(t, "*/1 * * * *")
	var runtimeID string
	if err := testPool.QueryRow(ctx, `SELECT a.runtime_id::text FROM agent a
		JOIN automation ap ON ap.executor_id = a.id WHERE ap.id = $1`, trigger.AutomationID).Scan(&runtimeID); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		rdb.Del(context.Background(), "ovy:claim:runtime:empty:"+runtimeID, "ovy:claim:runtime:version:"+runtimeID)
	})
	app := newApplication(testPool, realtime.NewHub(), events.New(), analytics.NoopClient{}, rdb, RouterOptions{})
	if app.Workers.Liveness != app.HTTP.LivenessStore {
		t.Fatal("Redis heartbeat/sweeper stores diverged")
	}
	cache := app.HTTP.TaskService.EmptyClaim
	cache.MarkEmpty(ctx, runtimeID, cache.CurrentVersion(ctx, runtimeID))
	if !cache.IsEmpty(ctx, runtimeID) {
		t.Fatal("failed to establish the idle HTTP claim verdict")
	}
	manager := scheduler.NewManager(testPool, scheduler.Options{RunnerID: "application-cache-acceptance"})
	if err := manager.Register(scheduler.AutomationScheduleDispatchJob(testPool, app.queries, app.Workers.Automations)); err != nil {
		t.Fatal(err)
	}
	if err := manager.RunOnce(ctx); err != nil {
		t.Fatal(err)
	}
	var queuedTasks int
	if err := testPool.QueryRow(ctx, `SELECT count(*) FROM automation_run ar
		JOIN agent_task_queue task ON task.id = ar.task_id WHERE ar.trigger_id = $1`, trigger.ID).Scan(&queuedTasks); err != nil {
		t.Fatal(err)
	}
	if queuedTasks != 1 {
		t.Fatalf("scheduled dispatch persisted %d tasks, want 1", queuedTasks)
	}
	if cache.IsEmpty(ctx, runtimeID) {
		t.Fatal("scheduler queued real work but HTTP still trusts the stale empty-claim verdict")
	}
}

func TestBackgroundCapabilitiesDoNotRetainHTTPContainer(t *testing.T) {
	checks := []struct {
		path, name, forbidden string
	}{
		{"application_lifecycle.go", "applicationWorkers", "Handler"},
		{"../../internal/handler/webhook_delivery_worker.go", "WebhookDeliveryWorker", "Handler"},
		{"../../internal/handler/work_product_discovery.go", "WorkProductDiscoveryRuntime", "Handler"},
		{"../../internal/handler/runtime_events.go", "RuntimeEventPublisher", "Handler"},
		{"../../internal/service/agent_coordination.go", "AgentCoordinationService", "TaskService"},
		{"../../internal/service/task.go", "TaskService", "AgentCoordinationService"},
	}
	for _, check := range checks {
		file, err := parser.ParseFile(token.NewFileSet(), check.path, nil, 0)
		if err != nil {
			t.Fatal(err)
		}
		found := false
		ast.Inspect(file, func(node ast.Node) bool {
			spec, ok := node.(*ast.TypeSpec)
			if !ok || spec.Name.Name != check.name {
				return true
			}
			found = true
			ast.Inspect(spec.Type, func(node ast.Node) bool {
				if name, ok := node.(*ast.Ident); ok && name.Name == check.forbidden {
					t.Errorf("%s retains concrete %s", check.name, check.forbidden)
				}
				return true
			})
			return false
		})
		if !found {
			t.Fatalf("%s moved; point the dependency guard at its actual owner", check.name)
		}
	}
}
