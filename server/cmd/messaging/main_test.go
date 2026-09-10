package main

import (
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
)

type commandProviderTransport func(*http.Request) (*http.Response, error)

func (f commandProviderTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func TestWeixinOperatorCommandUsesServerEnvironmentAndScopeFlags(t *testing.T) {
	dsn := os.Getenv("DATABASE_URL")
	if dsn == "" {
		t.Skip("DATABASE_URL is required")
	}
	pool, err := pgxpool.New(t.Context(), dsn)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(pool.Close)
	fx := testutil.New(pool, "", "")
	userID := fx.User(t, "Weixin Command", "weixin-command@example.test")
	workspaceID := fx.Workspace(t, "Weixin Command", "weixin-command")
	fx.Member(t, workspaceID, userID, "owner")
	fx.Cleanup(t, `DELETE FROM channel_installation WHERE workspace_id = $1`, workspaceID)
	fx.Cleanup(t, `DELETE FROM channel_user_binding WHERE workspace_id = $1`, workspaceID)
	t.Setenv("ORVILO_APP_URL", "https://app.example.test")
	t.Setenv("ORVILO_MESSAGING_MODE", "server_configured")
	t.Setenv("ORVILO_WEIXIN_SECRET_KEY", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
	t.Setenv("ORVILO_MESSAGING_WORKSPACE_ID", "invalid-environment-scope")
	t.Setenv("ORVILO_MESSAGING_INSTALLER_USER_ID", "invalid-environment-installer")
	t.Setenv("ORVILO_MESSAGING_AGENT_ID", "")
	previous := http.DefaultTransport
	http.DefaultTransport = commandProviderTransport(func(r *http.Request) (*http.Response, error) {
		w := httptest.NewRecorder()
		w.Header().Set("Content-Type", "application/json")
		if r.URL.Path == "/ilink/bot/get_bot_qrcode" {
			_, _ = io.WriteString(w, `{"qrcode":"command-poll-token","qrcode_img_content":"https://weixin.qq.com/x/command-scan"}`)
		} else if r.URL.Path == "/ilink/bot/get_qrcode_status" {
			_ = json.NewEncoder(w).Encode(map[string]string{
				"status": "confirmed", "bot_token": "command-bot-token-private",
				"ilink_bot_id": "command-bot", "ilink_user_id": "command-scanner",
			})
		} else {
			t.Fatalf("unexpected provider path %q", r.URL.Path)
		}
		return w.Result(), nil
	})
	t.Cleanup(func() { http.DefaultTransport = previous })
	var output bytes.Buffer
	err = run(t.Context(), []string{"weixin-auth", "--workspace-id", workspaceID, "--installer-user-id", userID}, strings.NewReader(""), &output)
	if err != nil {
		t.Fatal(err)
	}
	if got := fx.Count(t, `SELECT count(*) FROM channel_installation WHERE workspace_id = $1 AND channel_type = 'weixin'`, workspaceID); got != 1 {
		t.Fatalf("command installed rows = %d, want 1", got)
	}
	if strings.Contains(output.String(), "command-bot-token-private") || strings.Contains(output.String(), "command-poll-token") {
		t.Fatal("command exposed provider credentials")
	}
}

func TestWeixinOperatorHelpDoesNotRequireCredentials(t *testing.T) {
	t.Setenv("DATABASE_URL", "")
	t.Setenv("ORVILO_WEIXIN_SECRET_KEY", "")
	var output bytes.Buffer
	if err := run(t.Context(), []string{"weixin-auth", "--help"}, strings.NewReader(""), &output); err != nil {
		t.Fatal(err)
	}
	for _, expected := range []string{"workspace-id", "installer-user-id", "agent-id", "ORVILO_WEIXIN_SECRET_KEY"} {
		if !strings.Contains(output.String(), expected) {
			t.Fatalf("help is missing %q", expected)
		}
	}
}

func TestWeixinOperatorCannotOverrideDeploymentMode(t *testing.T) {
	t.Setenv("ORVILO_APP_URL", "https://orvilo.aspectlylabs.com")
	t.Setenv("ORVILO_MESSAGING_MODE", "server_configured")
	var output bytes.Buffer
	if err := run(t.Context(), []string{"weixin-auth"}, strings.NewReader(""), &output); err == nil || !strings.Contains(err.Error(), "server_configured") {
		t.Fatalf("hosted deployment accepted operator command: %v", err)
	}
	if err := run(t.Context(), []string{"weixin-auth", "--mode", "server_configured"}, strings.NewReader(""), &output); err == nil {
		t.Fatal("operator exposed a mode override")
	}
}

func TestWeixinOperatorRequiresExplicitDatabase(t *testing.T) {
	t.Setenv("ORVILO_APP_URL", "https://app.example.test")
	t.Setenv("ORVILO_MESSAGING_MODE", "server_configured")
	t.Setenv("DATABASE_URL", "")
	var output bytes.Buffer
	if err := run(t.Context(), []string{"weixin-auth"}, strings.NewReader(""), &output); err == nil || !strings.Contains(err.Error(), "DATABASE_URL") {
		t.Fatalf("operator silently selected a database: %v", err)
	}
}

func TestWeixinOperatorDatabaseErrorDoesNotExposeCredentials(t *testing.T) {
	t.Setenv("ORVILO_APP_URL", "https://app.example.test")
	t.Setenv("ORVILO_MESSAGING_MODE", "server_configured")
	t.Setenv("DATABASE_URL", "postgres://fixture-user:do-not-print-this@127.0.0.1:not-a-port/operator")
	var output bytes.Buffer
	err := run(t.Context(), []string{"weixin-auth"}, strings.NewReader(""), &output)
	if err == nil || strings.Contains(err.Error(), "do-not-print-this") || strings.Contains(output.String(), "do-not-print-this") {
		t.Fatalf("database failure was not reported safely: %v", err)
	}
}
