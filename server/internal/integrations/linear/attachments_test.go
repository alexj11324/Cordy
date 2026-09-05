package linear

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
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
