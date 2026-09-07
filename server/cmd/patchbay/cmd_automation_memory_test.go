package main

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/spf13/cobra"
)

func TestAutomationMemoryCLIWritePreservesContentAndRevision(t *testing.T) {
	const content = "# Findings\n\n第一轮记录\n"
	const automationID = "11111111-1111-1111-1111-111111111111"
	conflict := false
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "PUT" || r.URL.Path != "/api/automations/"+automationID+"/memories/MEMORIES.md" {
			t.Errorf("unexpected request: %s %s", r.Method, r.URL.Path)
			http.NotFound(w, r)
			return
		}
		var body struct {
			Content  string `json:"content"`
			Revision int    `json:"expected_revision"`
		}
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil || body.Content != content || body.Revision != 7 {
			t.Errorf("write body=%+v err=%v", body, err)
		}
		if conflict {
			w.WriteHeader(409)
			_ = json.NewEncoder(w).Encode(map[string]string{"error": "memory changed"})
			return
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"name": "MEMORIES.md", "content": content, "revision": 8})
	}))
	defer srv.Close()
	t.Setenv("ORVILO_SERVER_URL", srv.URL)
	t.Setenv("ORVILO_WORKSPACE_ID", "ws-1")
	t.Setenv("ORVILO_TOKEN", "test-token")
	file := filepath.Join(t.TempDir(), "note.md")
	if err := os.WriteFile(file, []byte(content), 0600); err != nil {
		t.Fatal(err)
	}
	cmd := &cobra.Command{Use: "write"}
	cmd.Flags().String("output", "json", "")
	cmd.Flags().String("content", "", "")
	cmd.Flags().String("file", "", "")
	cmd.Flags().Int64("revision", -1, "")
	_ = cmd.Flags().Set("file", file)
	_ = cmd.Flags().Set("revision", "7")
	var out bytes.Buffer
	cmd.SetOut(&out)
	if err := runAutomationMemory(cmd, []string{automationID}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), `"revision": 8`) {
		t.Fatalf("missing saved revision: %s", out.String())
	}
	conflict = true
	if err := runAutomationMemory(cmd, []string{automationID}); err == nil || !strings.Contains(err.Error(), "memory changed") {
		t.Fatalf("conflict was not surfaced: %v", err)
	}
	if err := os.WriteFile(file, []byte{0xff}, 0600); err != nil {
		t.Fatal(err)
	}
	if err := runAutomationMemory(cmd, []string{automationID}); err == nil || !strings.Contains(err.Error(), "UTF-8") {
		t.Fatalf("invalid file encoding was not rejected: %v", err)
	}
}
