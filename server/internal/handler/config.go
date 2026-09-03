package handler

import (
	"net"
	"net/http"
	"net/url"
	"os"
	"strings"

	"github.com/patchbay-ai/patchbay/server/internal/analytics"
	"github.com/patchbay-ai/patchbay/server/internal/featureflags"
	"github.com/patchbay-ai/patchbay/server/internal/util/secretbox"
)

type MessagingCapabilities struct {
	Mode          string                        `json:"mode"`
	SetupWritable bool                          `json:"setupWritable"`
	Platforms     []MessagingPlatformCapability `json:"platforms"`
}

type MessagingPlatformCapability struct {
	Type         string `json:"type"`
	Enabled      bool   `json:"enabled"`
	Experimental bool   `json:"experimental"`
}

type AppConfig struct {
	CdnDomain string `json:"cdn_domain"`
	// CdnSigned tells clients that the CDN domain above serves PRIVATE
	// content through time-bounded signed URLs (CloudFront signing is
	// enabled). When true, a raw storage URL on the CDN domain is NOT
	// publicly fetchable — renderers must not pick it as a native
	// <img>/<video> source and should fall back to the per-attachment
	// API endpoint or a freshly signed download_url instead (MUL-3254).
	// Omitted when false so older clients see the previous shape.
	CdnSigned bool `json:"cdn_signed,omitempty"`
	// Public auth config consumed by the web app at runtime so self-hosted
	// deployments do not need to rebuild the frontend image when operators
	// toggle signup or wire Google OAuth.
	AllowSignup    bool   `json:"allow_signup"`
	GoogleClientID string `json:"google_client_id,omitempty"`
	// WorkspaceCreationDisabled mirrors the server-side
	// DISABLE_WORKSPACE_CREATION env var so the UI can hide every
	// "Create workspace" affordance on self-hosted instances. Omitted
	// from the JSON when false to keep responses identical to the
	// previous shape for the common managed-cloud case (#3433).
	WorkspaceCreationDisabled bool `json:"workspace_creation_disabled,omitempty"`
	// Public daemon setup config consumed by the web app at runtime so
	// self-hosted instances can show `patchbay setup self-host` commands
	// with the operator's own domains instead of Patchbay Cloud defaults.
	DaemonServerURL string `json:"daemon_server_url,omitempty"`
	DaemonAppURL    string `json:"daemon_app_url,omitempty"`

	// VCSIntegrationAvailable mirrors the PATCHBAY_VCS_INTEGRATION_ENABLED
	// deployment switch so the Settings UI can hide the whole self-hosted Git
	// provider section on deployments where it is off (the managed cloud),
	// instead of rendering it and surfacing an operator-only "missing
	// PATCHBAY_VCS_SECRET_KEY" hint a cloud user cannot resolve. Omitted when
	// false so the managed-cloud response keeps its previous shape; the UI
	// defaults absent to false (hidden).
	VCSIntegrationAvailable bool `json:"vcs_integration_available,omitempty"`

	// PostHog public config for the frontend. The key is the same Project
	// API Key the backend uses; returning it here (instead of baking it
	// into the frontend bundle via NEXT_PUBLIC_*) means self-hosted
	// instances — whose server returns an empty key — automatically
	// disable frontend event shipping too.
	PosthogKey           string `json:"posthog_key"`
	PosthogHost          string `json:"posthog_host"`
	AnalyticsEnvironment string `json:"analytics_environment"`

	// FeatureFlags exposes only frontend-safe boolean decisions. Do not dump
	// raw rules here: /api/config is public and may be called anonymously.
	FeatureFlags map[string]bool `json:"feature_flags,omitempty"`

	// LocalWorktreeSupported tells clients this server understands
	// local_directory `execution_mode` and enforces the worktree capability
	// gate when a resource is saved.
	//
	// Load-bearing for CLIENTS, not for this server. Releases before v0.4.25
	// unmarshalled the ref into a struct without the field and re-marshalled
	// it, so `execution_mode: "worktree"` was silently DROPPED and answered
	// 201 — the resource then ran in_place, editing the working copy the user
	// asked to isolate, with no gate anywhere to catch it. A new client cannot
	// tell that from success, so it has to ask first, and absent has to read as
	// "cannot honour it": every release that drops the field also omits this
	// one. Releases between that fix and this signal do gate the save but say
	// nothing, so they are treated the same way — the client cannot distinguish
	// them, and only one of the two guesses is safe.
	LocalWorktreeSupported bool `json:"local_worktree_supported"`

	// AgentConversationStartersSupported tells independently deployed clients
	// that agent create/update persists conversation_starters. Older handlers
	// ignored the unknown JSON field and still returned success, so clients
	// must fail closed when this declaration is absent.
	AgentConversationStartersSupported bool `json:"agent_conversation_starters_supported"`

	// ServerVersion is the running API build version, so self-hosted
	// operators can confirm what's deployed and include it in bug reports.
	// Only emitted on self-hosted deployments — omitted on the managed cloud,
	// which is continuously deployed so its users can't act on the version —
	// and empty for dev builds that aren't stamped via -X main.version.
	ServerVersion string `json:"server_version,omitempty"`

	// Messaging is safe for anonymous clients: it contains only deployment
	// mode and boolean capabilities, never provider credentials.
	Messaging MessagingCapabilities `json:"messaging"`
}

