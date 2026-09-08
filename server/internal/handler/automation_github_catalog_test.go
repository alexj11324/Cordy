package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

func TestAutomationGitHubCatalogScopesReposAndMeToWriter(t *testing.T) {
	agentID := createWebhookTestAgent(t, "GitHub catalog agent")
	automationID := createWebhookTestAutomation(t, agentID, "active", "run_only")
	pemBytes, _ := generateTestRSAKeyPEM(t)
	t.Setenv("GITHUB_APP_ID", "424242")
	t.Setenv("GITHUB_APP_PRIVATE_KEY", string(pemBytes))
	const installationID int64 = 919191
	row, err := testHandler.Queries.CreateGitHubInstallation(context.Background(), db.CreateGitHubInstallationParams{
		WorkspaceID: parseUUID(testWorkspaceID), InstallationID: installationID,
		AccountLogin: "current-octocat", AccountType: "User", ConnectedByID: parseUUID(testUserID),
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_, _ = testPool.Exec(context.Background(), `DELETE FROM github_installation WHERE id=$1`, row.ID)
	})

	provider := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, request *http.Request) {
		switch {
		case request.Method == http.MethodPost:
			writeJSON(w, http.StatusCreated, map[string]any{"token": "installation-token"})
		case request.Method == http.MethodGet && request.URL.Path == "/installation/repositories":
			writeJSON(w, http.StatusOK, map[string]any{"total_count": 1, "repositories": []map[string]any{{
				"id": 1, "full_name": "acme/private", "html_url": "https://github.com/acme/private",
				"clone_url": "https://github.com/acme/private.git", "private": true,
				"archived": false, "default_branch": "main",
			}}})
		case request.Method == http.MethodDelete:
			w.WriteHeader(http.StatusNoContent)
		default:
			http.NotFound(w, request)
		}
	}))
	t.Cleanup(provider.Close)
	oldBase := githubAPIBase
	githubAPIBase = provider.URL
	t.Cleanup(func() { githubAPIBase = oldBase })

	recorder := httptest.NewRecorder()
	request := withURLParam(newRequest(http.MethodGet, "/api/automations/"+automationID+"/github-catalog", nil), "id", automationID)
	testHandler.GetAutomationGitHubCatalog(recorder, request)
	if recorder.Code != http.StatusOK {
		t.Fatalf("catalog = %d %s", recorder.Code, recorder.Body.String())
	}
	var response AutomationGitHubCatalogResponse
	if err := json.Unmarshal(recorder.Body.Bytes(), &response); err != nil {
		t.Fatal(err)
	}
	if len(response.Repositories) != 1 || response.Repositories[0].FullName != "acme/private" {
		t.Fatalf("repositories = %+v", response.Repositories)
	}
	if len(response.MeLogins) != 1 || response.MeLogins[0] != "current-octocat" {
		t.Fatalf("me_logins = %#v", response.MeLogins)
	}
}
