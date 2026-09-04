// Package messagingbootstrap materializes self-hosted messaging credentials
// before the channel supervisor starts. Browser setup remains read-only in
// server_configured mode; operators opt in explicitly with the bootstrap flag.
package messagingbootstrap

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"strings"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/patchbay-ai/patchbay/server/internal/util"
	"github.com/patchbay-ai/patchbay/server/internal/util/secretbox"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

const (
	bootstrapFlag    = "PATCHBAY_MESSAGING_BOOTSTRAP"
	workspaceIDEnv   = "PATCHBAY_MESSAGING_WORKSPACE_ID"
	installerUserEnv = "PATCHBAY_MESSAGING_INSTALLER_USER_ID"
	agentIDEnv       = "PATCHBAY_MESSAGING_AGENT_ID"
	serverConfigured = "server_configured"
)

type scope struct {
	workspaceID     pgtype.UUID
	agentID         pgtype.UUID
	installerUserID pgtype.UUID
}

type installationSpec struct {
	provider    string
	channelType string
	appID       string
	config      []byte
}

// ProvisionFromEnvironment applies the server's opt-in credential bootstrap
// contract. It is idempotent and must run before the channel
// supervisor so the first sweep can discover the resulting installation rows.
func ProvisionFromEnvironment(ctx context.Context, pool *pgxpool.Pool, mode string) error {
	if mode != serverConfigured {
		return nil
	}
	flagValue, flagPresent := envValue(bootstrapFlag)
	enabled, err := parseEnvFlag(bootstrapFlag, flagValue, flagPresent)
	if err != nil || !enabled {
		return err
	}
	specs, err := installationSpecs()
	if err != nil {
		return err
	}
	if len(specs) == 0 {
		slog.Info("self-hosted messaging bootstrap enabled but no provider credentials are configured")
		return nil
	}
	bootstrapScope, err := scopeFromEnvironment()
	if err != nil {
		return err
	}
	for _, spec := range specs {
		if err := persistInstallation(ctx, pool, bootstrapScope, spec); err != nil {
			return fmt.Errorf("bootstrap %s installation: %w", spec.provider, err)
		}
		slog.Info("self-hosted messaging installation bootstrapped", "provider", spec.provider)
	}
	return nil
}

func scopeFromEnvironment() (scope, error) {
	workspaceID, err := requiredUUID(workspaceIDEnv)
	if err != nil {
		return scope{}, err
	}
	installerUserID, err := requiredUUID(installerUserEnv)
	if err != nil {
		return scope{}, err
	}
	agentID, err := optionalUUID(agentIDEnv)
	if err != nil {
		return scope{}, err
	}
	return scope{workspaceID: workspaceID, agentID: agentID, installerUserID: installerUserID}, nil
}

func installationSpecs() ([]installationSpec, error) {
	builders := []func() (*installationSpec, error){slackSpec, telegramSpec, larkSpec, dingtalkSpec, wecomSpec}
	specs := make([]installationSpec, 0, len(builders))
	for _, build := range builders {
		spec, err := build()
		if err != nil {
			return nil, err
		}
		if spec != nil {
			specs = append(specs, *spec)
		}
	}
	return specs, nil
}

func slackSpec() (*installationSpec, error) {
	botToken, botOK := envValue("SLACK_BOT_TOKEN")
	appToken, appOK := envValue("SLACK_APP_TOKEN")
	if !botOK && !appOK {
		return nil, nil
	}
	if !botOK || !appOK {
		return nil, errors.New("SLACK_BOT_TOKEN and SLACK_APP_TOKEN must be configured together")
	}
	if !strings.HasPrefix(botToken, "xoxb-") {
		return nil, errors.New("SLACK_BOT_TOKEN must start with xoxb-")
	}
	if !strings.HasPrefix(appToken, "xapp-") {
		return nil, errors.New("SLACK_APP_TOKEN must start with xapp-")
	}
	parsedAppID := parseSlackAppID(appToken)
	appID, appIDOK := envValue("SLACK_APP_ID")
	if !appIDOK {
		appID = parsedAppID
	}
	if !strings.HasPrefix(appID, "A") || len(appID) < 2 {
		return nil, errors.New("SLACK_APP_ID is missing or invalid")
	}
	if parsedAppID != "" && parsedAppID != appID {
		return nil, errors.New("SLACK_APP_ID must match the app id in SLACK_APP_TOKEN")
	}
	teamID, err := requiredValue("SLACK_TEAM_ID")
	if err != nil {
		return nil, err
	}
	botUserID, err := requiredValue("SLACK_BOT_USER_ID")
	if err != nil {
		return nil, err
	}
	botEncrypted, err := sealBase64("PATCHBAY_SLACK_SECRET_KEY", botToken)
	if err != nil {
		return nil, err
	}
	appEncrypted, err := sealBase64("PATCHBAY_SLACK_SECRET_KEY", appToken)
	if err != nil {
		return nil, err
	}
	return marshalSpec("slack", "slack", appID, map[string]any{
		"app_id": appID, "team_id": teamID, "bot_user_id": botUserID,
		"bot_token_encrypted": botEncrypted, "app_token_encrypted": appEncrypted,
	})
}

