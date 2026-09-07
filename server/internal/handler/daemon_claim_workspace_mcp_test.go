package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

// claimAgentMcpConfigForTest runs one claim for the given runtime and returns
// the mcp_config the daemon would receive for the claimed task's agent.
func claimAgentMcpConfigForTest(t *testing.T, runtimeID string) json.RawMessage {
	t.Helper()

	w := httptest.NewRecorder()
	req := newDaemonTokenRequest("POST", "/api/daemon/runtimes/"+runtimeID+"/tasks/claim", nil, testWorkspaceID, "ws-mcp-daemon")
	req = withURLParam(req, "runtimeId", runtimeID)
	testHandler.ClaimTaskByRuntime(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("ClaimTaskByRuntime: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var claimResp struct {
		Task *AgentTaskResponse `json:"task"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &claimResp); err != nil {
		t.Fatalf("decode claim: %v", err)
	}
	if claimResp.Task == nil || claimResp.Task.Agent == nil {
		t.Fatalf("missing task agent in claim response: %s", w.Body.String())
	}
	return claimResp.Task.Agent.McpConfig
}

// setupWorkspaceMcpClaimFixture creates a runtime, an agent with the given
// saved mcp_config, one queued task ready to claim, and binds the named
// workspace MCP servers to that agent.
func setupWorkspaceMcpClaimFixture(t *testing.T, ctx context.Context, name, agentMcpConfig string, bind ...string) string {
	t.Helper()

	runtimeID := createClaimReclaimRuntime(t, ctx, name+" runtime")
	agentID, issueID := createClaimReclaimAgentAndIssue(t, ctx, runtimeID, name+" agent")
	for _, serverID := range bind {
		if _, err := testPool.Exec(ctx,
			`INSERT INTO agent_mcp_server (agent_id, server_id) VALUES ($1, $2)`, agentID, serverID); err != nil {
			t.Fatalf("setup: bind mcp server: %v", err)
		}
	}

	var stored []byte
	if agentMcpConfig != "" {
		stored = []byte(agentMcpConfig)
	}
	if _, err := testPool.Exec(ctx, `UPDATE agent SET mcp_config = $1 WHERE id = $2`, stored, agentID); err != nil {
		t.Fatalf("setup: set agent mcp_config: %v", err)
	}

	var taskID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO agent_task_queue (
			agent_id, runtime_id, issue_id, status, priority,
			originator_user_id, accountable_user_id, originator_source
		)
		VALUES ($1, $2, $3, 'queued', 0, $4, $4, 'direct_human')
		RETURNING id
	`, agentID, runtimeID, issueID, testUserID).Scan(&taskID); err != nil {
		t.Fatalf("setup: create task: %v", err)
	}
	t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM agent_task_queue WHERE id = $1`, taskID) })

	return runtimeID
}

// The claim payload is where a workspace library server actually reaches an
// agent, so assert the resolved document on the wire — not just the resolver.
func TestClaimTaskByRuntime_CarriesBoundWorkspaceMcpServers(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	serverID := createWorkspaceMcpServerForTest(t, "shared", `{"url":"https://shared.example"}`)
	runtimeID := setupWorkspaceMcpClaimFixture(t, ctx, "ws-mcp-bound", "", serverID)

	servers := decodeServers(t, claimAgentMcpConfigForTest(t, runtimeID))
	if len(servers) != 1 || servers["shared"] == nil {
		t.Fatalf("agent should run the workspace library server, got %v", serverNames(servers))
	}
}

// A library entry applies to every agent in the workspace, even with no
// agent_mcp_server binding.
func TestClaimTaskByRuntime_UnboundWorkspaceMcpServerIsCarried(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	createWorkspaceMcpServerForTest(t, "shared", `{"url":"https://shared.example"}`)
	runtimeID := setupWorkspaceMcpClaimFixture(t, ctx, "ws-mcp-unbound", "")

	servers := decodeServers(t, claimAgentMcpConfigForTest(t, runtimeID))
	if len(servers) != 1 || servers["shared"] == nil {
		t.Fatalf("unassigned workspace library server must reach the agent, got %v", serverNames(servers))
	}
}

// Two agents in the same workspace both receive the library server with no
// bindings of their own.
func TestClaimTaskByRuntime_UnboundWorkspaceMcpServerReachesEveryAgent(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	createWorkspaceMcpServerForTest(t, "shared-all", `{"url":"https://shared-all.example"}`)
	runtimeA := setupWorkspaceMcpClaimFixture(t, ctx, "ws-mcp-unbound-a", "")
	runtimeB := setupWorkspaceMcpClaimFixture(t, ctx, "ws-mcp-unbound-b", "")

	for _, runtimeID := range []string{runtimeA, runtimeB} {
		servers := decodeServers(t, claimAgentMcpConfigForTest(t, runtimeID))
		if servers["shared-all"] == nil {
			t.Fatalf("runtime %s missing shared library server, got %v", runtimeID, serverNames(servers))
		}
	}
}

// A disabled junction row is ignored; the library entry is still carried.
func TestClaimTaskByRuntime_DisabledBindingIsNotCarried(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	serverID := createWorkspaceMcpServerForTest(t, "shared", `{"url":"https://shared.example"}`)
	runtimeID := setupWorkspaceMcpClaimFixture(t, ctx, "ws-mcp-disabled", "", serverID)
	if _, err := testPool.Exec(ctx,
		`UPDATE agent_mcp_server SET enabled = FALSE WHERE server_id = $1`, serverID); err != nil {
		t.Fatalf("disable binding: %v", err)
	}

	servers := decodeServers(t, claimAgentMcpConfigForTest(t, runtimeID))
	if len(servers) != 1 || servers["shared"] == nil {
		t.Fatalf("disabled junction must not hide the library server, got %v", serverNames(servers))
	}
}

func TestClaimTaskByRuntime_MergesBoundAndAgentOwnMcpServers(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	shared := createWorkspaceMcpServerForTest(t, "shared", `{"url":"https://shared.example"}`)
	linear := createWorkspaceMcpServerForTest(t, "linear", `{"url":"https://ws-linear.example"}`)
	runtimeID := setupWorkspaceMcpClaimFixture(t, ctx, "ws-mcp-merge",
		`{"mcpServers":{"private":{"url":"https://private.example"},"linear":{"url":"https://agent-linear.example"}}}`,
		shared, linear)

	servers := decodeServers(t, claimAgentMcpConfigForTest(t, runtimeID))
	if len(servers) != 2 || servers["shared"] == nil || servers["linear"] == nil {
		t.Fatalf("server set = %v, want shared/linear from the workspace library", serverNames(servers))
	}
	if servers["private"] != nil {
		t.Fatalf("agent-own private server must not appear on claim, got %v", serverNames(servers))
	}
	entry, _ := servers["linear"].(map[string]any)
	if entry["url"] != "https://ws-linear.example" {
		t.Errorf("workspace library linear URL must win; leftover agent mcp_config must not, got %v", entry["url"])
	}
}
