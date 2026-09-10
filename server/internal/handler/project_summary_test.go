package handler

import (
	"net/http"
	"testing"

	"github.com/orvilo-ai/orvilo/server/internal/testutil"
)

func TestProjectSummaryLifecycle(t *testing.T) {
	created := testutil.Decode[ProjectResponse](t, testHandler.CreateProject,
		newRequest(http.MethodPost, "/api/projects?workspace_id="+testWorkspaceID, map[string]any{
			"title":   "Project summary lifecycle",
			"summary": "A short project overview.",
		}), http.StatusCreated)
	dbfx.Cleanup(t, `DELETE FROM project WHERE id = $1`, created.ID)
	if created.Summary == nil || *created.Summary != "A short project overview." {
		t.Fatalf("created summary = %v", created.Summary)
	}

	get := func() ProjectResponse {
		return testutil.Decode[ProjectResponse](t, testHandler.GetProject,
			withURLParam(newRequest(http.MethodGet, "/api/projects/"+created.ID, nil), "id", created.ID), http.StatusOK)
	}
	if got := get(); got.Summary == nil || *got.Summary != "A short project overview." {
		t.Fatalf("persisted summary = %v", got.Summary)
	}

	listed := testutil.Decode[struct {
		Projects []ProjectResponse `json:"projects"`
	}](t, testHandler.ListProjects,
		newRequest(http.MethodGet, "/api/projects?workspace_id="+testWorkspaceID, nil), http.StatusOK)
	var listedProject *ProjectResponse
	for i := range listed.Projects {
		if listed.Projects[i].ID == created.ID {
			listedProject = &listed.Projects[i]
			break
		}
	}
	if listedProject == nil || listedProject.Summary == nil || *listedProject.Summary != "A short project overview." {
		t.Fatalf("listed summary = %v", listedProject)
	}

	searched := testutil.Decode[struct {
		Projects []SearchProjectResponse `json:"projects"`
	}](t, testHandler.SearchProjects,
		newRequest(http.MethodGet, "/api/projects/search?q=Project+summary+lifecycle", nil), http.StatusOK)
	var searchedProject *SearchProjectResponse
	for i := range searched.Projects {
		if searched.Projects[i].ID == created.ID {
			searchedProject = &searched.Projects[i]
			break
		}
	}
	if searchedProject == nil || searchedProject.Summary == nil || *searchedProject.Summary != "A short project overview." {
		t.Fatalf("searched summary = %v", searchedProject)
	}

	updated := testutil.Decode[ProjectResponse](t, testHandler.UpdateProject,
		withURLParam(newRequest(http.MethodPut, "/api/projects/"+created.ID, map[string]any{
			"summary": "Updated overview.",
		}), "id", created.ID), http.StatusOK)
	if updated.Summary == nil || *updated.Summary != "Updated overview." {
		t.Fatalf("updated summary = %v", updated.Summary)
	}

	cleared := testutil.Decode[ProjectResponse](t, testHandler.UpdateProject,
		withURLParam(newRequest(http.MethodPut, "/api/projects/"+created.ID, map[string]any{
			"summary": nil,
		}), "id", created.ID), http.StatusOK)
	if cleared.Summary != nil {
		t.Fatalf("cleared summary = %v", cleared.Summary)
	}
}

func TestSearchProjectSummaryMatchIncludesSummarySnippet(t *testing.T) {
	created := testutil.Decode[ProjectResponse](t, testHandler.CreateProject,
		newRequest(http.MethodPost, "/api/projects?workspace_id="+testWorkspaceID, map[string]any{
			"title":   "Untitled delivery board",
			"summary": "Unique summary signal for project search.",
		}), http.StatusCreated)
	dbfx.Cleanup(t, `DELETE FROM project WHERE id = $1`, created.ID)

	searched := testutil.Decode[struct {
		Projects []SearchProjectResponse `json:"projects"`
	}](t, testHandler.SearchProjects,
		newRequest(http.MethodGet, "/api/projects/search?q=Unique+summary+signal", nil), http.StatusOK)
	for _, project := range searched.Projects {
		if project.ID != created.ID {
			continue
		}
		if project.MatchSource != "summary" {
			t.Fatalf("match source = %q, want summary", project.MatchSource)
		}
		if project.MatchedSnippet == nil || *project.MatchedSnippet == "" {
			t.Fatalf("summary match snippet = %v, want non-empty", project.MatchedSnippet)
		}
		return
	}
	t.Fatalf("summary-only project %q missing from search results", created.ID)
}
