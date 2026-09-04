package messagingbootstrap

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"strings"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/util/secretbox"
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
	t.Setenv("PATCHBAY_SLACK_SECRET_KEY", base64.StdEncoding.EncodeToString(key))
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

func clearProviderCredentials(t *testing.T) {
	t.Helper()
	for _, name := range []string{
		"SLACK_BOT_TOKEN", "SLACK_APP_TOKEN", "SLACK_APP_ID", "SLACK_TEAM_ID", "SLACK_BOT_USER_ID",
		"TELEGRAM_BOT_TOKEN", "TELEGRAM_BOT_USERNAME",
		"LARK_APP_ID", "LARK_APP_SECRET", "LARK_TENANT_KEY", "LARK_BOT_OPEN_ID", "LARK_BOT_UNION_ID", "LARK_REGION",
		"DINGTALK_CLIENT_ID", "DINGTALK_CLIENT_SECRET", "DINGTALK_ROBOT_CODE",
		"WECOM_BOT_ID", "WECOM_SECRET", "WECOM_BOT_NAME",
	} {
		t.Setenv(name, "")
	}
}