func telegramSpec() (*installationSpec, error) {
	botToken, ok := envValue("TELEGRAM_BOT_TOKEN")
	if !ok {
		return nil, nil
	}
	botID, secret, found := strings.Cut(botToken, ":")
	if !found || botID == "" || secret == "" {
		return nil, errors.New("TELEGRAM_BOT_TOKEN has an invalid bot token shape")
	}
	for _, r := range botID {
		if r < '0' || r > '9' {
			return nil, errors.New("TELEGRAM_BOT_TOKEN has an invalid bot token shape")
		}
	}
	encrypted, err := sealBase64("PATCHBAY_TELEGRAM_SECRET_KEY", botToken)
	if err != nil {
		return nil, err
	}
	username, _ := envValue("TELEGRAM_BOT_USERNAME")
	return marshalSpec("telegram", "telegram", botID, map[string]any{
		"app_id": botID, "bot_username": username, "bot_token_encrypted": encrypted,
	})
}

func larkSpec() (*installationSpec, error) {
	appID, appOK := envValue("LARK_APP_ID")
	appSecret, secretOK := envValue("LARK_APP_SECRET")
	if !appOK && !secretOK {
		return nil, nil
	}
	if !appOK || !secretOK {
		return nil, errors.New("LARK_APP_ID and LARK_APP_SECRET must be configured together")
	}
	encrypted, err := sealBase64("PATCHBAY_LARK_SECRET_KEY", appSecret)
	if err != nil {
		return nil, err
	}
	tenantKey, _ := envValue("LARK_TENANT_KEY")
	botOpenID, _ := envValue("LARK_BOT_OPEN_ID")
	botUnionID, _ := envValue("LARK_BOT_UNION_ID")
	region, ok := envValue("LARK_REGION")
	if !ok {
		region = "feishu"
	}
	return marshalSpec("lark", "feishu", appID, map[string]any{
		"app_id": appID, "app_secret_encrypted": encrypted, "tenant_key": tenantKey,
		"bot_open_id": botOpenID, "bot_union_id": botUnionID, "region": region,
	})
}

func dingtalkSpec() (*installationSpec, error) {
	appID, appOK := envValue("DINGTALK_CLIENT_ID")
	appSecret, secretOK := envValue("DINGTALK_CLIENT_SECRET")
	if !appOK && !secretOK {
		return nil, nil
	}
	if !appOK || !secretOK {
		return nil, errors.New("DINGTALK_CLIENT_ID and DINGTALK_CLIENT_SECRET must be configured together")
	}
	encrypted, err := sealBase64("PATCHBAY_DINGTALK_SECRET_KEY", appSecret)
	if err != nil {
		return nil, err
	}
	robotCode, ok := envValue("DINGTALK_ROBOT_CODE")
	if !ok {
		robotCode = appID
	}
	return marshalSpec("dingtalk", "dingtalk", appID, map[string]any{
		"app_id": appID, "robot_code": robotCode, "app_secret_encrypted": encrypted,
	})
}

func wecomSpec() (*installationSpec, error) {
	botID, botOK := envValue("WECOM_BOT_ID")
	secret, secretOK := envValue("WECOM_SECRET")
	if !botOK && !secretOK {
		return nil, nil
	}
	if !botOK || !secretOK {
		return nil, errors.New("WECOM_BOT_ID and WECOM_SECRET must be configured together")
	}
	sealed, err := sealBytes("PATCHBAY_WECOM_SECRET_KEY", secret)
	if err != nil {
		return nil, err
	}
	botName, _ := envValue("WECOM_BOT_NAME")
	return marshalSpec("wecom", "wecom", botID, map[string]any{
		"app_id": botID, "bot_id": botID,
		"secret_encrypted": base64.StdEncoding.EncodeToString(sealed), "bot_display_name": botName,
	})
}

func marshalSpec(provider, channelType, appID string, config map[string]any) (*installationSpec, error) {
	raw, err := json.Marshal(config)
	if err != nil {
		return nil, fmt.Errorf("encode %s bootstrap config: %w", provider, err)
	}
	return &installationSpec{provider: provider, channelType: channelType, appID: appID, config: raw}, nil
}

