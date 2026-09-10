package messagingbootstrap

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"os"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/weixin"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	"github.com/orvilo-ai/orvilo/server/internal/util/secretbox"
)

func TestParseSlackAppID(t *testing.T) {
	if got := parseSlackAppID("xapp-1-A123-456"); got != "A123" {
		t.Fatalf("parseSlackAppID = %q, want A123", got)
	}
	for _, token := range []string{"xoxb-123", "xapp-1-B123-456", "xapp-1-"} {
		if got := parseSlackAppID(token); got != "" {
			t.Fatalf("parseSlackAppID(%q) = %q, want empty", token, got)
		}
	}
}

func TestParseEnvFlag(t *testing.T) {
	for _, value := range []string{"0", "false", "no"} {
		got, err := parseEnvFlag(bootstrapFlag, value, true)
		if err != nil || got {
			t.Fatalf("parseEnvFlag(%q) = %v, %v; want false, nil", value, got, err)
		}
	}
	if got, err := parseEnvFlag(bootstrapFlag, "", false); err != nil || got {
		t.Fatalf("missing parseEnvFlag = %v, %v; want false, nil", got, err)
	}
	if _, err := parseEnvFlag(bootstrapFlag, "sometimes", true); err == nil {
		t.Fatal("invalid bootstrap flag should fail")
	}
}

func TestBootstrapWithNoProviderCredentialsDoesNotRequireScope(t *testing.T) {
	clearProviderCredentials(t)
	t.Setenv(bootstrapFlag, "true")
	t.Setenv(workspaceIDEnv, "")
	t.Setenv(installerUserEnv, "")

	if err := ProvisionFromEnvironment(context.Background(), nil, serverConfigured); err != nil {
		t.Fatalf("ProvisionFromEnvironment with no credentials: %v", err)
	}
}

func TestSlackSpecEncryptsCredentialsAndPreservesRoutingIdentity(t *testing.T) {
	clearProviderCredentials(t)
	key := make([]byte, secretbox.KeySize)
	t.Setenv("ORVILO_SLACK_SECRET_KEY", base64.StdEncoding.EncodeToString(key))
	t.Setenv("SLACK_BOT_TOKEN", "xoxb-secret")
	appToken := strings.Join([]string{"xapp", "1", "A123", "456", "fixture"}, "-")
	t.Setenv("SLACK_APP_TOKEN", appToken)
	t.Setenv("SLACK_TEAM_ID", "T123")
	t.Setenv("SLACK_BOT_USER_ID", "U123")

	spec, err := slackSpec()
	if err != nil {
		t.Fatalf("slackSpec: %v", err)
	}
	if spec == nil || spec.appID != "A123" || spec.channelType != "slack" {
		t.Fatalf("slackSpec identity = %+v", spec)
	}
	if strings.Contains(string(spec.config), "xoxb-secret") || strings.Contains(string(spec.config), appToken) {
		t.Fatal("bootstrap config contains plaintext credentials")
	}
	var config map[string]string
	if err := json.Unmarshal(spec.config, &config); err != nil {
		t.Fatalf("decode config: %v", err)
	}
	if config["team_id"] != "T123" || config["bot_user_id"] != "U123" {
		t.Fatalf("routing config = %#v", config)
	}
	box, err := secretbox.New(key)
	if err != nil {
		t.Fatal(err)
	}
	sealed, err := base64.StdEncoding.DecodeString(config["bot_token_encrypted"])
	if err != nil {
		t.Fatal(err)
	}
	plaintext, err := box.Open(sealed)
	if err != nil || string(plaintext) != "xoxb-secret" {
		t.Fatalf("decrypted bot token = %q, %v", plaintext, err)
	}
}

func TestWeixinSpecEncryptsServerCredentials(t *testing.T) {
	clearProviderCredentials(t)
	key := make([]byte, secretbox.KeySize)
	t.Setenv("ORVILO_WEIXIN_SECRET_KEY", base64.StdEncoding.EncodeToString(key))
	t.Setenv("WEIXIN_BOT_ID", "weixin-bot")
	t.Setenv("WEIXIN_ILINK_USER_ID", "weixin-user")
	t.Setenv("WEIXIN_BOT_TOKEN", "opaque-weixin-token")
	t.Setenv("WEIXIN_BASE_URL", "https://ilinkai.weixin.qq.com")
	specs, err := installationSpecs()
	if err != nil {
		t.Fatal(err)
	}
	if len(specs) != 1 || specs[0].channelType != "weixin" {
		t.Fatalf("Weixin bootstrap omitted: specs=%+v", specs)
	}
	if strings.Contains(string(specs[0].config), "opaque-weixin-token") {
		t.Fatal("bootstrap config contains plaintext credentials")
	}
	box, err := secretbox.New(key)
	if err != nil {
		t.Fatal(err)
	}
	creds, err := weixin.DecodeCredentials(specs[0].config, box.Open)
	if err != nil || creds.BotID != "weixin-bot" || creds.ILinkUserID != "weixin-user" || creds.BotToken != "opaque-weixin-token" {
		t.Fatalf("invalid runtime credentials after bootstrap: %+v, %v", creds, err)
	}
}

