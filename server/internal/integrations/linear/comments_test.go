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
		for key, want := range map[string]any{"id": "comment-1", "issueId": "issue-1", "parentId": "parent-1", "body": "Hello", "createAsUser": "Alex via Patchbay", "doNotSubscribeToIssue": true} {
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
	comment, err := client.CreateComment(context.Background(), "fixture-token", "comment-1", "issue-1", "parent-1", "Hello", "Alex via Patchbay")
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
