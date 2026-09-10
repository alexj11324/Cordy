package handler

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/ghsnapshot"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

type failingDiscoveryLookup struct {
	err   error
	calls int
}

func (*failingDiscoveryLookup) Enabled() bool { return true }

func (f *failingDiscoveryLookup) PullRequestsByHead(context.Context, int64, string, string, string) ([]ghsnapshot.PullRequestHeadMatch, error) {
	f.calls++
	return nil, f.err
}

func TestWorkProductDiscoveryProviderFailureKeepsLeaseRetryable(t *testing.T) {
	for _, tc := range []struct {
		name string
		err  error
	}{
		{"network", errors.New("temporary provider connection failure")},
		{"rate limit", &ghsnapshot.RateLimitError{RetryAfter: time.Second}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			ctx := context.Background()
			const repo = "acme/retry-discovery"
			const workDir = "/srv/retry-discovery"
			workspaceID := dbfx.Workspace(t, "Discovery retry", "discovery-retry-"+uuid.NewString(), testutil.Cols{
				"repos": testutil.Raw(`'[{"url":"https://github.com/acme/retry-discovery"}]'::jsonb`),
			})
			agentID := dbfx.Agent(t, "Discovery retry agent", testRuntimeID, testutil.Cols{"workspace_id": workspaceID})
			taskID := dbfx.Task(t, agentID, testutil.Cols{
				"status":       "completed",
				"completed_at": testutil.Raw("now()"),
				"work_dir":     workDir,
			})
			dbfx.Insert(t, "github_installation", testutil.Cols{
				"workspace_id": workspaceID, "installation_id": time.Now().UnixNano(),
				"account_login": "acme", "account_type": "Organization",
			})
			dbfx.Exec(t, `INSERT INTO agent_task_execution_provenance (
				workspace_id, task_id, repo_identity, execution_workspace,
				head_branch, head_sha, head_state, finished_at, discovery_status
			) VALUES ($1, $2, $3, $4, 'agent/retry', $5, 'attached', now(), 'pending')`,
				workspaceID, taskID, repo, workDir, strings.Repeat("a", 40))
			dbfx.Cleanup(t, `DELETE FROM agent_task_execution_provenance WHERE workspace_id = $1`, workspaceID)

			task, err := testHandler.Queries.GetAgentTaskInWorkspace(ctx, db.GetAgentTaskInWorkspaceParams{
				ID: parseUUID(taskID), WorkspaceID: parseUUID(workspaceID),
			})
			if err != nil {
				t.Fatal(err)
			}
			lookup := &failingDiscoveryLookup{err: tc.err}
			runtime := NewWorkProductDiscoveryRuntime(testHandler.Queries, testPool, testPool, nil, nil)
			runtime.prRefresh = lookup
			item := db.AgentTaskExecutionProvenance{
				WorkspaceID: parseUUID(workspaceID), TaskID: parseUUID(taskID),
				RepoIdentity: repo, ExecutionWorkspace: workDir,
			}
			claimed, ok, err := runtime.claim(ctx, item)
			if err != nil || !ok {
				t.Fatalf("claim = %v, %v", ok, err)
			}
			if err := runtime.discoverOne(ctx, task, claimed); !errors.Is(err, tc.err) {
				t.Fatalf("provider failure = %v, want %v", err, tc.err)
			}
			var status string
			if err := testPool.QueryRow(ctx, `SELECT discovery_status FROM agent_task_execution_provenance WHERE task_id = $1`, taskID).Scan(&status); err != nil {
				t.Fatal(err)
			}
			if status != "in_progress" {
				t.Fatalf("provider failure finalized discovery as %q", status)
			}

			// Advance the lease in the database instead of sleeping for five minutes.
			dbfx.Exec(t, `UPDATE agent_task_execution_provenance SET updated_at = now() - interval '6 minutes' WHERE task_id = $1`, taskID)
			pending, err := testHandler.Queries.ListPendingExecutionDiscoveryTasks(ctx, 100)
			if err != nil {
				t.Fatal(err)
			}
			found := false
			for _, row := range pending {
				found = found || uuidToString(row.TaskID) == taskID
			}
			if !found {
				t.Fatal("expired provider-failure lease is absent from the retry queue")
			}
			lookup.err = nil
			reclaimed, ok, err := runtime.claim(ctx, item)
			if err != nil || !ok {
				t.Fatalf("reclaim = %v, %v", ok, err)
			}
			if err := runtime.discoverOne(ctx, task, reclaimed); err != nil {
				t.Fatal(err)
			}
			if err := testPool.QueryRow(ctx, `SELECT discovery_status FROM agent_task_execution_provenance WHERE task_id = $1`, taskID).Scan(&status); err != nil {
				t.Fatal(err)
			}
			if status != "unassociated" || lookup.calls != 2 {
				t.Fatalf("recovered discovery = %q, calls = %d", status, lookup.calls)
			}
		})
	}
}
