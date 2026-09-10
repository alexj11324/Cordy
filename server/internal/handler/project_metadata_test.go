package handler

import (
	"net/http"
	"testing"

	"github.com/orvilo-ai/orvilo/server/internal/testutil"
)

func TestProjectMetadataLifecycle(t *testing.T) {
	otherUser := dbfx.User(t, "Project Metadata Member", "project-metadata-member@orvilo.test")
	dbfx.Member(t, testWorkspaceID, otherUser, "member")
	labelID := dbfx.Insert(t, "issue_label", testutil.Cols{
		"workspace_id":  testWorkspaceID,
		"resource_type": "project",
		"name":          "Project Metadata Label",
		"description":   "",
		"color":         "#64748b",
	})
	prerequisiteID := dbfx.Project(t, "Project Metadata Prerequisite")

	created := testutil.Decode[ProjectResponse](t, testHandler.CreateProject,
		newRequest("POST", "/api/projects?workspace_id="+testWorkspaceID, map[string]any{
			"title":          "Project Metadata Lifecycle",
			"member_ids":     []string{testUserID, otherUser},
			"label_ids":      []string{labelID},
			"dependency_ids": []string{prerequisiteID},
			"milestones": []ProjectMilestone{{
				ID: "11111111-1111-4111-8111-111111111111", Title: "Ship metadata", DueDate: strPtr("2026-10-01"), Status: "planned",
			}},
		}), http.StatusCreated)
	if len(created.MemberIDs) != 2 || created.MemberIDs[0] != testUserID || created.MemberIDs[1] != otherUser {
		t.Fatalf("created member_ids = %#v", created.MemberIDs)
	}
	if len(created.LabelIDs) != 1 || created.LabelIDs[0] != labelID {
		t.Fatalf("created label_ids = %#v", created.LabelIDs)
	}
	if len(created.DependencyIDs) != 1 || created.DependencyIDs[0] != prerequisiteID {
		t.Fatalf("created dependency_ids = %#v", created.DependencyIDs)
	}
	if len(created.Milestones) != 1 || created.Milestones[0].Title != "Ship metadata" {
		t.Fatalf("created milestones = %#v", created.Milestones)
	}

	deleteProject := func(id string) {
		req := newRequest("DELETE", "/api/projects/"+id, nil)
		req = withURLParam(req, "id", id)
		testutil.Call(t, testHandler.DeleteProject, req).Want(http.StatusNoContent)
	}
	defer func() {
		// The prerequisite is deleted below in the happy path. The cleanup is
		// still useful when an assertion fails before that point.
		deleteProject(created.ID)
	}()

	got := testutil.Decode[ProjectResponse](t, testHandler.GetProject,
		withURLParam(newRequest("GET", "/api/projects/"+created.ID, nil), "id", created.ID), http.StatusOK)
	if got.LabelIDs[0] != labelID || got.DependencyIDs[0] != prerequisiteID || got.Milestones[0].DueDate == nil || *got.Milestones[0].DueDate != "2026-10-01" {
		t.Fatalf("GET metadata = %#v", got)
	}
	listed := testutil.Decode[struct {
		Projects []ProjectResponse `json:"projects"`
	}](t, testHandler.ListProjects,
		newRequest("GET", "/api/projects?workspace_id="+testWorkspaceID, nil), http.StatusOK)
	var listedProject *ProjectResponse
	for i := range listed.Projects {
		if listed.Projects[i].ID == created.ID {
			listedProject = &listed.Projects[i]
			break
		}
	}
	if listedProject == nil || len(listedProject.DependencyIDs) != 1 || listedProject.DependencyIDs[0] != prerequisiteID {
		t.Fatalf("LIST metadata = %#v", listed.Projects)
	}

	testutil.Decode[ProjectResponse](t, testHandler.UpdateProject,
		withURLParam(newRequest("PUT", "/api/projects/"+created.ID, map[string]any{
			"member_ids": []string{}, "milestones": []ProjectMilestone{},
		}), "id", created.ID), http.StatusOK)
	got = testutil.Decode[ProjectResponse](t, testHandler.GetProject,
		withURLParam(newRequest("GET", "/api/projects/"+created.ID, nil), "id", created.ID), http.StatusOK)
	if len(got.MemberIDs) != 0 || len(got.Milestones) != 0 || len(got.LabelIDs) != 1 || len(got.DependencyIDs) != 1 {
		t.Fatalf("partial update did not preserve/clear fields: %#v", got)
	}
	labels := testutil.Decode[struct {
		Labels []LabelResponse `json:"labels"`
	}](t, testHandler.ListLabels,
		newRequest("GET", "/api/labels?workspace_id="+testWorkspaceID+"&resource_type=project", nil), http.StatusOK)
	if len(labels.Labels) != 1 || labels.Labels[0].ID != labelID || labels.Labels[0].UsageCount != 1 {
		t.Fatalf("project label usage = %#v", labels.Labels)
	}

	// The project already depends on its prerequisite, so making the
	// prerequisite depend on the project would close a cycle. The rejected
	// update must leave the prerequisite unchanged.
	cycleReq := newRequest("PUT", "/api/projects/"+prerequisiteID, map[string]any{
		"dependency_ids": []string{created.ID},
	})
	testutil.Call(t, testHandler.UpdateProject, withURLParam(cycleReq, "id", prerequisiteID)).Want(http.StatusBadRequest)

	deleteLabelReq := newRequest("DELETE", "/api/labels/"+labelID+"?workspace_id="+testWorkspaceID, nil)
	testutil.Call(t, testHandler.DeleteLabel, withURLParam(deleteLabelReq, "id", labelID)).Want(http.StatusNoContent)
	got = testutil.Decode[ProjectResponse](t, testHandler.GetProject,
		withURLParam(newRequest("GET", "/api/projects/"+created.ID, nil), "id", created.ID), http.StatusOK)
	if len(got.LabelIDs) != 0 {
		t.Fatalf("deleted label reference cleanup = %#v", got.LabelIDs)
	}

	deleteProject(prerequisiteID)
	got = testutil.Decode[ProjectResponse](t, testHandler.GetProject,
		withURLParam(newRequest("GET", "/api/projects/"+created.ID, nil), "id", created.ID), http.StatusOK)
	if len(got.DependencyIDs) != 0 {
		t.Fatalf("inbound dependency cleanup = %#v", got.DependencyIDs)
	}
}

