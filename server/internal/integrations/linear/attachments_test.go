package linear

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestUpsertAttachmentUsesProviderURLIdentity(t *testing.T) {
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
		if input["issueId"] != "issue" || input["title"] != "PR #1" || input["url"] != "https://github.com/acme/repo/pull/1" {
			t.Errorf("input=%v", input)
		}
		_, _ = w.Write([]byte(`{"data":{"attachmentCreate":{"success":true}}}`))
	}))
	defer server.Close()
	client := NewHTTPClient(server.Client())
	client.GraphQLURL = server.URL
	if err := client.UpsertAttachment(context.Background(), "token", "issue", "PR #1", "https://github.com/acme/repo/pull/1"); err != nil {
		t.Fatal(err)
	}
}

func TestUpsertAttachmentRejectsUnsafeURL(t *testing.T) {
	client := NewHTTPClient(nil)
	for _, value := range []string{"http://example.com/pr", "https://user:pass@example.com/pr", "not a url"} {
		if err := client.UpsertAttachment(context.Background(), "token", "issue", "PR", value); err == nil {
			t.Fatalf("accepted %q", value)
		}
	}
}

func TestDeleteAttachmentByURLScopesDeletionToIssue(t *testing.T) {
	var requests int
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requests++
		var request struct {
			Query     string                     `json:"query"`
			Variables map[string]json.RawMessage `json:"variables"`
		}
		if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
			t.Error(err)
		}
		if requests == 1 {
			if !strings.Contains(request.Query, "attachmentsForURL") {
				t.Fatalf("query=%s", request.Query)
			}
			_, _ = w.Write([]byte(`{"data":{"attachmentsForURL":{"nodes":[{"id":"other","issue":{"id":"other-issue"}}],"pageInfo":{"hasNextPage":true,"endCursor":"page-2"}}}}`))
			return
		}
		if requests == 2 {
			if string(request.Variables["after"]) != `"page-2"` {
				t.Fatalf("second page cursor=%s", request.Variables["after"])
			}
			_, _ = w.Write([]byte(`{"data":{"attachmentsForURL":{"nodes":[{"id":"ours","issue":{"id":"issue"}}],"pageInfo":{"hasNextPage":false}}}}`))
			return
		}
		if !strings.Contains(request.Query, "attachmentDelete") || string(request.Variables["id"]) != `"ours"` {
			t.Fatalf("delete request=%+v", request)
		}
		_, _ = w.Write([]byte(`{"data":{"attachmentDelete":{"success":true}}}`))
	}))
	defer server.Close()
	client := NewHTTPClient(server.Client())
	client.GraphQLURL = server.URL
	if err := client.DeleteAttachmentByURL(context.Background(), "token", "issue", "https://github.com/acme/repo/pull/1"); err != nil {
		t.Fatal(err)
	}
	if requests != 3 {
		t.Fatalf("requests=%d, want two list pages plus one scoped delete", requests)
	}
}
