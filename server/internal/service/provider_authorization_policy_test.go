package service

import (
	"testing"

	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestProviderGrantConditionsFailClosedForUnknownFields(t *testing.T) {
	t.Parallel()
	input := ProviderLeaseValidation{
		TaskID: testUUID(1), RuntimeID: testUUID(2), Provider: "claude", Model: "model-a",
		RequestedMaxTokens: 100,
	}
	grant := db.AuthorizationGrant{Conditions: []byte(`{
		"provider":"claude","provider_action":"provider.invoke","device_id":"` + uuidText(input.RuntimeID) + `",
		"models":["model-a"],"future_condition":true
	}`)}
	if _, ok := grantConditionsMatch(grant, input, false); ok {
		t.Fatal("unknown provider condition widened a grant")
	}
	grant.Conditions = []byte(`{"provider":"claude","provider_action":"provider.invoke","device_id":"` + uuidText(input.RuntimeID) + `","models":["model-a"],"max_tokens":50}`)
	if _, ok := grantConditionsMatch(grant, input, false); ok {
		t.Fatal("request above max_tokens was accepted")
	}
}

func TestProviderGrantPrincipalTaskRunIsExactAndLineageBound(t *testing.T) {
	t.Parallel()
	taskID, parentID, otherID := testUUID(3), testUUID(4), testUUID(5)
	grant := db.AuthorizationGrant{PrincipalType: "task_run", PrincipalID: taskID}
	lineage := map[pgtype.UUID]struct{}{taskID: {}, parentID: {}}
	if !grantPrincipalMatches(grant, testUUID(6), testUUID(7), testUUID(8), lineage, nil) {
		t.Fatal("current task principal did not match")
	}
	grant.PrincipalID = otherID
	if grantPrincipalMatches(grant, testUUID(6), testUUID(7), testUUID(8), lineage, nil) {
		t.Fatal("unrelated task principal matched")
	}
}

func TestProviderGrantConditionsRequireExactTaskForDelegatedAllow(t *testing.T) {
	t.Parallel()
	input := ProviderLeaseValidation{TaskID: testUUID(9), RuntimeID: testUUID(10), Provider: "codex", Model: "model-a"}
	grant := db.AuthorizationGrant{Effect: "allow", Conditions: []byte(`{"provider":"codex","provider_action":"provider.invoke","device_id":"` + uuidText(input.RuntimeID) + `"}`)}
	if _, ok := grantConditionsMatch(grant, input, true); ok {
		t.Fatal("delegated allow without exact task condition matched")
	}
	grant.Conditions = []byte(`{"provider":"codex","provider_action":"provider.invoke","device_id":"` + uuidText(input.RuntimeID) + `","task_id":"` + uuidText(input.TaskID) + `"}`)
	if _, ok := grantConditionsMatch(grant, input, true); !ok {
		t.Fatal("delegated allow with exact task condition was rejected")
	}
}