func persistInstallation(ctx context.Context, pool *pgxpool.Pool, bootstrapScope scope, spec installationSpec) error {
	tx, err := pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	q := db.New(tx)
	scopeKey := "hub"
	if bootstrapScope.agentID.Valid {
		scopeKey = util.UUIDToString(bootstrapScope.agentID)
	}
	if _, err := tx.Exec(ctx, `SELECT pg_advisory_xact_lock(hashtext($1), hashtext($2))`,
		"channel_installation:"+spec.channelType,
		util.UUIDToString(bootstrapScope.workspaceID)+":"+scopeKey,
	); err != nil {
		return fmt.Errorf("lock installation scope: %w", err)
	}
	if err := q.LockChannelInstallationAppIDSlot(ctx, db.LockChannelInstallationAppIDSlotParams{
		ChannelType: spec.channelType, AppID: spec.appID,
	}); err != nil {
		return fmt.Errorf("lock installation app id: %w", err)
	}
	if _, err := q.ReclaimDeadChannelInstallationByAppID(ctx, db.ReclaimDeadChannelInstallationByAppIDParams{
		ChannelType: spec.channelType, AppID: spec.appID,
		WorkspaceID: bootstrapScope.workspaceID, AgentID: bootstrapScope.agentID,
	}); err != nil && !errors.Is(err, pgx.ErrNoRows) {
		return fmt.Errorf("reclaim dead installation: %w", err)
	}
	if bootstrapScope.agentID.Valid {
		_, err = q.UpsertChannelInstallation(ctx, db.UpsertChannelInstallationParams{
			WorkspaceID: bootstrapScope.workspaceID, AgentID: bootstrapScope.agentID,
			ChannelType: spec.channelType, Config: spec.config, InstallerUserID: bootstrapScope.installerUserID,
		})
	} else {
		_, err = q.UpsertChannelInstallationHub(ctx, db.UpsertChannelInstallationHubParams{
			WorkspaceID: bootstrapScope.workspaceID, ChannelType: spec.channelType,
			Config: spec.config, InstallerUserID: bootstrapScope.installerUserID,
		})
	}
	if err != nil {
		return fmt.Errorf("upsert installation: %w", err)
	}
	return tx.Commit(ctx)
}

func sealBase64(keyEnv, value string) (string, error) {
	sealed, err := sealBytes(keyEnv, value)
	if err != nil {
		return "", err
	}
	return base64.StdEncoding.EncodeToString(sealed), nil
}

func sealBytes(keyEnv, value string) ([]byte, error) {
	key, err := secretbox.LoadKey(keyEnv)
	if err != nil {
		return nil, fmt.Errorf("load %s: %w", keyEnv, err)
	}
	box, err := secretbox.New(key)
	if err != nil {
		return nil, fmt.Errorf("initialize %s: %w", keyEnv, err)
	}
	sealed, err := box.Seal([]byte(value))
	if err != nil {
		return nil, fmt.Errorf("encrypt value for %s: %w", keyEnv, err)
	}
	return sealed, nil
}

func envValue(name string) (string, bool) {
	value := strings.TrimSpace(os.Getenv(name))
	return value, value != ""
}

func requiredValue(name string) (string, error) {
	value, ok := envValue(name)
	if !ok {
		return "", fmt.Errorf("%s must be configured", name)
	}
	return value, nil
}

func requiredUUID(name string) (pgtype.UUID, error) {
	value, err := requiredValue(name)
	if err != nil {
		return pgtype.UUID{}, err
	}
	parsed, err := util.ParseUUID(value)
	if err != nil {
		return pgtype.UUID{}, fmt.Errorf("%s must be a UUID", name)
	}
	return parsed, nil
}

func optionalUUID(name string) (pgtype.UUID, error) {
	value, ok := envValue(name)
	if !ok {
		return pgtype.UUID{}, nil
	}
	parsed, err := util.ParseUUID(value)
	if err != nil {
		return pgtype.UUID{}, fmt.Errorf("%s must be a UUID", name)
	}
	return parsed, nil
}

func parseEnvFlag(name string, value string, present bool) (bool, error) {
	if !present || value == "0" || value == "false" || value == "no" {
		return false, nil
	}
	if value == "1" || value == "true" || value == "yes" {
		return true, nil
	}
	return false, fmt.Errorf("%s must be true or false, got %q", name, value)
}

func parseSlackAppID(token string) string {
	fields := strings.Split(token, "-")
	if len(fields) < 3 || fields[0] != "xapp" || !strings.HasPrefix(fields[2], "A") || len(fields[2]) < 2 {
		return ""
	}
	return fields[2]
}
