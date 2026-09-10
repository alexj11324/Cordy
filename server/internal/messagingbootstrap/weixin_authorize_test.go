package messagingbootstrap

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/weixin"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	"github.com/orvilo-ai/orvilo/server/internal/util/secretbox"
)

const operatorBotToken = "operator-bot-token-must-never-be-printed"

type operatorRoundTripper func(*http.Request) (*http.Response, error)

func (f operatorRoundTripper) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func operatorFixture(t *testing.T) (*pgxpool.Pool, *testutil.Fixture, WeixinAuthorizationScope) {
	t.Helper()
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
	userID := fx.User(t, "Weixin Operator", "weixin-operator@example.test")
	workspaceID := fx.Workspace(t, "Weixin Operator", "weixin-operator")
	fx.Member(t, workspaceID, userID, "owner")
	fx.WorkspaceID, fx.UserID = workspaceID, userID
	fx.Cleanup(t, `DELETE FROM channel_installation WHERE workspace_id = $1`, workspaceID)
	fx.Cleanup(t, `DELETE FROM channel_user_binding WHERE workspace_id = $1`, workspaceID)
	t.Setenv("ORVILO_WEIXIN_SECRET_KEY", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
	return pool, fx, WeixinAuthorizationScope{WorkspaceID: workspaceID, InstallerUserID: userID}
}

func operatorProvider(t *testing.T, status func(http.ResponseWriter, *http.Request)) *int {
	t.Helper()
	calls := new(int)
	previous := http.DefaultTransport
	http.DefaultTransport = operatorRoundTripper(func(r *http.Request) (*http.Response, error) {
		*calls++
		w := httptest.NewRecorder()
		w.Header().Set("Content-Type", "application/json")
		switch r.URL.Path {
		case "/ilink/bot/get_bot_qrcode":
			_, _ = io.WriteString(w, `{"qrcode":"opaque-poll-token","qrcode_img_content":"https://weixin.qq.com/x/operator-scan"}`)
		case "/ilink/bot/get_qrcode_status":
			if r.URL.Query().Get("qrcode") != "opaque-poll-token" {
				t.Errorf("poll did not use the opaque QR token")
			}
			status(w, r)
		default:
			t.Fatalf("unexpected provider path %q", r.URL.Path)
		}
		return w.Result(), nil
	})
	t.Cleanup(func() { http.DefaultTransport = previous })
	return calls
}

func confirmedOperatorResponse(w http.ResponseWriter) {
	_ = json.NewEncoder(w).Encode(map[string]string{
		"status": "confirmed", "bot_token": operatorBotToken,
		"ilink_bot_id": "operator-bot", "ilink_user_id": "operator-weixin-user",
	})
}

func TestWeixinOperatorConfirmationEncryptsAndBinds(t *testing.T) {
	pool, fx, scope := operatorFixture(t)
	operatorProvider(t, func(w http.ResponseWriter, r *http.Request) { confirmedOperatorResponse(w) })
	var output bytes.Buffer
	for range 2 {
		installationID, err := AuthorizeWeixin(t.Context(), pool, serverConfigured, scope, strings.NewReader(""), &output)
		if err != nil || !installationID.Valid {
			t.Fatalf("authorization = %v, %v", installationID.Valid, err)
		}
	}
	if got := fx.Count(t, `SELECT count(*) FROM channel_installation WHERE workspace_id = $1 AND channel_type = 'weixin'`, scope.WorkspaceID); got != 1 {
		t.Fatalf("installed rows = %d, want 1", got)
	}
	if got := fx.Count(t, `SELECT count(*) FROM channel_user_binding WHERE workspace_id = $1 AND channel_type = 'weixin'`, scope.WorkspaceID); got != 1 {
		t.Fatalf("scanner bindings = %d, want 1", got)
	}
	var config []byte
	if err := pool.QueryRow(t.Context(), `SELECT config FROM channel_installation WHERE workspace_id = $1 AND channel_type = 'weixin'`, scope.WorkspaceID).Scan(&config); err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(config), operatorBotToken) || strings.Contains(output.String(), operatorBotToken) || strings.Contains(output.String(), "opaque-poll-token") {
		t.Fatal("operator exposed an authorization credential or polling token")
	}
	if !strings.Contains(output.String(), "https://weixin.qq.com/x/operator-scan") || !strings.Contains(output.String(), "Installation ID:") {
		t.Fatalf("missing scan URL or safe success receipt: %s", output.String())
	}
	box, err := secretbox.New(make([]byte, secretbox.KeySize))
	if err != nil {
		t.Fatal(err)
	}
	credentials, err := weixin.DecodeCredentials(config, box.Open)
	if err != nil || credentials.BotToken != operatorBotToken {
		t.Fatalf("stored token cannot be decrypted: %v", err)
	}
}

