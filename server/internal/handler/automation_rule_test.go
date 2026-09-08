package handler

import (
	"testing"

	"github.com/jackc/pgx/v5/pgtype"

	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

func TestAutomationRuleSubstantiveChangeIncludesExecutionSettings(t *testing.T) {
	base := db.Automation{
		ExecutorType:  "agent",
		Status:        "active",
		ExecutionMode: "run_only",
		Model:         pgtype.Text{String: "claude-sonnet", Valid: true},
		Tools:         []byte(`{"mcp_server_ids":["one"]}`),
	}
	if automationRuleSubstantiveChange(base, base) {
		t.Fatal("identical automation should not be substantive")
	}
	modelChanged := base
	modelChanged.Model = pgtype.Text{String: "claude-opus", Valid: true}
	if !automationRuleSubstantiveChange(base, modelChanged) {
		t.Fatal("model change must republish the automation rule")
	}
	toolsChanged := base
	toolsChanged.Tools = []byte(`{"mcp_server_ids":["two"]}`)
	if !automationRuleSubstantiveChange(base, toolsChanged) {
		t.Fatal("tools change must republish the automation rule")
	}
}