// GetConfig is mounted on the public (unauthenticated) route group because
// the web app calls it before login to decide whether to render the Google
// sign-in button and signup UI. Only add fields here that are safe to expose
// to anonymous callers — never user- or tenant-scoped data.
func (h *Handler) GetConfig(w http.ResponseWriter, r *http.Request) {
	config := AppConfig{
		// A property of this build, not of the deployment: if this code is
		// running, the save gate is running with it.
		LocalWorktreeSupported:             true,
		AgentConversationStartersSupported: true,
		AllowSignup:                        os.Getenv("ALLOW_SIGNUP") != "false",
		GoogleClientID:                     os.Getenv("GOOGLE_CLIENT_ID"),
		WorkspaceCreationDisabled:          os.Getenv("DISABLE_WORKSPACE_CREATION") == "true",
		Messaging:                          messagingCapabilitiesFromEnv(),
	}
	if h.Storage != nil {
		config.CdnDomain = h.Storage.CdnDomain()
	}
	config.CdnSigned = h.CFSigner != nil
	config.DaemonServerURL, config.DaemonAppURL = daemonSetupURLsFromEnv()
	config.VCSIntegrationAvailable = h.cfg.VCSIntegrationEnabled
	config.FeatureFlags = featureflags.EvaluateFrontendPublicFlags(r.Context(), h.FeatureFlags)
	// Only surface the build version on self-hosted deployments. The managed
	// cloud is continuously deployed and its users can't choose the build, so
	// the Help popover's version row would just be noise there (MUL-4108).
	if !isOfficialCloudDeployment() {
		config.ServerVersion = h.cfg.ServerVersion
	}

	// Re-read from env on every request so operators can rotate keys via
	// secret refresh without a server restart.
	if v := os.Getenv("ANALYTICS_DISABLED"); v != "true" && v != "1" {
		config.PosthogKey = os.Getenv("POSTHOG_API_KEY")
		config.PosthogHost = os.Getenv("POSTHOG_HOST")
		config.AnalyticsEnvironment = analytics.EnvironmentFromEnv()
		if config.PosthogHost == "" && config.PosthogKey != "" {
			config.PosthogHost = "https://us.i.posthog.com"
		}
	}

	writeJSON(w, http.StatusOK, config)
}

func messagingCapabilitiesFromEnv() MessagingCapabilities {
	appURL := resolveFrontendAppURL()
	officialCloud := isOfficialCloudDaemonConfig(appURL)
	requested := strings.TrimSpace(os.Getenv("PATCHBAY_MESSAGING_MODE"))
	configured := false
	platforms := make([]MessagingPlatformCapability, 0, 6)
	for _, item := range []struct {
		channelType string
		keyEnv      string
	}{
		{"lark", "PATCHBAY_LARK_SECRET_KEY"},
		{"slack", "PATCHBAY_SLACK_SECRET_KEY"},
		{"dingtalk", "PATCHBAY_DINGTALK_SECRET_KEY"},
		{"wecom", "PATCHBAY_WECOM_SECRET_KEY"},
		{"telegram", "PATCHBAY_TELEGRAM_SECRET_KEY"},
		{"weixin", "PATCHBAY_WEIXIN_SECRET_KEY"},
	} {
		_, err := secretbox.LoadKey(item.keyEnv)
		enabled := err == nil
		configured = configured || enabled
		platforms = append(platforms, MessagingPlatformCapability{
			Type: item.channelType, Enabled: enabled, Experimental: true,
		})
	}

	mode := "disabled"
	switch requested {
	case "managed", "server_configured", "disabled":
		mode = requested
	default:
		if officialCloud {
			mode = "managed"
		} else if configured {
			mode = "server_configured"
		}
	}
	if mode != "disabled" && !officialCloud && !isPublicHTTPSURL(appURL) {
		mode = "disabled"
	}
	for i := range platforms {
		platforms[i].Enabled = mode != "disabled" && platforms[i].Enabled
	}
	return MessagingCapabilities{
		Mode: mode, SetupWritable: mode == "managed", Platforms: platforms,
	}
}