func TestWeixinOperatorFollowsRegionRedirectAndVerificationChallenge(t *testing.T) {
	pool, _, scope := operatorFixture(t)
	step := 0
	operatorProvider(t, func(w http.ResponseWriter, r *http.Request) {
		step++
		switch step {
		case 1:
			_, _ = io.WriteString(w, `{"status":"scaned_but_redirect","redirect_host":"region.weixin.qq.com"}`)
		case 2:
			if r.URL.Hostname() != "region.weixin.qq.com" {
				t.Errorf("redirect polling host = %q", r.URL.Hostname())
			}
			_, _ = io.WriteString(w, `{"status":"need_verifycode"}`)
		case 3:
			if r.URL.Query().Get("verify_code") != "111111" {
				t.Errorf("first verification code was not submitted")
			}
			_, _ = io.WriteString(w, `{"status":"need_verifycode"}`)
		case 4:
			if r.URL.Query().Get("verify_code") != "123456" {
				t.Errorf("verification code was not submitted")
			}
			confirmedOperatorResponse(w)
		default:
			t.Fatalf("unexpected poll %d", step)
		}
	})
	var output bytes.Buffer
	if _, err := AuthorizeWeixin(t.Context(), pool, serverConfigured, scope, strings.NewReader("not-digits\n111111\n123456\n"), &output); err != nil {
		t.Fatal(err)
	}
	if strings.Contains(output.String(), "123456") || strings.Contains(output.String(), "111111") || strings.Contains(output.String(), operatorBotToken) {
		t.Fatal("operator echoed verification or bot credentials")
	}
	var baseURL string
	if err := pool.QueryRow(t.Context(), `SELECT config->>'base_url' FROM channel_installation WHERE workspace_id = $1 AND channel_type = 'weixin'`, scope.WorkspaceID).Scan(&baseURL); err != nil {
		t.Fatal(err)
	}
	if baseURL != "https://region.weixin.qq.com" {
		t.Fatalf("installed regional base URL = %q", baseURL)
	}
}

func TestWeixinOperatorTerminalFailuresDoNotPersist(t *testing.T) {
	for _, test := range []struct {
		name, response, input string
		cancel                bool
	}{
		{name: "expired", response: `{"status":"expired"}`},
		{name: "verification blocked", response: `{"status":"verify_code_blocked"}`},
		{name: "verification EOF", response: `{"status":"need_verifycode"}`},
		{name: "unsafe redirect", response: `{"status":"scaned_but_redirect","redirect_host":"localhost"}`},
		{name: "incomplete confirmation", response: `{"status":"confirmed","bot_token":"` + operatorBotToken + `"}`},
		{name: "already connected", response: `{"status":"binded_redirect"}`},
		{name: "cancelled", response: `{"status":"wait"}`, cancel: true},
		{name: "cancelled verification", response: `{"status":"need_verifycode"}`, cancel: true},
	} {
		t.Run(test.name, func(t *testing.T) {
			pool, fx, scope := operatorFixture(t)
			ctx, cancel := context.WithCancel(t.Context())
			defer cancel()
			operatorProvider(t, func(w http.ResponseWriter, r *http.Request) {
				_, _ = io.WriteString(w, test.response)
				if test.cancel {
					cancel()
				}
			})
			var output bytes.Buffer
			if _, err := AuthorizeWeixin(ctx, pool, serverConfigured, scope, strings.NewReader(test.input), &output); err == nil {
				t.Fatal("terminal authorization failure succeeded")
			}
			if got := fx.Count(t, `SELECT count(*) FROM channel_installation WHERE workspace_id = $1`, scope.WorkspaceID); got != 0 {
				t.Fatalf("failed authorization persisted %d installations", got)
			}
			if got := fx.Count(t, `SELECT count(*) FROM channel_user_binding WHERE workspace_id = $1`, scope.WorkspaceID); got != 0 {
				t.Fatalf("failed authorization persisted %d bindings", got)
			}
			if strings.Contains(output.String(), operatorBotToken) {
				t.Fatal("operator exposed provider credentials")
			}
		})
	}
}

