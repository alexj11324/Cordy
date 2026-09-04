package linear

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestHTTPClientOAuthUsesPKCEAndRejectsIncompleteTokens(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if err := r.ParseForm(); err != nil { t.Fatal(err) }
		if r.Form.Get("grant_type") != "authorization_code" || r.Form.Get("code_verifier") != "verifier" || r.Form.Get("redirect_uri") != "https://app/callback" { t.Fatalf("oauth form = %v", r.Form) }
		_ = json.NewEncoder(w).Encode(map[string]any{"access_token":"access","refresh_token":"refresh","scope":[]string{"read","issues:create"},"expires_in":3600})
	}))
	defer server.Close()
	client := NewHTTPClient(server.Client()); client.TokenURL = server.URL
	token, err := client.ExchangeAuthorizationCode(context.Background(),"code","https://app/callback","verifier","client","secret")
	if err != nil || token.AccessToken != "access" || token.RefreshToken != "refresh" || token.Scope != "read issues:create" { t.Fatalf("token=%+v err=%v",token,err) }
	bad := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { _ = json.NewEncoder(w).Encode(map[string]any{"access_token":"access","refresh_token":"refresh","expires_in":0}) }))
	defer bad.Close(); client.TokenURL = bad.URL
	if _, err = client.RefreshToken(context.Background(),"refresh","client","secret"); err == nil || !IsKind(err, ErrorInvalidResponse) { t.Fatalf("incomplete token err=%v",err) }
}

func TestPatchbayIssueMarkerRoundTrip(t *testing.T) {
	description := DescriptionWithPatchbayMarker("body", "issue-1")
	if PatchbayIssueIDFromDescription(description) != "issue-1" { t.Fatalf("marker=%q",description) }
	if got := StripPatchbayIssueMarker(description); got != "body" { t.Fatalf("stripped=%q",got) }
}

func TestHTTPClientTokenRefreshAndRevokeContracts(t *testing.T) {
	var revoked bool
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if err := r.ParseForm(); err != nil {
			t.Fatal(err)
		}
		switch r.URL.Path {
		case "/token":
			if r.Form.Get("grant_type") != "refresh_token" || r.Form.Get("refresh_token") != "old-refresh" {
				t.Fatalf("refresh form = %v", r.Form)
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"access_token": "new-access", "refresh_token": "new-refresh", "scope": "read write", "expires_in": 3600})
		case "/revoke":
			revoked = r.Form.Get("token") == "new-access" && r.Form.Get("client_secret") == "secret"
			w.WriteHeader(http.StatusNoContent)
		default:
			t.Fatalf("unexpected path %s", r.URL.Path)
		}
	}))
	defer server.Close()
	client := NewHTTPClient(server.Client())
	client.TokenURL = server.URL + "/token"
	client.RevokeURL = server.URL + "/revoke"
	token, err := client.RefreshToken(context.Background(), "old-refresh", "client", "secret")
	if err != nil {
		t.Fatal(err)
	}
	if token.AccessToken != "new-access" || token.RefreshToken != "new-refresh" {
		t.Fatalf("token = %+v", token)
	}
	if err := client.RevokeToken(context.Background(), token.AccessToken, "client", "secret"); err != nil {
		t.Fatal(err)
	}
	if !revoked {
		t.Fatal("revoke request did not carry expected credential")
	}
}

func TestHTTPClientGraphQLIssueContracts(t *testing.T) {
	seen := map[string]bool{}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer access" {
			t.Fatalf("authorization = %q", r.Header.Get("Authorization"))
		}
		var request struct {
			Query     string         `json:"query"`
			Variables map[string]any `json:"variables"`
		}
		if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
			t.Fatal(err)
		}
		switch {
		case strings.Contains(request.Query, "PatchbayIssues"):
			seen["list"] = true
			_ = json.NewEncoder(w).Encode(map[string]any{"data": map[string]any{"issues": map[string]any{"nodes": []any{map[string]any{"id": "remote-1", "identifier": "ENG-1", "title": "Imported", "priority": 2, "updatedAt": "2026-01-01T00:00:00Z", "team": map[string]any{"id": "team-1"}}}, "pageInfo": map[string]any{"hasNextPage": false, "endCursor": nil}}}})
		case strings.Contains(request.Query, "PatchbayCreateIssue"):
			seen["create"] = true
			input := request.Variables["input"].(map[string]any)
			if input["projectId"] != "project-1" || input["teamId"] != "team-1" {
				t.Fatalf("create input = %#v", input)
			}
			_ = json.NewEncoder(w).Encode(issueMutation("issueCreate", "remote-2"))
		case strings.Contains(request.Query, "PatchbayUpdateIssue"):
			seen["update"] = request.Variables["id"] == "remote-2"
			_ = json.NewEncoder(w).Encode(issueMutation("issueUpdate", "remote-2"))
		case strings.Contains(request.Query, "PatchbayDeleteIssue"):
			seen["delete"] = request.Variables["id"] == "remote-2"
			_ = json.NewEncoder(w).Encode(map[string]any{"data": map[string]any{"issueDelete": map[string]any{"success": true}}})
		default:
			t.Fatalf("unexpected GraphQL operation %s", request.Query)
		}
	}))
	defer server.Close()
	client := NewHTTPClient(server.Client())
	client.GraphQLURL = server.URL
	issues, err := client.ListIssues(context.Background(), "access", "project-1", "team-1")
	if err != nil || len(issues) != 1 || issues[0].Identifier != "ENG-1" {
		t.Fatalf("issues=%+v err=%v", issues, err)
	}
	created, err := client.CreateIssue(context.Background(), "access", IssueInput{ProjectID: "project-1", TeamID: "team-1", Title: "Local"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err = client.UpdateIssue(context.Background(), "access", created.ID, IssueInput{TeamID: "team-1", Title: "Updated"}); err != nil {
		t.Fatal(err)
	}
	if err = client.DeleteIssue(context.Background(), "access", created.ID); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"list", "create", "update", "delete"} {
		if !seen[name] {
			t.Fatalf("operation %s not observed", name)
		}
	}
}

func issueMutation(name, id string) map[string]any {
	return map[string]any{"data": map[string]any{name: map[string]any{"success": true, "issue": map[string]any{"id": id, "identifier": "ENG-2", "title": "Synced", "priority": 3, "updatedAt": "2026-01-01T00:00:00Z", "team": map[string]any{"id": "team-1"}}}}}
}

func TestHTTPClientSurfacesGraphQLErrors(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{"errors": []any{map[string]any{"message": "denied"}}})
	}))
	defer server.Close()
	client := NewHTTPClient(server.Client())
	client.GraphQLURL = server.URL
	_, err := client.ListIssues(context.Background(), "access", "p", "")
	if err == nil || !strings.Contains(err.Error(), "denied") {
		t.Fatalf("err = %v", err)
	}
}