// ResolvedMessagingModeFromEnv exposes the startup mode to the server boot
// path without duplicating the capability resolution rules used by /api/config.
func ResolvedMessagingModeFromEnv() string {
	return messagingCapabilitiesFromEnv().Mode
}

func isPublicHTTPSURL(raw string) bool {
	u, err := url.Parse(strings.TrimSpace(raw))
	if err != nil || u.Scheme != "https" || u.Hostname() == "" || u.User != nil || u.RawQuery != "" || u.Fragment != "" {
		return false
	}
	host := strings.TrimSuffix(strings.ToLower(u.Hostname()), ".")
	if host == "localhost" || strings.HasSuffix(host, ".local") {
		return false
	}
	if ip := net.ParseIP(host); ip != nil {
		return !ip.IsUnspecified() && !ip.IsLoopback() && !ip.IsPrivate() && !ip.IsLinkLocalUnicast()
	}
	return true
}

func daemonSetupURLsFromEnv() (string, string) {
	serverURL := normalizePublicURL(os.Getenv("PATCHBAY_DAEMON_SERVER_URL"))
	if serverURL == "" {
		serverURL = normalizePublicURL(os.Getenv("PATCHBAY_PUBLIC_URL"))
	}
	appURL := resolveFrontendAppURL()
	if appURL == "" {
		return "", ""
	}

	if serverURL == "" {
		serverURL = appURL
	}
	if isOfficialCloudDaemonConfig(appURL) {
		return "", ""
	}
	return serverURL, appURL
}

// resolveFrontendAppURL returns the operator-configured frontend origin
// (PATCHBAY_APP_URL, falling back to FRONTEND_ORIGIN), normalized. Shared by
// the daemon-setup URLs and the managed-cloud detection so both read the same
// signal.
func resolveFrontendAppURL() string {
	appURL := normalizePublicURL(os.Getenv("PATCHBAY_APP_URL"))
	if appURL == "" {
		appURL = normalizePublicURL(os.Getenv("FRONTEND_ORIGIN"))
	}
	return appURL
}

func normalizePublicURL(raw string) string {
	return strings.TrimRight(strings.TrimSpace(raw), "/")
}

// isOfficialCloudDaemonConfig reports whether this deployment is the official
// Patchbay Cloud, identified by its frontend host alone
// (patchbay.aspectlylabs.com). The
// daemon setup for the managed cloud is always
// `patchbay setup` (which hardcodes api.aspectlylabs.com), so the per-deployment URLs
// must be omitted from /api/config even when PATCHBAY_PUBLIC_URL is unset or
// misconfigured. Previously this also required
// serverURL==api.aspectlylabs.com, so a
// cloud deployment that forgot PATCHBAY_PUBLIC_URL fell through and emitted a
// `setup self-host --server-url https://patchbay.aspectlylabs.com` command — pointing the
// daemon's backend at the frontend (no /health, no WebSocket proxy).
func isOfficialCloudDaemonConfig(appURL string) bool {
	return urlHostEquals(appURL, "patchbay.aspectlylabs.com")
}

// isOfficialCloudDeployment reports whether this server is the official Patchbay
// Cloud, reusing the same frontend-host signal as the daemon setup
// (patchbay.aspectlylabs.com).
// Managed-cloud-only behavior — such as suppressing the Help popover's
// server-version row, which only matters to self-hosted operators — is gated on
// this.
func isOfficialCloudDeployment() bool {
	return isOfficialCloudDaemonConfig(resolveFrontendAppURL())
}

func urlHostEquals(raw, want string) bool {
	host := canonicalURLHost(raw)
	if host == "" {
		return false
	}
	want = strings.TrimSuffix(strings.ToLower(strings.TrimSpace(want)), ".")
	return host == want
}

func canonicalURLHost(raw string) string {
	raw = strings.TrimSpace(raw)
	u, err := url.Parse(raw)
	if err != nil {
		return ""
	}
	host := u.Hostname()
	if host == "" && !strings.Contains(raw, "://") {
		u, err = url.Parse("https://" + raw)
		if err != nil {
			return ""
		}
		host = u.Hostname()
	}
	return strings.TrimSuffix(strings.ToLower(host), ".")
}