func TestWeixinOperatorProviderErrorsDoNotLeakCredentialsOrPersist(t *testing.T) {
	for _, phase := range []string{"begin", "status"} {
		t.Run(phase, func(t *testing.T) {
			pool, fx, scope := operatorFixture(t)
			previous := http.DefaultTransport
			failed := false
			http.DefaultTransport = operatorRoundTripper(func(r *http.Request) (*http.Response, error) {
				w := httptest.NewRecorder()
				w.Header().Set("Content-Type", "application/json")
				if (phase == "begin" && r.URL.Path == "/ilink/bot/get_bot_qrcode") || (phase == "status" && r.URL.Path == "/ilink/bot/get_qrcode_status" && !failed) {
					failed = true
					_, _ = io.WriteString(w, `{"ret":500,"errmsg":"`+operatorBotToken+`"}`)
				} else if r.URL.Path == "/ilink/bot/get_bot_qrcode" {
					_, _ = io.WriteString(w, `{"qrcode":"opaque-poll-token","qrcode_img_content":"https://weixin.qq.com/x/operator-scan"}`)
				} else {
					_, _ = io.WriteString(w, `{"status":"expired"}`)
				}
				return w.Result(), nil
			})
			t.Cleanup(func() { http.DefaultTransport = previous })
			var output bytes.Buffer
			_, err := AuthorizeWeixin(t.Context(), pool, serverConfigured, scope, strings.NewReader(""), &output)
			if err == nil || !failed {
				t.Fatalf("provider failure was not exercised: %v", err)
			}
			if strings.Contains(err.Error(), operatorBotToken) || strings.Contains(output.String(), operatorBotToken) {
				t.Fatal("operator exposed a provider error's credential payload")
			}
			if got := fx.Count(t, `SELECT count(*) FROM channel_installation WHERE workspace_id = $1`, scope.WorkspaceID); got != 0 {
				t.Fatalf("provider failure persisted %d installations", got)
			}
		})
	}
}

func TestWeixinOperatorRechecksMembershipAtConfirmation(t *testing.T) {
	pool, fx, scope := operatorFixture(t)
	operatorProvider(t, func(w http.ResponseWriter, r *http.Request) {
		if _, err := pool.Exec(t.Context(), `DELETE FROM member WHERE workspace_id = $1 AND user_id = $2`, scope.WorkspaceID, scope.InstallerUserID); err != nil {
			t.Fatal(err)
		}
		confirmedOperatorResponse(w)
	})
	_, err := AuthorizeWeixin(t.Context(), pool, serverConfigured, scope, strings.NewReader(""), io.Discard)
	if err == nil || !strings.Contains(err.Error(), "authorization changed") {
		t.Fatalf("lost membership did not fence confirmation: %v", err)
	}
	if got := fx.Count(t, `SELECT count(*) FROM channel_installation WHERE workspace_id = $1`, scope.WorkspaceID); got != 0 {
		t.Fatalf("lost authorization persisted %d installations", got)
	}
}

func TestWeixinOperatorAgentOwnership(t *testing.T) {
	for _, test := range []struct {
		name, role string
		owned      bool
		archived   bool
		allowed    bool
	}{
		{name: "agent owner", role: "member", owned: true, allowed: true},
		{name: "workspace admin", role: "admin", allowed: true},
		{name: "another member", role: "member"},
		{name: "archived agent", role: "owner", owned: true, archived: true},
	} {
		t.Run(test.name, func(t *testing.T) {
			pool, fx, scope := operatorFixture(t)
			actorID := fx.User(t, "Agent Operator", "weixin-agent-operator@example.test")
			fx.Member(t, scope.WorkspaceID, actorID, test.role)
			fields := testutil.Cols{}
			if test.owned {
				fields["owner_id"] = actorID
			}
			if test.archived {
				fields["archived_at"] = testutil.Raw("now()")
			}
			scope.AgentID = fx.Agent(t, "Weixin Agent", "", fields)
			scope.InstallerUserID = actorID
			calls := operatorProvider(t, func(w http.ResponseWriter, r *http.Request) { confirmedOperatorResponse(w) })
			_, err := AuthorizeWeixin(t.Context(), pool, serverConfigured, scope, strings.NewReader(""), io.Discard)
			if (err == nil) != test.allowed {
				t.Fatalf("allowed = %v, error = %v", test.allowed, err)
			}
			if !test.allowed && *calls != 0 {
				t.Fatal("provider was called before agent permission validation")
			}
		})
	}
}

func TestWeixinOperatorRejectsUnauthorizedScopeBeforeQR(t *testing.T) {
	pool, fx, scope := operatorFixture(t)
	otherUser := fx.User(t, "Other Operator", "other-weixin-operator@example.test")
	fx.Member(t, scope.WorkspaceID, otherUser, "member")
	scope.InstallerUserID = otherUser
	calls := operatorProvider(t, func(w http.ResponseWriter, r *http.Request) { confirmedOperatorResponse(w) })
	if _, err := AuthorizeWeixin(t.Context(), pool, serverConfigured, scope, strings.NewReader(""), io.Discard); err == nil {
		t.Fatal("non-admin workspace installation was allowed")
	}
	if *calls != 0 {
		t.Fatal("provider was called before permission validation")
	}
}

func TestWeixinOperatorRequiresSelfHostedMode(t *testing.T) {
	for _, mode := range []string{"disabled", "managed", ""} {
		if _, err := AuthorizeWeixin(t.Context(), nil, mode, WeixinAuthorizationScope{}, strings.NewReader(""), io.Discard); err == nil {
			t.Fatalf("operator accepted mode %q", mode)
		}
	}
}
