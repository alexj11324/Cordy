// Package weixin implements the native Tencent personal-WeChat iLink adapter.
//
// Weixin is deliberately separate from the WeCom adapter: WeCom is a
// corporate smart-bot WebSocket, while iLink is a QR-bound personal bot with
// HTTP long polling. Both adapters use the shared channel_* tables and the
// channel engine; this package owns only iLink wire/config details.
package weixin

import (
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"strings"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
)

// TypeWeixin is the durable channel discriminator used in channel_installation
// and every generic channel binding/event row.
const TypeWeixin channel.Type = "weixin"

// OriginWeixinChat is the issue origin label for /issue turns received over
// iLink. It is intentionally a channel-specific string, not a new schema.
const OriginWeixinChat = "weixin_chat"

// DefaultBaseURL is the only provider host documented by the Rust mainline
// adapter. Per-installation redirect hosts are accepted only when they pass
// ValidateProviderBaseURL.
const DefaultBaseURL = "https://ilinkai.weixin.qq.com"

// Decrypter opens the secretbox ciphertext stored in channel_installation.
// The nil behavior is useful only for fixture tests; production wiring always
// supplies secretbox.Box.Open and refuses plaintext storage at installation.
type Decrypter func(ciphertext []byte) ([]byte, error)

type installConfig struct {
	AppID             string `json:"app_id"`
	ILinkUserID       string `json:"ilink_user_id"`
	BaseURL           string `json:"base_url"`
	BotTokenEncrypted string `json:"bot_token_encrypted"`
}

// Credentials is the decrypted runtime config for one iLink installation.
type Credentials struct {
	BotID       string
	ILinkUserID string
	BaseURL     string
	BotToken    string
}

// PublicConfig is the secret-free management projection.
type PublicConfig struct {
	BotID       string
	ILinkUserID string
}

var errMissingCredentials = errors.New("weixin: installation is missing bot credentials")

// DecodeCredentials parses the exact config shape written by the Rust
// mainline installer and decrypts bot_token_encrypted. No provider-specific
// token format is inferred here: iLink returns the opaque bot token at QR
// confirmation time.
func DecodeCredentials(raw json.RawMessage, decrypt Decrypter) (Credentials, error) {
	if len(raw) == 0 {
		return Credentials{}, errors.New("weixin: empty installation config")
	}
	var cfg installConfig
	if err := json.Unmarshal(raw, &cfg); err != nil {
		return Credentials{}, fmt.Errorf("decode weixin installation config: %w", err)
	}
	ciphertext, err := base64.StdEncoding.DecodeString(stripWhitespace(cfg.BotTokenEncrypted))
	if err != nil {
		return Credentials{}, fmt.Errorf("decode weixin bot token: %w", err)
	}
	plaintext := ciphertext
	if decrypt != nil {
		plaintext, err = decrypt(ciphertext)
		if err != nil {
			return Credentials{}, fmt.Errorf("decrypt weixin bot token: %w", err)
		}
	}
	if cfg.AppID == "" || len(plaintext) == 0 {
		return Credentials{}, errMissingCredentials
	}
	baseURL, err := ValidateProviderBaseURL(cfg.BaseURL)
	if err != nil {
		return Credentials{}, fmt.Errorf("validate weixin installation base url: %w", err)
	}
	return Credentials{
		BotID:       cfg.AppID,
		ILinkUserID: cfg.ILinkUserID,
		BaseURL:     baseURL,
		BotToken:    string(plaintext),
	}, nil
}

// DecodePublicConfig extracts only the non-secret fields. Management list
// endpoints use this instead of DecodeCredentials so they never need to open
// a token merely to render an installation row.
func DecodePublicConfig(raw json.RawMessage) PublicConfig {
	var cfg installConfig
	_ = json.Unmarshal(raw, &cfg)
	return PublicConfig{BotID: cfg.AppID, ILinkUserID: cfg.ILinkUserID}
}

func encodeInstallConfig(botID, userID, baseURL, encryptedToken string) ([]byte, error) {
	if strings.TrimSpace(botID) == "" || strings.TrimSpace(userID) == "" || strings.TrimSpace(encryptedToken) == "" {
		return nil, errMissingCredentials
	}
	validatedBaseURL, err := ValidateProviderBaseURL(baseURL)
	if err != nil {
		return nil, err
	}
	return json.Marshal(installConfig{
		AppID:             strings.TrimSpace(botID),
		ILinkUserID:       strings.TrimSpace(userID),
		BaseURL:           validatedBaseURL,
		BotTokenEncrypted: encryptedToken,
	})
}

func normalizeBaseURL(value string) string {
	value = strings.TrimRight(strings.TrimSpace(value), "/")
	if value == "" {
		return DefaultBaseURL
	}
	return value
}

// ValidateProviderBaseURL applies the redirect-host boundary from the Rust
// adapter. iLink may return a regional redirect host, but it must remain an
// HTTPS host under the documented Tencent Weixin domain. Paths and queries
// are discarded so a provider-supplied callback cannot smuggle an arbitrary
// request target into later API calls.
func ValidateProviderBaseURL(value string) (string, error) {
	value = strings.TrimSpace(value)
	if value == "" {
		return DefaultBaseURL, nil
	}
	if !strings.Contains(value, "://") {
		value = "https://" + value
	}
	u, err := url.Parse(value)
	if err != nil || u.Scheme != "https" || u.Hostname() == "" || u.User != nil {
		return "", errors.New("weixin: provider base url must be an https host")
	}
	if u.Port() != "" && u.Port() != "443" {
		return "", errors.New("weixin: provider base url may only use port 443")
	}
	host := strings.ToLower(strings.TrimSuffix(u.Hostname(), "."))
	if host != "ilinkai.weixin.qq.com" && !strings.HasSuffix(host, ".weixin.qq.com") {
		return "", fmt.Errorf("weixin: provider base url host %q is not allowed", host)
	}
	return "https://" + host, nil
}

func stripWhitespace(value string) string {
	var b strings.Builder
	b.Grow(len(value))
	for _, r := range value {
		switch r {
		case ' ', '\t', '\r', '\n':
			continue
		default:
			b.WriteRune(r)
		}
	}
	return b.String()
}
