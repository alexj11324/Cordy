package daemon

import (
	"context"
	"errors"
	"go/ast"
	"go/parser"
	"go/token"
	"log/slog"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/orvilo-ai/orvilo/server/pkg/agent"
)

var _ interface {
	run(context.Context, taskExecutionInput) (TaskResult, error)
} = (*taskExecution)(nil)
var _ providerExecutionClient = (*stageTranscriptClient)(nil)

// The deadline and lease cleanup can be tested with only preparation
// capabilities. No daemon, runtime registry, HTTP client or provider is needed.
func TestTaskExecutionStagePreparationDeadlineReleasesItsCapabilities(t *testing.T) {
	task := Task{ID: "prepared-task", WorkspaceID: "workspace", AgentID: "agent", Agent: &AgentData{ID: "agent", Name: "Agent"}}
	var registered, cleared, leaseClosed int
	stage := &taskExecution{
		logger:                      slog.Default(),
		effectiveTaskPrepareTimeout: func() time.Duration { return -time.Nanosecond },
		registerTaskRepos: func(workspaceID, taskID string, _ []RepoData) {
			if workspaceID != task.WorkspaceID || taskID != task.ID {
				t.Fatalf("wrong task preparation scope: %s/%s", workspaceID, taskID)
			}
			registered++
		},
		clearTaskRepoRefs: func(workspaceID, taskID string) {
			if workspaceID != task.WorkspaceID || taskID != task.ID {
				t.Fatalf("wrong cleanup scope: %s/%s", workspaceID, taskID)
			}
			cleared++
		},
		agentEntry: func(provider string) (AgentEntry, bool) {
			if provider != "claude" {
				t.Fatalf("provider=%s", provider)
			}
			return AgentEntry{Path: "test-only-unused-executable"}, true
		},
		customProfileLaunchForRuntime: func(string) (profileLaunchSpec, bool) { return profileLaunchSpec{}, false },
		resolveAgentEntryForLaunch: func(_ context.Context, _ string, entry AgentEntry) (AgentEntry, string, error) {
			return entry, "test", nil
		},
		startTaskPrepareLeaseExtender: func(_ context.Context, _ Task, _ *slog.Logger) func() { return func() { leaseClosed++ } },
		ensureTaskSkillBundles:        func(ctx context.Context, _ *Task) error { <-ctx.Done(); return ctx.Err() },
	}
	result, err := stage.run(context.Background(), taskExecutionInput{Task: task, Provider: "claude", Slot: 3, Log: slog.Default()})
	if !errors.Is(err, errTaskPrepareTimeout) {
		t.Fatalf("preparation error=%v", err)
	}
	if result.Status != "" || result.Comment != "" || result.WorkDir != "" {
		t.Fatalf("failed preparation leaked an execution result: %+v", result)
	}
	if registered != 1 || cleared != 1 || leaseClosed != 1 {
		t.Fatalf("capability lifecycle registered=%d cleared=%d leaseClosed=%d", registered, cleared, leaseClosed)
	}
}

type stageTranscriptClient struct {
	counter      *atomic.Int64
	mu           sync.Mutex
	messages     []TaskMessageData
	taskIDs      []string
	activeCounts []int64
}

func (c *stageTranscriptClient) ReportTaskMessages(_ context.Context, taskID string, messages []TaskMessageData) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.messages = append(c.messages, messages...)
	c.taskIDs = append(c.taskIDs, taskID)
	c.activeCounts = append(c.activeCounts, c.counter.Load())
	return nil
}
func (c *stageTranscriptClient) PinTaskSession(context.Context, string, string, string) error {
	return nil
}

// The two-method transport is intentionally too small to implement a daemon
// client. The shared counter is observable during delivery and released on return.
func TestProviderExecutionStageUsesSharedCounterAndScopedTransport(t *testing.T) {
	var active atomic.Int64
	active.Store(4)
	transport := &stageTranscriptClient{counter: &active}
	stage := &providerExecution{client: transport, runningTasks: &active}
	var sequence atomic.Int32
	sequence.Store(10)
	result, tools, err := stage.executeAndDrain(context.Background(), &transcriptBackend{}, "original task", agent.ExecOptions{}, slog.Default(), "task-stage", "", &sequence)
	if err != nil {
		t.Fatal(err)
	}
	if result.Status != "failed" || tools != 1 {
		t.Fatalf("provider result=%+v tools=%d", result, tools)
	}
	if active.Load() != 4 {
		t.Fatalf("shared provider count after return=%d", active.Load())
	}
	transport.mu.Lock()
	defer transport.mu.Unlock()
	if len(transport.messages) != 2 || sequence.Load() != 12 {
		t.Fatalf("transcript=%+v sequence=%d", transport.messages, sequence.Load())
	}
	for i, taskID := range transport.taskIDs {
		if taskID != "task-stage" || transport.activeCounts[i] != 5 {
			t.Fatalf("delivery scope/count=%s/%d", taskID, transport.activeCounts[i])
		}
	}
}

func TestExecutionStagesCannotDependOnDaemonOrFullClient(t *testing.T) {
	for _, path := range []string{"task_execution.go", "provider_execution.go", "capacity_recovery.go"} {
		file, err := parser.ParseFile(token.NewFileSet(), path, nil, 0)
		if err != nil {
			t.Fatal(err)
		}
		providerConfigNames := make(map[*ast.Ident]bool)
		ast.Inspect(file, func(node ast.Node) bool {
			if selector, ok := node.(*ast.SelectorExpr); ok {
				if pkg, ok := selector.X.(*ast.Ident); ok && pkg.Name == "agent" && selector.Sel.Name == "Config" {
					providerConfigNames[selector.Sel] = true
				}
			}
			return true
		})
		ast.Inspect(file, func(node ast.Node) bool {
			switch node := node.(type) {
			case *ast.Ident:
				if !providerConfigNames[node] && node.Obj == nil && (node.Name == "Daemon" || node.Name == "Client" || node.Name == "Config" || node.Name == "any") {
					t.Errorf("%s widens the execution boundary through %s", path, node.Name)
				}
			case *ast.InterfaceType:
				if len(node.Methods.List) == 0 {
					t.Errorf("%s introduces an unrestricted interface", path)
				}
			}
			return true
		})
	}
}
