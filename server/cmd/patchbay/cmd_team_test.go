package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/spf13/cobra"
)

func newTeamMemberSetRoleTestCmd() *cobra.Command {
	cmd := &cobra.Command{Use: "set-role"}
	cmd.Flags().String("server-url", "", "")
	cmd.Flags().String("workspace-id", "", "")
	cmd.Flags().String("profile", "", "")
	cmd.Flags().String("member-id", "", "")
	cmd.Flags().String("member-type", "agent", "")
	cmd.Flags().String("role", "", "")
	cmd.Flags().String("output", "json", "")
	return cmd
}

func TestTeamMemberSetRoleCommandIsRegistered(t *testing.T) {
	cmd, _, err := teamMemberCmd.Find([]string{"set-role", "team-123"})
	if err != nil {
		t.Fatalf("find set-role command: %v", err)
	}
	if cmd == nil || cmd.Name() != "set-role" {
		t.Fatalf("set-role command not registered; got %#v", cmd)
	}
	for _, flag := range []string{"member-id", "member-type", "role", "output"} {
		if cmd.Flags().Lookup(flag) == nil {
			t.Fatalf("set-role command missing --%s flag", flag)
		}
	}
}

func TestRunTeamMemberSetRolePatchesRole(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("PATCHBAY_TOKEN", "test-token")
	t.Setenv("PATCHBAY_WORKSPACE_ID", "workspace-123")

	var gotMethod, gotPath string
	var gotBody map[string]any
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotMethod = r.Method
		gotPath = r.URL.Path
		if err := json.NewDecoder(r.Body).Decode(&gotBody); err != nil {
			t.Fatalf("decode request body: %v", err)
		}
		if r.Header.Get("X-Workspace-ID") != "workspace-123" {
			t.Fatalf("X-Workspace-ID = %q, want workspace-123", r.Header.Get("X-Workspace-ID"))
		}
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"team_id":    "team-123",
			"member_id":   "member-456",
			"member_type": "agent",
			"role":        "reviewer",
		})
	}))
	defer srv.Close()
	t.Setenv("PATCHBAY_SERVER_URL", srv.URL)

	cmd := newTeamMemberSetRoleTestCmd()
	_ = cmd.Flags().Set("member-id", "member-456")
	_ = cmd.Flags().Set("member-type", "agent")
	_ = cmd.Flags().Set("role", "reviewer")
	_ = cmd.Flags().Set("output", "json")

	if err := runTeamMemberSetRole(cmd, []string{"team-123"}); err != nil {
		t.Fatalf("runTeamMemberSetRole: %v", err)
	}
	if gotMethod != http.MethodPatch {
		t.Fatalf("method = %s, want PATCH", gotMethod)
	}
	if gotPath != "/api/teams/team-123/members/role" {
		t.Fatalf("path = %q, want /api/teams/team-123/members/role", gotPath)
	}
	wantBody := map[string]any{"member_id": "member-456", "member_type": "agent", "role": "reviewer"}
	for k, want := range wantBody {
		if gotBody[k] != want {
			t.Fatalf("body[%s] = %v, want %v (full body: %#v)", k, gotBody[k], want, gotBody)
		}
	}
}

func TestRunTeamMemberSetRoleValidatesRequiredFlags(t *testing.T) {
	cmd := newTeamMemberSetRoleTestCmd()
	if err := runTeamMemberSetRole(cmd, []string{"team-123"}); err == nil {
		t.Fatal("expected missing --member-id error")
	}

	cmd = newTeamMemberSetRoleTestCmd()
	_ = cmd.Flags().Set("member-id", "member-456")
	_ = cmd.Flags().Set("member-type", "invalid")
	if err := runTeamMemberSetRole(cmd, []string{"team-123"}); err == nil {
		t.Fatal("expected invalid --member-type error")
	}

	cmd = newTeamMemberSetRoleTestCmd()
	_ = cmd.Flags().Set("member-id", "member-456")
	if err := runTeamMemberSetRole(cmd, []string{"team-123"}); err == nil {
		t.Fatal("expected missing --role error")
	}
}