func TestWeixinSpecRejectsPartialCredentialsAndUnsafeHosts(t *testing.T) {
	clearProviderCredentials(t)
	t.Setenv("WEIXIN_BOT_TOKEN", "opaque-token")
	if _, err := installationSpecs(); err == nil || !strings.Contains(err.Error(), "must be configured together") {
		t.Fatalf("partial Weixin credentials accepted: %v", err)
	}
	t.Setenv("WEIXIN_BOT_ID", "bot")
	t.Setenv("WEIXIN_ILINK_USER_ID", "user")
	t.Setenv("WEIXIN_BASE_URL", "https://untrusted.example.com")
	if _, err := installationSpecs(); err == nil || !strings.Contains(err.Error(), "WEIXIN_BASE_URL") {
		t.Fatalf("unsafe Weixin provider host accepted: %v", err)
	}
}

func TestBootstrapSkipsHostedAndDisabledModes(t *testing.T) {
	t.Setenv(bootstrapFlag, "true")
	for _, mode := range []string{"managed", "disabled"} {
		if err := ProvisionFromEnvironment(t.Context(), nil, mode); err != nil {
			t.Fatalf("%s tried to apply server-owned bootstrap: %v", mode, err)
		}
	}
}

func TestBootstrapAllSixProvidersPersistsIdempotently(t *testing.T) {
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
	userID := fx.User(t, "Messaging Bootstrap", "six-platform-bootstrap@example.com")
	workspaceID := fx.Workspace(t, "Messaging Bootstrap", "six-platform-bootstrap")
	fx.Member(t, workspaceID, userID, "owner")
	fx.Cleanup(t, `DELETE FROM channel_installation WHERE workspace_id = $1`, workspaceID)
	clearProviderCredentials(t)
	for name, value := range map[string]string{
		bootstrapFlag: "true", workspaceIDEnv: workspaceID, installerUserEnv: userID, agentIDEnv: "",
		"SLACK_BOT_TOKEN": "xoxb-bootstrap", "SLACK_APP_TOKEN": "xapp-1-ABOOTSTRAP-fixture",
		"SLACK_TEAM_ID": "TBOOTSTRAP", "SLACK_BOT_USER_ID": "UBOOTSTRAP",
		"TELEGRAM_BOT_TOKEN": "123456:bootstrap-fixture",
		"LARK_APP_ID":        "cli_bootstrap", "LARK_APP_SECRET": "lark-bootstrap-secret",
		"DINGTALK_CLIENT_ID": "dingtalk-bootstrap", "DINGTALK_CLIENT_SECRET": "dingtalk-bootstrap-secret",
		"WECOM_BOT_ID": "wecom-bootstrap", "WECOM_SECRET": "wecom-bootstrap-secret",
		"WEIXIN_BOT_ID": "weixin-bootstrap", "WEIXIN_ILINK_USER_ID": "weixin-user", "WEIXIN_BOT_TOKEN": "weixin-bootstrap-token",
	} {
		t.Setenv(name, value)
	}
	for _, key := range []string{
		"ORVILO_LARK_SECRET_KEY", "ORVILO_SLACK_SECRET_KEY", "ORVILO_DINGTALK_SECRET_KEY",
		"ORVILO_WECOM_SECRET_KEY", "ORVILO_TELEGRAM_SECRET_KEY", "ORVILO_WEIXIN_SECRET_KEY",
	} {
		t.Setenv(key, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
	}
	for range 2 {
		if err := ProvisionFromEnvironment(t.Context(), pool, serverConfigured); err != nil {
			t.Fatal(err)
		}
	}
	for _, kind := range []string{"feishu", "slack", "dingtalk", "wecom", "telegram", "weixin"} {
		count := fx.Count(t, `SELECT count(*) FROM channel_installation WHERE workspace_id = $1 AND channel_type = $2 AND status = 'installed'`, workspaceID, kind)
		if count != 1 {
			t.Errorf("%s installed rows=%d, want 1", kind, count)
		}
	}
}

func clearProviderCredentials(t *testing.T) {
	t.Helper()
	for _, name := range []string{
		"SLACK_BOT_TOKEN", "SLACK_APP_TOKEN", "SLACK_APP_ID", "SLACK_TEAM_ID", "SLACK_BOT_USER_ID",
		"TELEGRAM_BOT_TOKEN", "TELEGRAM_BOT_USERNAME",
		"LARK_APP_ID", "LARK_APP_SECRET", "LARK_TENANT_KEY", "LARK_BOT_OPEN_ID", "LARK_BOT_UNION_ID", "LARK_REGION",
		"DINGTALK_CLIENT_ID", "DINGTALK_CLIENT_SECRET", "DINGTALK_ROBOT_CODE",
		"WECOM_BOT_ID", "WECOM_SECRET", "WECOM_BOT_NAME",
		"WEIXIN_BOT_ID", "WEIXIN_ILINK_USER_ID", "WEIXIN_BOT_TOKEN", "WEIXIN_BASE_URL",
	} {
		t.Setenv(name, "")
	}
}
