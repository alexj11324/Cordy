package handler

import (
	"context"
	"net/http"
	"testing"

	"github.com/orvilo-ai/orvilo/server/internal/testutil"
)

func TestWorkspaceLeadLifecycleAndArchiveCleanup(t *testing.T) {
	if testHandler == nil {
		t.Skip("database not available")
	}

	ctx := context.Background()
	agentID := dbfx.Agent(t, "Workspace lead test agent", testRuntimeID)
	dbfx.Cleanup(t, `UPDATE workspace SET lead_agent_id = NULL WHERE id = $1`, testWorkspaceID)

	setLead := newRequest(http.MethodPatch, "/api/workspaces/"+testWorkspaceID, map[string]any{
		"lead_agent_id": agentID,
	})
	setLead = withURLParam(setLead, "id", testWorkspaceID)
	w := testutil.Call(t, testHandler.UpdateWorkspace, setLead).Want(http.StatusOK)
	var response WorkspaceResponse
	w.JSON(&response)
	if response.LeadAgentID == nil || *response.LeadAgentID != agentID {
		t.Fatalf("lead_agent_id response = %v, want %s", response.LeadAgentID, agentID)
	}

	var storedLead string
	if err := testPool.QueryRow(ctx, `SELECT lead_agent_id::text FROM workspace WHERE id = $1`, testWorkspaceID).Scan(&storedLead); err != nil {
		t.Fatalf("read lead_agent_id: %v", err)
	}
	if storedLead != agentID {
		t.Fatalf("stored lead_agent_id = %s, want %s", storedLead, agentID)
	}

	clearLead := newRequest(http.MethodPatch, "/api/workspaces/"+testWorkspaceID, map[string]any{
		"lead_agent_id": nil,
	})
	clearLead = withURLParam(clearLead, "id", testWorkspaceID)
	cleared := testutil.Call(t, testHandler.UpdateWorkspace, clearLead).Want(http.StatusOK)
	var clearedResponse WorkspaceResponse
	cleared.JSON(&clearedResponse)
	if clearedResponse.LeadAgentID != nil {
		t.Fatalf("cleared lead response = %v, want null", clearedResponse.LeadAgentID)
	}

	setLeadAgain := newRequest(http.MethodPatch, "/api/workspaces/"+testWorkspaceID, map[string]any{
		"lead_agent_id": agentID,
	})
	setLeadAgain = withURLParam(setLeadAgain, "id", testWorkspaceID)
	testutil.Call(t, testHandler.UpdateWorkspace, setLeadAgain).Want(http.StatusOK)

	archive := newRequest(http.MethodPost, "/api/agents/"+agentID+"/archive", nil)
	archive = withURLParam(archive, "id", agentID)
	testutil.Call(t, testHandler.ArchiveAgent, archive).Want(http.StatusOK)

	var clearedByArchive *string
	if err := testPool.QueryRow(ctx, `SELECT lead_agent_id::text FROM workspace WHERE id = $1`, testWorkspaceID).Scan(&clearedByArchive); err != nil {
		t.Fatalf("read archived lead_agent_id: %v", err)
	}
	if clearedByArchive != nil {
		t.Fatalf("lead_agent_id after archive = %s, want null", *clearedByArchive)
	}
}