func TestProjectMetadataRejectsUnknownReferencesAtomically(t *testing.T) {
	before := dbfx.Count(t, `SELECT count(*) FROM project WHERE workspace_id = $1`, testWorkspaceID)
	unknown := "22222222-2222-4222-8222-222222222222"
	req := newRequest("POST", "/api/projects?workspace_id="+testWorkspaceID, map[string]any{
		"title":      "Invalid Project Metadata",
		"member_ids": []string{unknown},
	})
	testutil.Call(t, testHandler.CreateProject, req).Want(http.StatusBadRequest)
	after := dbfx.Count(t, `SELECT count(*) FROM project WHERE workspace_id = $1`, testWorkspaceID)
	if after != before {
		t.Fatalf("invalid metadata changed project count: before=%d after=%d", before, after)
	}
}

func TestProjectMetadataRejectsInvalidReferencesAndMilestones(t *testing.T) {
	issueLabelID := dbfx.Insert(t, "issue_label", testutil.Cols{
		"workspace_id":  testWorkspaceID,
		"resource_type": "issue",
		"name":          "Issue Namespace Label",
		"description":   "",
		"color":         "#64748b",
	})
	projectID := dbfx.Project(t, "Project Metadata Invalid Update")
	unknownID := "33333333-3333-4333-8333-333333333333"
	cases := []struct {
		name string
		body map[string]any
		path string
	}{
		{name: "member outside workspace", body: map[string]any{"member_ids": []string{unknownID}}, path: "/api/projects?workspace_id=" + testWorkspaceID},
		{name: "issue label namespace", body: map[string]any{"label_ids": []string{issueLabelID}}, path: "/api/projects?workspace_id=" + testWorkspaceID},
		{name: "dependency outside workspace", body: map[string]any{"dependency_ids": []string{unknownID}}, path: "/api/projects?workspace_id=" + testWorkspaceID},
		{name: "self dependency", body: map[string]any{"dependency_ids": []string{projectID}}, path: "/api/projects/" + projectID},
		{name: "milestone date", body: map[string]any{"milestones": []ProjectMilestone{{ID: "44444444-4444-4444-8444-444444444444", Title: "bad date", DueDate: strPtr("2026-13-01"), Status: "planned"}}}, path: "/api/projects?workspace_id=" + testWorkspaceID},
		{name: "milestone status", body: map[string]any{"milestones": []ProjectMilestone{{ID: "55555555-5555-4555-8555-555555555555", Title: "bad status", Status: "started"}}}, path: "/api/projects?workspace_id=" + testWorkspaceID},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			body := map[string]any{}
			for key, value := range tc.body {
				body[key] = value
			}
			if tc.name == "self dependency" {
				req := newRequest("PUT", tc.path, body)
				testutil.Call(t, testHandler.UpdateProject, withURLParam(req, "id", projectID)).Want(http.StatusBadRequest)
				return
			}
			body["title"] = "Invalid metadata case"
			req := newRequest("POST", tc.path, body)
			testutil.Call(t, testHandler.CreateProject, req).Want(http.StatusBadRequest)
		})
	}
	got := testutil.Decode[ProjectResponse](t, testHandler.GetProject,
		withURLParam(newRequest("GET", "/api/projects/"+projectID, nil), "id", projectID), http.StatusOK)
	if len(got.DependencyIDs) != 0 || len(got.Milestones) != 0 {
		t.Fatalf("invalid metadata changed project: %#v", got)
	}
}

func TestProjectMetadataClearsDepartedMember(t *testing.T) {
	departingUserID := dbfx.User(t, "Departing Project Member", "project-metadata-departing@orvilo.test")
	memberID := dbfx.Member(t, testWorkspaceID, departingUserID, "member")
	created := testutil.Decode[ProjectResponse](t, testHandler.CreateProject,
		newRequest("POST", "/api/projects?workspace_id="+testWorkspaceID, map[string]any{
			"title":      "Project Member Cleanup",
			"member_ids": []string{departingUserID},
		}), http.StatusCreated)
	dbfx.Cleanup(t, `DELETE FROM project WHERE id = $1`, created.ID)

	deleteMember := newRequestAs(testUserID, "DELETE", "/api/workspaces/"+testWorkspaceID+"/members/"+memberID, nil)
	deleteMember = withURLParams(deleteMember, "id", testWorkspaceID, "memberId", memberID)
	testutil.Call(t, testHandler.DeleteMember, deleteMember).Want(http.StatusNoContent)

	got := testutil.Decode[ProjectResponse](t, testHandler.GetProject,
		withURLParam(newRequest("GET", "/api/projects/"+created.ID, nil), "id", created.ID), http.StatusOK)
	if len(got.MemberIDs) != 0 {
		t.Fatalf("departed member reference = %#v", got.MemberIDs)
	}
}
