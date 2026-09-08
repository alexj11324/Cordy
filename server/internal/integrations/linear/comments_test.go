package linear

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestCreateCommentPreservesRetryIdentityAndParent(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var request struct {
			Variables map[string]json.RawMessage `json:"variables"`
		}
		if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
			t.Error(err)
		}
		var input map[string]any
		if err := json.Unmarshal(request.Variables["input"], &input); err != nil {
			t.Error(err)
		}
		for key, want := range map[string]any{"id": "comment-1", "issueId": "issue-1", "parentId": "parent-1", "body": "Hello", "createAsUser": "Alex vian Orvilo", "doNotSubscribeToIssue": true} {
			if input[key] != want {
				t.Errorf("%s = %v, want %v", key, input[key], want)
			}
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"data":{"commentCreate":{"success":true,"comment":{"id":"comment-1","issue":{"id":"issue-1"},"body":"Hello"}}}}`))
	}))
	defer server.Close()
	client := NewHTTPClient(server.Client())
	client.GraphQLURL = server.URL
	comment, err := client.CreateComment(context.Background(), "fixture-token", "comment-1", "issue-1", "parent-1", "Hello", "Alex vian Orvilo")
	if err != nil || comment.ID != "comment-1" {
		t.Fatalf("comment=%+v err=%v", comment, err)
	}
}

func TestCommentMutationFailureIsNotAcknowledged(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{"data":{"commentUpdate":{"success":false},"commentDelete":{"success":false},"commentCreate":{"success":false}}}`))
	}))
	defer server.Close()
	client := NewHTTPClient(server.Client())
	client.GraphQLURL = server.URL
	if err := client.UpdateComment(context.Background(), "fixture-token", "id", "body"); err == nil {
		t.Fatal("rejected update acknowledged")
	}
	if err := client.DeleteComment(context.Background(), "fixture-token", "id"); err == nil {
		t.Fatal("rejected delete acknowledged")
	}
	if _, err := client.CreateComment(context.Background(), "fixture-token", "id", "issue", "", "body", ""); err == nil {
		t.Fatal("rejected create acknowledged")
	}
}

func TestListCommentsRejectsCrossIssueResults(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{"data":{"comments":{"nodes":[{"id":"comment","issue":{"id":"other"},"updatedAt":"2026-09-05T00:00:00Z"}],"pageInfo":{"hasNextPage":false}}}}`))
	}))
	defer server.Close()
	client := NewHTTPClient(server.Client())
	client.GraphQLURL = server.URL
	if _, err := client.ListComments(context.Background(), "fixture-token", "expected"); err == nil {
		t.Fatal("cross-issue result accepted")
	}
}

func TestListCommentsRejectsStalledCursor(t *testing.T) {
	calls := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls++
		_, _ = w.Write([]byte(`{"data":{"comments":{"nodes":[],"pageInfo":{"hasNextPage":true,"endCursor":"same"}}}}`))
	}))
	defer server.Close()
	client := NewHTTPClient(server.Client())
	client.GraphQLURL = server.URL
	_, err := client.ListComments(context.Background(), "fixture-token", "issue")
	if err == nil || !strings.Contains(err.Error(), "advance") || calls != 2 {
		t.Fatalf("calls=%d err=%v", calls, err)
	}
}

func TestListCommentsUsesStringIdentifierVariable(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var request struct {
			Query string `json:"query"`
		}
		if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
			t.Fatal(err)
		}
		if !strings.Contains(request.Query, "query($issue: String!") {
			_ = json.NewEncoder(w).Encode(map[string]any{"errors": []any{map[string]any{
				"message": `Variable "$issue" of type "ID!" used in position expecting type "String!".`,
			}}})
			return
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"data": map[string]any{
			"comments": map[string]any{"nodes": []any{}, "pageInfo": map[string]any{"hasNextPage": false}},
		}})
	}))
	defer server.Close()
	client := NewHTTPClient(server.Client())
	client.GraphQLURL = server.URL
	if comments, err := client.ListComments(context.Background(), "fixture-token", "issue-1"); err != nil || len(comments) != 0 {
		t.Fatalf("comments=%v err=%v", comments, err)
	}
}
