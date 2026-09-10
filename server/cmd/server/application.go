package main

import (
	"context"
	"crypto/sha256"
	"fmt"
	"log/slog"
	"net"
	"net/netip"
	"os"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/redis/go-redis/v9"

	"github.com/orvilo-ai/orvilo/server/internal/analytics"
	"github.com/orvilo-ai/orvilo/server/internal/auth"
	"github.com/orvilo-ai/orvilo/server/internal/cloudruntime"
	"github.com/orvilo-ai/orvilo/server/internal/daemonws"
	"github.com/orvilo-ai/orvilo/server/internal/entitlement"
	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/featureflags"
	"github.com/orvilo-ai/orvilo/server/internal/handler"
	"github.com/orvilo-ai/orvilo/server/internal/hostedcapacity"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/channel"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/channel/engine"
	composiointeg "github.com/orvilo-ai/orvilo/server/internal/integrations/composio"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/dingtalk"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/ghsnapshot"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/lark"
	linearapi "github.com/orvilo-ai/orvilo/server/internal/integrations/linear"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/linearsync"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/slack"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/telegram"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/wecom"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/weixin"
	obsmetrics "github.com/orvilo-ai/orvilo/server/internal/metrics"
	"github.com/orvilo-ai/orvilo/server/internal/middleware"
	"github.com/orvilo-ai/orvilo/server/internal/realtime"
	"github.com/orvilo-ai/orvilo/server/internal/seatcapacity"
	"github.com/orvilo-ai/orvilo/server/internal/service"
	"github.com/orvilo-ai/orvilo/server/internal/storage"
	"github.com/orvilo-ai/orvilo/server/internal/util"
	"github.com/orvilo-ai/orvilo/server/internal/util/secretbox"
	composiosdk "github.com/orvilo-ai/orvilo/server/pkg/composio"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/featureflag"
	"github.com/orvilo-ai/orvilo/server/pkg/llm"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
	publicapiv1 "github.com/orvilo-ai/orvilo/server/pkg/publicapi/v1"
)

func allowedOrigins() []string {
	raw := strings.TrimSpace(os.Getenv("CORS_ALLOWED_ORIGINS"))
	if raw == "" {
		raw = strings.TrimSpace(os.Getenv("FRONTEND_ORIGIN"))
	}
	if raw == "" {
		return defaultOrigins
	}

	parts := strings.Split(raw, ",")
	origins := make([]string, 0, len(parts))
	for _, part := range parts {
		origin := strings.TrimSpace(part)
		if origin != "" {
			origins = append(origins, origin)
		}
	}
	if len(origins) == 0 {
		return defaultOrigins
	}
	return origins
}

// pluginActionBaseURL resolves the versioned public base a hook handler calls
// back into. Managed deployments give the Plugin API its own hostname through
// ORVILO_PLUGIN_API_URL; self-hosted and local deployments can leave it empty
// and serve the same /api/v1/plugin contract on ORVILO_PUBLIC_URL.
func pluginActionBaseURL(publicURL string) string {
	if value := strings.TrimRight(strings.TrimSpace(os.Getenv("ORVILO_PLUGIN_API_URL")), "/"); value != "" {
		return value
	}
	publicURL = strings.TrimRight(strings.TrimSpace(publicURL), "/")
	if publicURL == "" {
		return ""
	}
	return publicURL + publicapiv1.BasePath
}

// parseTrustedProxies parses a comma-separated list of CIDR prefixes from the
// ORVILO_TRUSTED_PROXIES env var. Invalid entries are dropped with a single
// warn-line per entry rather than crashing the server — a typo in one CIDR
// shouldn't take the whole API down. Returns nil for empty input, which the
// rate limiter treats as "trust no proxy headers, use RemoteAddr only".
func parseTrustedProxies(raw string) []netip.Prefix {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return nil
	}
	var out []netip.Prefix
	for _, part := range strings.Split(raw, ",") {
		s := strings.TrimSpace(part)
		if s == "" {
			continue
		}
		p, err := netip.ParsePrefix(s)
		if err != nil {
			slog.Warn("ORVILO_TRUSTED_PROXIES: ignoring invalid CIDR",
				"value", s, "error", err)
			continue
		}
		out = append(out, p)
	}
	return out
}

// normalizeServerVersion maps the unstamped "dev" default (main.go's
// `version` var, unchanged when the binary wasn't built with
// -X main.version=<tag>) to an empty string. handler.Config.ServerVersion
// feeds /api/config's server_version field with omitempty, so an empty
// string hides the Help popover's version row instead of rendering
// "Server version dev" for a local `go build`/`go run` or a self-hosted
// `docker build` without --build-arg VERSION.
func normalizeServerVersion(v string) string {
	if v == "dev" {
		return ""
	}
	return v
}

func NewRouter(pool *pgxpool.Pool, hub *realtime.Hub, bus *events.Bus, analyticsClient analytics.Client, rdb *redis.Client) chi.Router {
	r, _ := NewRouterWithOptions(pool, hub, bus, analyticsClient, rdb, RouterOptions{})
	return r
}

type RouterOptions struct {
	HTTPMetrics         *obsmetrics.HTTPMetrics
	BusinessMetrics     *obsmetrics.BusinessMetrics
	ChannelLeaseMetrics *obsmetrics.ChannelLeaseMetrics
	// ChannelLeaseRedis is a dedicated non-blocking Redis client/pool. It is
	// required only when CHANNEL_WS_LEASE_BACKEND=redis.
	ChannelLeaseRedis *redis.Client
	// WecomRelay is the realtime relay, seen through the two halves the WeCom
	// adapter needs: publish a reply to the other replicas, and register as
	// the consumer that delivers the ones they publish. Nil on a deployment
	// with no Redis — where it is also unnecessary, because one replica both
	// publishes the completion and holds the socket.
	// WecomSenders is the process-wide installation→socket registry, minted in
	// main so the relay's deliverer can be registered before the shard readers
	// start. Nil is tolerated (tests, embedders): one is minted here instead.
	WecomSenders *wecom.SendersRegistry

	// WecomRelayOutbound is the cross-replica router, already registered with
	// the relay and running. Nil on a deployment with no Redis, where it is
	// also unnecessary: one replica publishes the completion and holds the
	// socket both.
	WecomRelayOutbound *wecom.RelayOutbound

	// WecomMetrics is the WeCom adapter's health sink. Nil discards every
	// counter, which is what a deployment with /metrics turned off gets.
	WecomMetrics *obsmetrics.WecomMetrics
	DaemonHub    *daemonws.Hub
	DaemonWakeup service.TaskWakeupNotifier
	FeatureFlags *featureflag.Service
	// HeartbeatScheduler, when non-nil, replaces the default synchronous
	// passthrough scheduler on the constructed Handler. main.go injects a
	// BatchedHeartbeatScheduler here so the caller can also drive Run/Stop;
	// tests leave this nil and get the legacy synchronous behavior.
	HeartbeatScheduler handler.HeartbeatScheduler
	// LLMMaxRetries carries the parsed ORVILO_LLM_MAX_RETRIES budget. Unlike
	// its three ORVILO_LLM_* siblings it is injected rather than read here,
	// because an invalid value must fail the boot and only main() can exit —
	// terminating the process from inside a router constructor would also kill
	// any test that happened to have the variable set. nil means unset, which
	// is what tests and NewRouter get.
	LLMMaxRetries *llm.RetryOverride
}

func buildChannelSupervisor(
	installations engine.InstallationStore,
	postgresLeases engine.LeaseStore,
	registry *channel.Registry,
	inbound channel.InboundHandler,
	opts RouterOptions,
) (*engine.Supervisor, handler.ChannelConnectionLeaseReader) {
	cfg, err := channelSupervisorConfigFromEnv(opts.ChannelLeaseMetrics)
	if err != nil {
		slog.Error("channel engine: invalid lease configuration; supervisor disabled", "error", err)
		return nil, nil
	}

	backend := strings.ToLower(strings.TrimSpace(os.Getenv("CHANNEL_WS_LEASE_BACKEND")))
	if backend == "" {
		backend = "postgres"
	}
	var leases engine.LeaseStore
	var connectionLeases handler.ChannelConnectionLeaseReader
	switch backend {
	case "postgres":
		leases = postgresLeases
	case "redis":
		if opts.ChannelLeaseRedis == nil {
			slog.Error("channel engine: Redis lease backend selected but CHANNEL_WS_LEASE_REDIS_URL/REDIS_URL is missing or invalid; supervisor disabled")
			return nil, nil
		}
		namespace := strings.TrimSpace(os.Getenv("CHANNEL_WS_LEASE_NAMESPACE"))
		redisLeases, err := engine.NewRedisLeaseStore(opts.ChannelLeaseRedis, namespace)
		if err != nil {
			slog.Error("channel engine: Redis lease configuration invalid; supervisor disabled", "error", err)
			return nil, nil
		}
		readyCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		err = redisLeases.Ready(readyCtx)
		cancel()
		if err != nil {
			slog.Error("channel engine: Redis lease backend unavailable; supervisor disabled", "error", err)
			return nil, nil
		}
		leases = redisLeases
		connectionLeases = redisLeases
	default:
		slog.Error("channel engine: unsupported CHANNEL_WS_LEASE_BACKEND; supervisor disabled", "backend", backend)
		return nil, nil
	}

	slog.Info("channel engine: lease backend configured",
		"backend", backend,
		"ttl", cfg.LeaseTTL.String(),
		"renew_interval", cfg.LeaseRenewInterval.String(),
		"poll_interval", cfg.PollInterval.String(),
	)
	return engine.NewSupervisor(installations, leases, registry, inbound, cfg), connectionLeases
}

// channelLeasePollInterval is how often a Supervisor scans for installations
// it should be holding, and therefore how long a WebSocket lease takes to
// finish moving to another replica.
//
// Read through one function because two places are sized by it: the supervisor
// itself, and the WeCom cross-replica dispatcher's re-offer chain — a frame
// that arrives mid-move has to stay offerable until the move completes, and
// pinning that to a constant of its own would let the two drift apart on any
// deployment that tunes the knob.
func channelLeasePollInterval() (time.Duration, error) {
	return strictPositiveDurationEnv("CHANNEL_WS_LEASE_POLL_INTERVAL", 30*time.Second)
}

func channelSupervisorConfigFromEnv(leaseMetrics *obsmetrics.ChannelLeaseMetrics) (engine.Config, error) {
	ttl, err := strictPositiveDurationEnv("CHANNEL_WS_LEASE_TTL", 180*time.Second)
	if err != nil {
		return engine.Config{}, err
	}
	renew, err := strictPositiveDurationEnv("CHANNEL_WS_LEASE_RENEW_INTERVAL", 60*time.Second)
	if err != nil {
		return engine.Config{}, err
	}
	poll, err := channelLeasePollInterval()
	if err != nil {
		return engine.Config{}, err
	}
	retry, err := strictPositiveDurationEnv("CHANNEL_WS_LEASE_ERROR_RETRY_INTERVAL", 5*time.Second)
	if err != nil {
		return engine.Config{}, err
	}
	margin, err := strictPositiveDurationEnv("CHANNEL_WS_LEASE_EXPIRY_SAFETY_MARGIN", 5*time.Second)
	if err != nil {
		return engine.Config{}, err
	}
	cfg := engine.Config{
		LeaseTTL:                ttl,
		LeaseRenewInterval:      renew,
		PollInterval:            poll,
		LeaseErrorRetryInterval: retry,
		LeaseExpirySafetyMargin: margin,
		LeaseMetrics:            leaseMetrics,
		Logger:                  slog.Default(),
	}
	if err := cfg.Validate(); err != nil {
		return engine.Config{}, err
	}
	return cfg, nil
}

func strictPositiveDurationEnv(name string, fallback time.Duration) (time.Duration, error) {
	raw := strings.TrimSpace(os.Getenv(name))
	if raw == "" {
		return fallback, nil
	}
	value, err := time.ParseDuration(raw)
	if err != nil || value <= 0 {
		return 0, fmt.Errorf("%s must be a positive duration (got %q)", name, raw)
	}
	return value, nil
}

func seatCapacityExecutor(cloudURL string) seatcapacity.Executor {
	executor, err := seatcapacity.New(seatcapacity.Config{
		BaseURL: cloudURL,
	})
	if err == nil {
		return executor
	}
	// A Cloud-connected deployment with malformed connection settings must not
	// silently restore unlimited membership. The fail-closed executor returns
	// 503 until the operator repairs the Cloud URL; it is deliberately
	// ineligible for recovery-worker startup so queued intents keep their retry
	// budget while configuration is broken.
	slog.Error("subscription seat capacity executor unavailable", "error", err)
	return seatcapacity.NewUnavailable(err)
}

// buildLarkConnector wires the real WS long-conn connector that talks
// to /callback/ws/endpoint directly with app_id/app_secret. The
// connector wraps every read with a ctx-cancel watchdog so lease loss /
// shutdown breaks the blocking ReadMessage in bounded time — the
// invariant §4.4 leans on. A single connector instance serves every
// installation; its Run is parameterized by the installation, so the
// feishuChannel hands it the per-installation row.
//
// If the endpoint fetcher fails to initialize (typically a malformed
// ORVILO_LARK_CALLBACK_BASE_URL), we log and fall back to the
// NoopConnector so the lease / supervisor lifecycle still exercises
// against real DB rows. Inbound messages are silently dropped until
// the config is fixed; the boot log labels the mode "noop" so the
// degraded state is visible.
//
// Returns the connector plus a short label for the boot log:
// "ws-long-conn" in the healthy case, "noop" in the fallback case.
func buildLarkConnector(installSvc *lark.InstallationService, apiClient lark.APIClient) (lark.EventConnector, string) {
	endpointFetcher, err := lark.NewHTTPConnectionTokenFetcher(lark.HTTPConnectionTokenConfig{
		BaseURL: strings.TrimSpace(os.Getenv("ORVILO_LARK_CALLBACK_BASE_URL")),
		Logger:  slog.Default(),
	})
	if err != nil {
		slog.Error("lark ws: endpoint fetcher init failed; falling back to noop", "error", err)
		return lark.NewNoopConnector(slog.Default()), "noop"
	}
	decoder := lark.NewLarkJSONFrameDecoder()
	dialer := lark.NewGorillaDialer()
	if proxyURL := strings.TrimSpace(os.Getenv("ORVILO_LARK_WS_PROXY_URL")); proxyURL != "" {
		dialer.ProxyURL = proxyURL
	}
	credsProvider := lark.CredentialsProviderFunc(func(ctx context.Context, inst lark.Installation) (lark.InstallationCredentials, error) {
		secret, err := installSvc.DecryptAppSecret(inst)
		if err != nil {
			return lark.InstallationCredentials{}, err
		}
		creds := lark.InstallationCredentials{
			AppID:     inst.AppID,
			AppSecret: secret,
			Region:    lark.RegionOrDefault(inst.Region),
		}
		if inst.TenantKey.Valid {
			creds.TenantKey = inst.TenantKey.String
		}
		return creds, nil
	})
	// Inbound enricher: expands quoted replies / forwarded bundles AND
	// prefetches a window of surrounding group history (MUL-3084) into the
	// agent's body via the IM API before dispatch. It shares the
	// connector's resolved credentials and runs under the connector's
	// EnrichTimeout so it cannot overrun the Lark long-conn ACK budget.
	enricher := lark.NewInboundEnricher(apiClient, lark.InboundEnricherConfig{
		RecentContextSize: lark.DefaultRecentContextSize,
		Logger:            slog.Default(),
	})
	conn, err := lark.NewWSLongConnConnector(lark.WSConnectorConfig{
		Dialer:              dialer,
		EndpointFetcher:     endpointFetcher,
		FrameDecoder:        decoder,
		Enricher:            enricher,
		CredentialsProvider: credsProvider,
		Logger:              slog.Default(),
	})
	if err != nil {
		slog.Error("lark ws: connector init failed; falling back to noop", "error", err)
		return lark.NewNoopConnector(slog.Default()), "noop"
	}
	return conn, "ws-long-conn"
}

// membershipChecker implements realtime.MembershipChecker using database queries.
type membershipChecker struct {
	queries *db.Queries
}

func (mc *membershipChecker) IsMember(ctx context.Context, userID, workspaceID string) bool {
	_, err := mc.queries.GetMemberByUserAndWorkspace(ctx, db.GetMemberByUserAndWorkspaceParams{
		UserID:      parseUUID(userID),
		WorkspaceID: parseUUID(workspaceID),
	})
	return err == nil
}

// opaqueTokenResolver implements realtime.OpaqueTokenResolver using database queries.
// patCache is shared with the Auth and DaemonAuth middlewares so a token
// revoke through any path invalidates the cache for all of them. Nil
// cache is supported and degrades to direct DB lookups.
type opaqueTokenResolver struct {
	queries *db.Queries
	cache   *auth.PATCache
}

func (pr *opaqueTokenResolver) ResolveToken(ctx context.Context, token string) (string, bool) {
	if strings.HasPrefix(token, auth.GuestTokenPrefix) {
		if pr.queries == nil {
			return "", false
		}
		user, err := auth.ResolveGuestUser(ctx, pr.queries, token)
		if err != nil {
			return "", false
		}
		uid := util.UUIDToString(user.ID)
		if auth.IsTemporarilyDisabledUser(uid, user.Email) {
			return "", false
		}
		return uid, true
	}

	hash := auth.HashToken(token)

	if userID, ok := pr.cache.Get(ctx, hash); ok {
		return userID, true
	}

	pat, err := pr.queries.GetPersonalAccessTokenByHash(ctx, hash)
	if err != nil {
		return "", false
	}

	userID := util.UUIDToString(pat.UserID)

	var expiresAt time.Time
	if pat.ExpiresAt.Valid {
		expiresAt = pat.ExpiresAt.Time
	}
	pr.cache.Set(ctx, hash, userID, auth.TTLForExpiry(time.Now(), expiresAt))

	// Cache miss = first WS auth in this TTL window. Refresh last_used_at;
	// subsequent connects within the window skip the write.
	go pr.queries.UpdatePersonalAccessTokenLastUsed(context.Background(), pat.ID)

	return userID, true
}

// parseUUID is a thin alias for util.MustParseUUID. Call sites here are all
// internal round-trips of DB-sourced UUIDs (e.g. issue.ID, e.ActorID), so an
// invalid value indicates a programming error and should panic loudly.
func parseUUID(s string) pgtype.UUID {
	return util.MustParseUUID(s)
}

// optionalUUID returns a NULL pgtype.UUID for an empty string and otherwise
// behaves like parseUUID. Use this for actor IDs on events where the producer
// may legitimately be a "system" actor with no member/agent attribution
// (e.g. GitHub webhook auto-status sync) — the activity_log and inbox_item
// tables both allow actor_id to be NULL.
func optionalUUID(s string) pgtype.UUID {
	if s == "" {
		return pgtype.UUID{}
	}
	return util.MustParseUUID(s)
}

func splitAndTrim(s string) []string {
	if s == "" {
		return nil
	}
	parts := strings.Split(s, ",")
	res := make([]string, 0, len(parts))
	for _, p := range parts {
		trimmed := strings.TrimSpace(p)
		if trimmed != "" {
			res = append(res, trimmed)
		}
	}
	return res
}

// composioStateSecret resolves the HMAC key for the connect-state. Prefers an
// explicit COMPOSIO_STATE_SECRET; otherwise derives a composio-specific key
// from JWT_SECRET via SHA-256 so the two signing domains never share an
// identical key. Returns nil when neither is set (composio stays disabled).
func composioStateSecret() []byte {
	if v := strings.TrimSpace(os.Getenv("COMPOSIO_STATE_SECRET")); v != "" {
		return []byte(v)
	}
	if v := strings.TrimSpace(os.Getenv("JWT_SECRET")); v != "" {
		sum := sha256.Sum256([]byte("composio-state:" + v))
		return sum[:]
	}
	return nil
}

// composioCallbackBaseURL resolves the public API base used to build the
// Composio callback URL. Prefers COMPOSIO_CALLBACK_BASE_URL, then the
// already-resolved ORVILO_PUBLIC_URL, then the app URL.
func composioCallbackBaseURL(publicURL string) string {
	if v := strings.TrimRight(strings.TrimSpace(os.Getenv("COMPOSIO_CALLBACK_BASE_URL")), "/"); v != "" {
		return v
	}
	if publicURL != "" {
		return publicURL
	}
	return handler.FrontendAppURLFromEnv()
}

// WecomRelay is what the WeCom adapter needs from the realtime relay:
// publish under ScopeWecomOutbound, and register the consumer that acts on
// frames the other replicas publish. *realtime.ShardedStreamRelay and
// *realtime.RedisRelay both satisfy it.
type WecomRelay interface {
	PublishWithID(scopeType, scopeID, exclude string, frame []byte, id string) error
	SetWecomOutboundDeliverer(d realtime.WecomOutboundDeliverer)
}

// wecomMetricsOrNil keeps a typed nil out of the adapter's interface field.
// A *WecomMetrics that is nil still satisfies wecom.Metrics, so assigning it
// directly would give the adapter a non-nil interface holding a nil pointer —
// and the first counter call would panic on a deployment with /metrics off.
func wecomMetricsOrNil(m *obsmetrics.WecomMetrics) wecom.Metrics {
	if m == nil {
		return nil
	}
	return m
}

type routeSettings struct {
	Origins                                                                                  []string
	Config                                                                                   handler.Config
	HTTPMetrics                                                                              *obsmetrics.HTTPMetrics
	RealtimeMetricsToken                                                                     string
	TrustedRateProxies                                                                       []*net.IPNet
	AuthRate, VerifyRate, HandoffRate, ContactRate, DevLoginRate, DeviceAuthRate, PluginRate int
}

type application struct {
	messagingMode    string
	HTTP             *handler.Handler
	queries          *db.Queries
	bus              *events.Bus
	Services         handler.Services
	Workers          applicationWorkers
	pool             *pgxpool.Pool
	hub              *realtime.Hub
	redis            *redis.Client
	routes           routeSettings
	cloudPATVerifier *auth.CloudPATVerifier
}

// NewRouterWithOptions is the HTTP embedding/test convenience entrypoint.
// Production main owns the assembled application's lifecycle separately.
func NewRouterWithOptions(pool *pgxpool.Pool, hub *realtime.Hub, bus *events.Bus, analyticsClient analytics.Client, rdb *redis.Client, opts RouterOptions) (chi.Router, *handler.Handler) {
	app := newApplication(pool, hub, bus, analyticsClient, rdb, opts)
	return newRouter(app), app.HTTP
}

func newApplication(pool *pgxpool.Pool, hub *realtime.Hub, bus *events.Bus, analyticsClient analytics.Client, rdb *redis.Client, opts RouterOptions) *application {
	queries := db.New(pool)
	emailSvc := service.NewEmailService()
	daemonHub := opts.DaemonHub
	if daemonHub == nil {
		daemonHub = daemonws.NewHub()
	}

	// Initialize storage with S3 as primary, fallback to local
	var store storage.Storage
	s3 := storage.NewS3StorageFromEnv()
	if s3 != nil {
		store = s3
	} else {
		local := storage.NewLocalStorageFromEnv()
		if local != nil {
			store = local
		}
	}

	cfSigner := auth.NewCloudFrontSignerFromEnv()
	origins := allowedOrigins()

	signupConfig := handler.Config{
		AllowSignup:              os.Getenv("ALLOW_SIGNUP") != "false",
		AllowedEmails:            splitAndTrim(os.Getenv("ALLOWED_EMAILS")),
		AllowedEmailDomains:      splitAndTrim(os.Getenv("ALLOWED_EMAIL_DOMAINS")),
		HostedDesktopIdentity:    os.Getenv("ORVILO_HOSTED_DESKTOP_IDENTITY") == "1",
		DesktopBrokerAuthToken:   strings.TrimSpace(os.Getenv("ORVILO_DESKTOP_BROKER_AUTH_TOKEN")),
		ClerkSecretKey:           strings.TrimSpace(os.Getenv("CLERK_SECRET_KEY")),
		ClerkJWTKey:              strings.TrimSpace(os.Getenv("CLERK_JWT_KEY")),
		ClerkIssuer:              strings.TrimRight(strings.TrimSpace(os.Getenv("CLERK_ISSUER")), "/"),
		ClerkAuthorizedParties:   splitAndTrim(os.Getenv("CLERK_AUTHORIZED_PARTIES")),
		DisableWorkspaceCreation: os.Getenv("DISABLE_WORKSPACE_CREATION") == "true",
		VCSIntegrationEnabled:    os.Getenv("ORVILO_VCS_INTEGRATION_ENABLED") == "true",
		PublicURL:                strings.TrimRight(strings.TrimSpace(os.Getenv("ORVILO_PUBLIC_URL")), "/"),
		AppURL:                   handler.FrontendAppURLFromEnv(),
		TrustedProxies:           parseTrustedProxies(os.Getenv("ORVILO_TRUSTED_PROXIES")),
		CloudURL:                 strings.TrimSpace(os.Getenv("ORVILO_CLOUD_URL")),
		CloudTimeout:             35 * time.Second,
		AttachmentDownloadMode:   os.Getenv("ATTACHMENT_DOWNLOAD_MODE"),
		AttachmentDownloadURLTTL: envDuration("ATTACHMENT_DOWNLOAD_URL_TTL", 30*time.Minute),
		AttachmentFrameAncestors: origins,
		PluginSurfaceOrigin:      strings.TrimRight(strings.TrimSpace(os.Getenv("ORVILO_PLUGIN_SURFACE_ORIGIN")), "/"),
		LLMAPIKey:                strings.TrimSpace(os.Getenv("ORVILO_LLM_API_KEY")),
		LLMBaseURL:               strings.TrimSpace(os.Getenv("ORVILO_LLM_BASE_URL")),
		LLMDefaultModel:          strings.TrimSpace(os.Getenv("ORVILO_LLM_DEFAULT_MODEL")),
		LLMMaxRetries:            opts.LLMMaxRetries,
		ServerVersion:            normalizeServerVersion(version),
	}
	services := assembleServices(queries, pool, hub, bus, analyticsClient, store, signupConfig, daemonHub)
	h := handler.New(queries, pool, hub, bus, emailSvc, store, cfSigner, analyticsClient, signupConfig, services, daemonHub)
	workers := applicationWorkers{Tasks: services.Tasks, Automations: services.Automations, Coordination: services.Coordination, Plugins: services.Plugins}
	// Weixin QR install sessions use Redis when it is already configured for
	// the server, with the adapter's bounded in-memory fallback otherwise.
	weixin.ConfigureSessionStore(rdb)
	invitationRateLimits := handler.DefaultInvitationRateLimits()
	invitationRateLimits.Actor.Limit = envNonNegativeInt("RATE_LIMIT_INVITATION_ACTOR_10M", invitationRateLimits.Actor.Limit)
	invitationRateLimits.Workspace.Limit = envNonNegativeInt("RATE_LIMIT_INVITATION_WORKSPACE_24H", invitationRateLimits.Workspace.Limit)
	invitationRateLimits.Recipient.Limit = envNonNegativeInt("RATE_LIMIT_INVITATION_RECIPIENT_24H", invitationRateLimits.Recipient.Limit)
	h.InvitationRateLimiters = handler.NewMemoryInvitationRateLimiters(invitationRateLimits)
	h.Metrics = opts.BusinessMetrics
	h.FeatureFlags = opts.FeatureFlags
	h.TaskService.FeatureFlags = opts.FeatureFlags
	h.TaskService.Metrics = opts.BusinessMetrics
	messagingMode := handler.ResolvedMessagingModeFromEnv()
	services.Tasks.ManagedMessaging = messagingMode == "managed"
	h.IssueService.Metrics = opts.BusinessMetrics
	entitlementClient, entitlementErr := entitlement.New(entitlement.Config{
		BaseURL:  signupConfig.CloudURL,
		Observer: opts.BusinessMetrics,
	})
	if entitlementErr != nil {
		slog.Error("entitlement policy client disabled by malformed Orvilo Cloud URL", "error", entitlementErr)
		opts.BusinessMetrics.RecordEntitlementConfigError()
	} else if entitlementClient.Enabled() {
		h.Entitlements = entitlementClient
		h.TaskService.Entitlements = entitlementClient
		h.IssueService.Entitlements = entitlementClient
		h.AutomationService.Entitlements = entitlementClient
		h.AutomationService.QuotaMetrics = opts.BusinessMetrics
	}
	// Cloud Runtime and strict seat capacity are one managed deployment. Reuse
	// the same base URL so Billing cannot be readable while invitation writes
	// silently skip Cloud's authoritative seat policy.
	h.SeatCapacity = seatCapacityExecutor(signupConfig.CloudURL)
	capacityLocker := seatcapacity.NewWorkspaceLocker(pool)
	h.SeatCapacityLocker = capacityLocker
	if seatcapacity.CanRunWorker(h.SeatCapacity) {
		workers.SeatCapacity = seatcapacity.NewWorker(queries, h.SeatCapacity, capacityLocker, seatcapacity.WorkerConfig{})
	}
	// Hosted IM installation capacity: the Cloud entitlement gate
	// im_installation_limit decides the per-workspace cap on concurrent
	// channel installations; the limiter reconciles durable pause markers on
	// every resolve and the worker keeps them convergent between installs.
	// ORVILO_HOSTED_IM_CAPACITY is the deployment switch — self-hosted
	// leaves it off and every install path runs exactly as before. An enabled
	// switch without a Cloud policy source fails closed (503) rather than
	// letting a workspace grow past an unreadable cap.
	hostedResolver := hostedcapacity.NewResolver(os.Getenv("ORVILO_HOSTED_IM_CAPACITY") == "true", h.Entitlements)
	if hostedResolver.Enabled() {
		h.HostedCapacity = hostedcapacity.NewLimiter(hostedResolver, queries, pool, slog.Default())
		workers.HostedCapacity = hostedcapacity.NewWorker(hostedResolver, queries, pool, hostedcapacity.WorkerConfig{})
		slog.Info("hosted IM installation capacity enforcement enabled")
	}
	if opts.BusinessMetrics != nil {
		// Wire the BusinessMetrics receiver into the cloud runtime client
		// so every outbound Fleet/Gateway request feeds the
		// orvilo_cloudruntime_request_* histograms.
		if client, ok := h.CloudRuntime.(*cloudruntime.Client); ok {
			client.SetRecorder(opts.BusinessMetrics)
		}
	}
	if opts.DaemonWakeup != nil {
		h.TaskService.Wakeup = opts.DaemonWakeup
		if notifier, ok := opts.DaemonWakeup.(handler.RuntimeProfileRefreshNotifier); ok {
			h.DaemonProfileRefresh = notifier
		}
		if notifier, ok := opts.DaemonWakeup.(handler.WorkspaceSetRefreshNotifier); ok {
			h.DaemonWorkspaceRefresh = notifier
		}
		if notifier, ok := opts.DaemonWakeup.(handler.DaemonPendingWorkNotifier); ok {
			h.DaemonPendingWork = notifier
		}
	}
	if rdb != nil {
		h.UpdateStore = handler.NewRedisUpdateStore(rdb)
		h.ModelListStore = handler.NewRedisModelListStore(rdb)
		h.ModelCatalogCache = handler.NewRedisModelCatalogCache(rdb)
		h.LocalSkillListStore = handler.NewRedisLocalSkillListStore(rdb)
		h.LocalSkillImportStore = handler.NewRedisLocalSkillImportStore(rdb)
		h.LivenessStore = handler.NewRedisLivenessStore(rdb)
		h.WebhookRateLimiter = handler.NewRedisWebhookRateLimiter(rdb, handler.DefaultWebhookRateLimit())
		h.WebhookIPRateLimiter = handler.NewRedisWebhookIPRateLimiter(rdb, handler.DefaultWebhookIPRateLimit())
		h.WebhookAbsoluteIPRateLimiter = handler.NewRedisWebhookAbsoluteIPRateLimiter(rdb, handler.DefaultWebhookAbsoluteIPRateLimit())
		h.InvitationRateLimiters = handler.NewRedisInvitationRateLimiters(rdb, invitationRateLimits)
	}

	// Channel engine (MUL-3620): the platform-agnostic inbound runtime.
	// Built UNCONDITIONALLY — it drives any channel.Channel, not just
	// Feishu, so it must not depend on the Lark master key (a future
	// Slack-only deployment has no Lark key). Platform adapters register a
	// Factory + ResolverSet into it below; the Supervisor enumerates active
	// installations across ALL channel types and routes each to its
	// registered platform's Factory. Installations whose channel_type has no
	// registered Factory are skipped by the Supervisor — either no platform is
	// configured, or (Slack/B2) the platform drives ONE deployment-level
	// connection of its own outside the per-installation supervisor. The Router
	// is the single shared inbound handler injected into every Channel.
	channelRegistry := channel.NewRegistry()
	channelHub := engine.NewPostgresHubRouter(queries, pool)
	channelRouter := engine.NewRouter(h.IssueService, h.TaskService, queries, engine.RouterConfig{
		Logger: slog.Default(), Lifecycle: h, Hub: channelHub,
	})
	// Debounce the per-session run trigger so a burst of messages collapses
	// into one agent run instead of one per message (MUL-2968).
	channelRouter.EnableRunBatching(engine.DefaultChatRunBatchWindow)
	h.ChannelRouter = channelRouter
	// Media intent-ledger reconciler: settles uploaded-but-unbound objects.
	// Built ONLY when a storage backend exists — store is nil when S3 is not
	// configured and the local upload dir failed to initialize, and a
	// reconciler with nil Storage would panic the worker goroutine on the
	// first unreferenced row (ledger rows can pre-exist from a boot where
	// storage WAS configured). Without storage the resolver skips every
	// upload, so no new rows appear and the ledger simply waits for a boot
	// with working storage. Started from main.go as its own worker.
	if store != nil {
		workers.ChannelMedia = &service.ChannelMediaReconciler{
			Queries: queries,
			Storage: store,
			Logger:  slog.Default(),
		}
	}
	installationStore := lark.NewChannelInstallationStore(queries)
	workers.Channels, h.ChannelConnectionLeases = buildChannelSupervisor(
		installationStore,
		installationStore,
		channelRegistry,
		channelRouter.Handle,
		opts,
	)

	h.ChannelSupervisor = workers.Channels
	workers.ChannelRouter = channelRouter

	// Lark integration. Only wired when ORVILO_LARK_SECRET_KEY is set:
	// the InstallationService refuses to fall back to plaintext storage
	// for app_secret, and the BindingTokenService cannot mint usable
	// tokens without it either. When the key is absent the Lark
	// handlers return 503 with a clear message; the rest of the server
	// continues to start so self-host deployments that have not opted
	// in to Lark are unaffected. Feishu registers its Factory + ResolverSet
	// into the channel engine above.
	if larkKey, err := secretbox.LoadKey("ORVILO_LARK_SECRET_KEY"); err == nil {
		box, err := secretbox.New(larkKey)
		if err != nil {
			slog.Error("lark: secretbox.New failed; lark integration disabled", "error", err)
		} else {
			installSvc, err := lark.NewInstallationService(queries, box)
			if err != nil {
				slog.Error("lark: InstallationService init failed; lark integration disabled", "error", err)
			} else {
				h.LarkInstallations = installSvc
				h.LarkBindingTokens = lark.NewBindingTokenService(queries, pool)
				slog.Info("lark integration enabled")

				// APIClient: wire the real Lark Open Platform HTTP client
				// (IM v1 send/patch + binding-prompt + bot info). Setting
				// ORVILO_LARK_SECRET_KEY is the operator's opt-in for
				// the integration as a whole; we don't expose a separate
				// "HTTP enabled" knob because the inbound dispatcher
				// without outbound replies is not a useful production
				// state, and CI / integration tests that want to avoid
				// real Lark traffic can point ORVILO_LARK_HTTP_BASE_URL
				// at a mock server.
				//
				// ORVILO_LARK_HTTP_BASE_URL is an OPTIONAL deployment-wide
				// override. Normal operation leaves it empty: each call then
				// resolves its open-platform host from the installation's
				// region (open.feishu.cn vs open.larksuite.com), so one
				// deployment serves both clouds. Set it only to force every
				// installation onto one host — a proxy, a mock for tests, or
				// a single-cloud staging setup.
				larkClient := lark.NewHTTPAPIClient(lark.HTTPClientConfig{
					BaseURL: strings.TrimSpace(os.Getenv("ORVILO_LARK_HTTP_BASE_URL")),
					Logger:  slog.Default(),
				})
				h.LarkAPIClient = larkClient

				// Channel-backed store: routes the lark package's DB seams
				// onto the channel_* tables (MUL-3515). Interface-wired
				// consumers (patcher, typing indicator, dispatcher, hub,
				// backfills) take it directly; the constructor-based services
				// wrap *db.Queries internally, so they keep taking queries.
				cs := lark.NewChannelStore(queries)
				patcher := lark.NewPatcher(cs, installSvc, larkClient, lark.PatcherConfig{})
				patcher.Register(bus)

				// Typing indicator: shows a "processing" reaction on the user's
				// message while the agent is working, then removes it before the
				// reply is sent. Best-effort; failures are logged only.
				typingIndicator := lark.NewTypingIndicatorManager(larkClient, installSvc, cs, slog.Default())
				patcher.SetTypingIndicatorManager(typingIndicator)

				// Inbound pipeline seams: lark_inbound_audit logger and the
				// shared channel-agnostic chat-session service. They back the
				// Feishu ResolverSet that the engine.Router runs through,
				// sharing the same IssueService + TaskService that back HTTP, so
				// /issue-created issues share counter, dup guard, project
				// boundary, broadcast, analytics and agent-enqueue with the rest
				// of the product. Feishu is just another consumer of the shared
				// engine.ChatSession (channel_type-keyed); the Lark session
				// titles preserve the pre-cutover wording.
				auditLogger := lark.NewAuditLogger(queries)
				feishuSession := engine.NewChatSession(queries, pool, channel.TypeFeishu, engine.SessionTitles{
					Group:    "Lark group chat",
					Direct:   "Lark direct message",
					Fallback: "Lark chat",
				})

				// OutcomeReplier wires the outbound side: NeedsBinding /
				// AgentOffline / AgentArchived / issue-created translate to a
				// Lark-side reply card. Requires the real APIClient and the
				// binding token service; otherwise it falls back to the noop
				// replier (outcomes logged, not delivered). We only register
				// it on the ResolverSet when it can actually deliver, so a
				// pre-outbound deployment pays no reply-goroutine cost.
				replier := lark.NewLarkOutcomeReplier(lark.OutcomeReplierConfig{
					APIClient:   larkClient,
					BindingSvc:  h.LarkBindingTokens,
					Credentials: installSvc,
					Queries:     queries,
					AppURL:      signupConfig.AppURL,
					Logger:      slog.Default(),
				})
				var resolverReplier lark.OutcomeReplier
				if larkClient.IsConfigured() {
					resolverReplier = replier
				}

				// Feishu adapter (MUL-3620): the WSLongConnConnector talks
				// Lark's long-conn protocol over gorilla/websocket and wraps
				// every read with a ctx-cancel watchdog so lease loss /
				// shutdown breaks the blocking ReadMessage in bounded time —
				// the invariant §4.4 leans on. If the endpoint fetcher fails
				// to initialize (bad ORVILO_LARK_CALLBACK_BASE_URL or
				// similar), buildLarkConnector logs and falls back to the
				// NoopConnector so the lease / supervisor lifecycle still runs
				// against real DB rows — inbound messages are silently dropped
				// until the config is fixed, with the boot log labelling the
				// mode "noop".
				//
				// Registering the Factory (connect/send) + ResolverSet
				// (inbound pipeline seams) is all it takes to add the platform
				// to the engine — no engine edit.
				connector, connectorLabel := buildLarkConnector(installSvc, larkClient)
				lark.RegisterFeishu(channelRegistry, lark.FeishuChannelDeps{
					Connector:   connector,
					APIClient:   larkClient,
					Credentials: installSvc,
					Logger:      slog.Default(),
				})
				mediaResolver := lark.NewFeishuMediaResolver(larkClient, installSvc, store, engine.NewDBMediaIntentLedger(queries), slog.Default())
				channelRouter.Register(channel.TypeFeishu, lark.NewFeishuResolverSet(
					cs, feishuSession, auditLogger, resolverReplier, typingIndicator, mediaResolver,
				))
				slog.Info("lark inbound pipeline wired", "connector", connectorLabel)

				// One-shot union_id backfill for installations created
				// before migration 112 added bot_union_id. Runs off the
				// hot startup path so a slow Lark round-trip cannot block
				// HTTP listener boot. New installs already write
				// bot_union_id during the device-flow finalize, so this
				// is bridge code — it will simply find no rows to update
				// on a fresh deployment and exit. MUL-2671.
				go lark.BackfillBotUnionIDs(context.Background(), cs, larkClient, installSvc, slog.Default())

				// Upgrade repair for deployments that ran the whole
				// integration against Lark international via the deployment-
				// wide base-URL override before per-installation region
				// existed: migration 116 backfilled their rows to 'feishu',
				// so relabel them to 'lark' (their true cloud) before the
				// operator clears the override. No-op on mainland / fresh
				// deployments. Off the hot startup path like the union_id
				// backfill. MUL-3083.
				go lark.BackfillRegionFromLegacyOverride(context.Background(), cs,
					strings.TrimSpace(os.Getenv("ORVILO_LARK_HTTP_BASE_URL")),
					strings.TrimSpace(os.Getenv("ORVILO_LARK_CALLBACK_BASE_URL")),
					slog.Default())

				// Device-flow registration service: end-to-end install
				// pipeline that talks to accounts.feishu.cn (RFC 8628)
				// for the QR-scan handshake and then commits the
				// resulting Bot credentials + the installer's
				// lark_user_binding in one DB transaction. The optional
				// ORVILO_LARK_REGISTRATION_DOMAIN / _LARK_DOMAIN env
				// vars override the protocol hosts for staging / dev.
				regCfg := lark.RegistrationConfig{
					Domain:     strings.TrimSpace(os.Getenv("ORVILO_LARK_REGISTRATION_DOMAIN")),
					LarkDomain: strings.TrimSpace(os.Getenv("ORVILO_LARK_REGISTRATION_LARK_DOMAIN")),
				}
				regClient := lark.NewRegistrationClient(regCfg)
				regSvc, rerr := lark.NewRegistrationService(
					lark.RegistrationServiceConfig{Logger: slog.Default()},
					regClient,
					larkClient,
					queries,
					pool,
					installSvc,
					h.LarkBindingTokens,
				)
				if rerr != nil {
					slog.Error("lark: RegistrationService init failed; install disabled", "error", rerr)
				} else {
					// Publish lark_installation:created at row-commit time so the
					// connection badge refreshes on every workspace client, not just
					// the tab that polls the install status to success.
					regSvc.SetEventBus(bus)
					// The QR finalize re-resolves the hosted installation cap
					// through the limiter (nil on self-hosted deployments).
					regSvc.SetHostedCapacityLimiter(h.HostedCapacity)
					h.LarkRegistration = regSvc
					slog.Info("lark device-flow install enabled")
				}
			}
		}
	} else {
		slog.Info("lark integration disabled (ORVILO_LARK_SECRET_KEY not set)")
	}

	// Slack integration. Multi-tenant B2 model (MUL-3666): Orvilo hosts ONE
	// Slack app, workspaces self-install via OAuth, and inbound runs on a single
	// deployment-level Socket Mode connection routed by team_id — replacing the
	// stage-3 per-installation connection model (MUL-3516).
	//
	// Two deployment-level env vars gate the two halves:
	//   - ORVILO_SLACK_SECRET_KEY decrypts the per-installation bot token
	//     (xoxb-) stored on the channel_installation row. It gates the inbound
	//     ResolverSet + the outbound reply subscriber, so without it there is no
	//     Slack at all.
	//   - ORVILO_SLACK_APP_TOKEN is the app-level token (xapp-) authorizing the
	//     single Socket Mode connection. It cannot be obtained via OAuth, so it
	//     is a one-time operator config. Without it, inbound is disabled (the
	//     ResolverSet + outbound are still wired so an existing install's replies
	//     keep flowing, but no new events are received).
	//
	// The ResolverSet/Outbound share the same engine.ChatSession, channel_*
	// tables, IssueService and TaskService as Feishu, so /issue, dedup, and
	// run-triggering behave identically. Feishu is untouched. Each Slack
	// installation is a bring-your-own-app (BYO) install carrying its OWN
	// app-level token, so a per-installation Slack Factory is registered and the
	// Supervisor drives one Socket Mode connection per installation (like Feishu).
	if slackKey, err := secretbox.LoadKey("ORVILO_SLACK_SECRET_KEY"); err == nil {
		box, err := secretbox.New(slackKey)
		if err != nil {
			slog.Error("slack: secretbox.New failed; slack integration disabled", "error", err)
		} else {
			// Outbound replier (MUL-3666): delivers NeedsBinding prompt /
			// AgentOffline / AgentArchived / issue-created notices. The binding
			// token service mints the single-use token embedded in the prompt's
			// redeem link; the redeem endpoint (registered below, public) binds
			// the Slack user to their Orvilo account.
			slackBindingSvc := slack.NewBindingTokenService(queries, pool)
			h.SlackBindingTokens = slackBindingSvc
			slackReplier := slack.NewOutboundReplier(slack.OutboundReplierConfig{
				Binding: slackBindingSvc,
				Decrypt: box.Open,
				// The bind link (/slack/bind) is a web-app page, so it must use the
				// app URL (ORVILO_APP_URL ?? FRONTEND_ORIGIN), NOT ORVILO_PUBLIC_URL
				// (the backend/API URL). Mirrors the Lark replier (appURLFromEnv).
				AppURL:  signupConfig.AppURL,
				Queries: queries,
				Logger:  slog.Default(),
			})
			// Typing indicator (MUL-3874): a 👀 reaction on the user's message
			// while the agent works, cleared when the run finishes or fails.
			// Best-effort; failures are logged only. Registered before the
			// outbound reply subscriber so, on EventChatDone, the reaction clears
			// ahead of the reply (bus delivery is synchronous, in subscription
			// order). Subscribing here is also the only path that clears the
			// reaction on a failed run, which the outbound replier does not handle.
			slackTyping := slack.NewTypingIndicatorManager(queries, box.Open, slog.Default())
			slackTyping.Register(bus)
			slack.NewAutomationCompletionReactor(queries, pool, box.Open, slog.Default()).Register(bus)
			// Slack attachments require object storage because each chat
			// attachment points to an uploaded object. When storage is disabled,
			// leave the media resolver unset and ingest Slack messages as text.
			var slackMedia engine.MediaResolver
			if store != nil {
				slackMedia = slack.NewMediaResolver(
					box.Open,
					store,
					engine.NewDBMediaIntentLedger(queries),
					slog.Default(),
				)
			}
			channelRouter.Register(slack.TypeSlack, slack.NewSlackResolverSet(queries, pool, slackReplier, slackTyping, slackMedia))
			slack.NewOutbound(queries, box.Open, slog.Default()).Register(bus)

			// On-demand history reader behind the unified `orvilo chat history`
			// command (MUL-3871): pull the session's Slack conversation when the
			// agent asks, instead of force-assembling it on every inbound.
			h.SlackHistory = slack.NewHistory(queries, box.Open, slog.Default())

			// `/issue`, `/new`, and `/clear` are real Slack slash commands delivered
			// over the same Socket Mode connection. `/issue` enqueues quick-create;
			// the two session controls reuse the shared route/context and task
			// services. Every outcome receives a private ephemeral acknowledgement.
			slackSlash := slack.NewSlashCommandProcessor(slack.SlashCommandConfig{
				Queries: queries,
				Hub:     channelHub,
				Tasks:   h.TaskService,
				Control: slack.NewSlackDMControlStarter(queries, pool, h.TaskService, h, channelRouter.FlushPendingSession),
				Binding: slackBindingSvc,
				AppURL:  signupConfig.AppURL,
				Logger:  slog.Default(),
			})

			// Per-installation inbound: the Supervisor builds + supervises one
			// Socket Mode connection per active Slack installation, authenticated
			// with that installation's OWN app-level token (xapp-, pasted at BYO
			// install) — no deployment-level app token, no single connection.
			slack.RegisterSlack(channelRegistry, slack.ChannelDeps{
				Decrypt: box.Open,
				Logger:  slog.Default(),
				Slash:   slackSlash,
				OnNativeEvent: func(ctx context.Context, installationID pgtype.UUID, body []byte) {
					inst, err := queries.GetChannelInstallation(ctx, db.GetChannelInstallationParams{
						ID:          installationID,
						ChannelType: string(slack.TypeSlack),
					})
					if err != nil || inst.Status != "installed" {
						return
					}
					h.HandleSlackNativeAutomation(ctx, inst, body)
				},
			})

			// BYO self-serve install (paste bot token + app-level token). The
			// InstallService needs only the at-rest encryption key — there is no
			// hosted OAuth client credential.
			installSvc, ierr := slack.NewInstallService(queries, pool, box, slog.Default())
			if ierr != nil {
				slog.Error("slack: InstallService init failed; install disabled", "error", ierr)
			} else {
				h.SlackInstall = installSvc
			}
			// Managed (hosted) OAuth for the official Orvilo Slack app: state
			// issuance + code exchange. The service stores only state hashes, so
			// it needs no secretbox; persistence still goes through InstallService
			// above, and the callback 503s without it. Client credentials are
			// optional at boot — begin mints state regardless but refuses the
			// authorize URL with 503 until ORVILO_SLACK_CLIENT_ID/_SECRET are
			// set, so a deployment without a hosted app fails loudly.
			managedOAuth, merr := slack.NewManagedOAuthService(slack.ManagedOAuthConfig{
				Queries:      queries,
				ClientID:     strings.TrimSpace(os.Getenv("ORVILO_SLACK_CLIENT_ID")),
				ClientSecret: strings.TrimSpace(os.Getenv("ORVILO_SLACK_CLIENT_SECRET")),
				Logger:       slog.Default(),
			})
			if merr != nil {
				slog.Error("slack: ManagedOAuthService init failed; managed install disabled", "error", merr)
			} else {
				h.ManagedSlack = managedOAuth
				workers.SlackTokens = slack.NewManagedTokenWorker(queries, box, managedOAuth, slog.Default())
			}
			// Managed Events API ingress for hosted-OAuth installs (no Socket
			// Mode link). The signing secret is optional at boot — without it
			// the endpoint 503s instead of accepting unsigned deliveries.
			// Registered on the public router below, next to the OAuth callback.
			managedWebhook, werr := slack.NewManagedWebhook(slack.ManagedWebhookConfig{
				Queries:       queries,
				Handle:        channelRouter.Handle,
				Slash:         slackSlash,
				SigningSecret: strings.TrimSpace(os.Getenv("ORVILO_SLACK_SIGNING_SECRET")),
				Logger:        slog.Default(),
				OnNativeEvent: h.HandleSlackNativeAutomation,
			})
			if werr != nil {
				slog.Error("slack: ManagedWebhook init failed; managed ingress disabled", "error", werr)
			} else {
				h.ManagedSlackWebhook = managedWebhook
			}
			slog.Info("slack integration enabled (BYO per-installation socket mode)")
		}
	} else {
		slog.Info("slack integration disabled (ORVILO_SLACK_SECRET_KEY not set)")
	}

	// DingTalk uses one outbound Stream connection per BYO installation. The
	// AppSecret is encrypted at rest and the integration is inert unless its
	// dedicated deployment key is configured.
	if dingtalkKey, err := secretbox.LoadKey("ORVILO_DINGTALK_SECRET_KEY"); err == nil {
		box, err := secretbox.New(dingtalkKey)
		if err != nil {
			slog.Error("dingtalk: secretbox.New failed; integration disabled", "error", err)
		} else {
			dingtalkClient := dingtalk.NewClient(nil, "")
			bindingSvc := dingtalk.NewBindingTokenService(queries, pool)
			h.DingTalkBindingTokens = bindingSvc
			replier := dingtalk.NewOutboundReplier(dingtalk.OutboundReplierConfig{
				Binding: bindingSvc,
				Decrypt: box.Open,
				Client:  dingtalkClient,
				AppURL:  signupConfig.AppURL,
				Logger:  slog.Default(),
			})
			ack := dingtalk.NewAckNotifier(dingtalkClient, box.Open, slog.Default())
			var media engine.MediaResolver
			if store != nil {
				media = dingtalk.NewMediaResolver(
					dingtalkClient,
					box.Open,
					store,
					engine.NewDBMediaIntentLedger(queries),
					slog.Default(),
				)
			}
			botNames := dingtalk.NewBotNameResolver(dingtalkClient, box.Open)
			channelRouter.Register(dingtalk.TypeDingTalk, dingtalk.NewDingTalkResolverSet(queries, pool, replier, ack, media, botNames))
			dingtalk.NewOutbound(queries, box.Open, dingtalkClient, slog.Default()).Register(bus)
			dingtalk.RegisterDingTalk(channelRegistry, dingtalk.ChannelDeps{
				Decrypt:  box.Open,
				Client:   dingtalkClient,
				BotNames: botNames,
				Logger:   slog.Default(),
			})
			installSvc, installErr := dingtalk.NewInstallService(queries, pool, box, slog.Default())
			if installErr != nil {
				slog.Error("dingtalk: InstallService init failed; install disabled", "error", installErr)
			} else {
				h.DingTalkInstall = installSvc
			}
			slog.Info("dingtalk integration enabled (BYO per-installation stream mode)")
		}
	} else {
		slog.Info("dingtalk integration disabled (ORVILO_DINGTALK_SECRET_KEY not set)")
	}

	// WeCom smart-bot integration ("智能机器人" / aibot). Per-installation
	// WebSocket long connection to wss://openws.work.weixin.qq.com; the
	// Supervisor drives one connection per active wecom installation, gated
	// by the shared ws_lease_token so multi-replica deployments still hold
	// at most one active socket per bot (WeCom itself only permits one).
	//
	// Gated by ORVILO_WECOM_SECRET_KEY. Without it, the whole block is
	// skipped and the wecom Web-UI endpoints return 503; existing deployments
	// are unaffected. The smart-bot flow does NOT require any public HTTP
	// callback, so nothing else needs to be exposed to the internet.
	if wecomKey, err := secretbox.LoadKey("ORVILO_WECOM_SECRET_KEY"); err == nil {
		box, err := secretbox.New(wecomKey)
		if err != nil {
			slog.Error("wecom: secretbox.New failed; wecom integration disabled", "error", err)
		} else {
			credsResolver, err := wecom.NewSecretboxCredentialsResolver(box)
			if err != nil {
				slog.Error("wecom: credentials resolver init failed; wecom integration disabled", "error", err)
			} else {
				wecomStore := wecom.NewStore(queries)
				h.WecomStore = wecomStore
				h.WecomCredentials = credsResolver

				// Binding tokens back the per-user "link your Orvilo account"
				// prompt sent to first-time WeCom senders. aibot userids are
				// anonymized T-prefixed ids with no relation to real userids
				// or emails, so an explicit binding table is the only correct
				// answer — see wecom/binding.go for the rationale.
				wecomBinding := wecom.NewBindingTokenService(queries, pool)
				h.WecomBindingTokens = wecomBinding

				// Senders registry: the wecom OutboundReplier is created here
				// at boot, but the live wsSender it needs to push
				// aibot_send_msg only exists inside a running wecomChannel.
				// wecom.NewSendersRegistry mints a shared map; the
				// ChannelDeps write side and the Replier read side both
				// receive it, and each Channel.Connect self-registers on
				// entry and clears on exit.
				wecomSenders := opts.WecomSenders
				if wecomSenders == nil {
					wecomSenders = wecom.NewSendersRegistry()
				}

				wecomReplier := wecom.NewOutboundReplier(wecom.OutboundReplierConfig{
					Binding: wecomBinding,
					Senders: wecomSenders,
					AppURL:  signupConfig.AppURL,
					Logger:  slog.Default(),
				})

				// Wecom shares the engine.ChatSession (channel_type-keyed) so
				// /issue, dedup, and run-triggering behave identically across
				// platforms. Session titles use the wecom-flavored wording
				// (Chinese product voice — wecom deployments are China-only).
				wecomSession := engine.NewChatSession(queries, pool, wecom.TypeWecom, engine.SessionTitles{
					Group:    "企业微信群聊",
					Direct:   "企业微信单聊",
					Fallback: "企业微信会话",
				})

				wecom.RegisterWecom(channelRegistry, wecom.ChannelDeps{
					Credentials: credsResolver,
					Senders:     wecomSenders,
					Metrics:     wecomMetricsOrNil(opts.WecomMetrics),
					Logger:      slog.Default(),
				})
				// Inbound media: a callback carries a pre-signed COS url and
				// a per-url key, so the resolver needs no WeCom credential —
				// only somewhere durable to put the bytes. Without an object
				// store there is nothing to point an attachment at, so the
				// resolver is left nil and attachments stay as their
				// placeholder text. Same nil-guard as DingTalk above.
				var wecomMedia engine.MediaResolver
				if store != nil {
					wecomMedia = wecom.NewMediaResolver(
						store,
						engine.NewDBMediaIntentLedger(queries),
						wecomSenders,
						slog.Default(),
					)
				}
				channelRouter.Register(wecom.TypeWecom, wecom.NewResolverSet(
					wecomStore, wecomSession, wecomReplier, wecomMedia,
				))

				// EventChatDone subscriber: pushes the agent's chat reply
				// back over the same aibot WebSocket the inbound loop owns.
				// Mirrors slack.NewOutbound(...).Register(bus). Without it
				// the agent's reply lands only in Orvilo's web UI — the
				// user in WeCom sees no response.
				//
				// WithAttachments adds the second hop: the files the agent
				// bound to that reply are read back out of object storage and
				// sent into the chat behind it. Passed only when this
				// deployment configured storage — with none there is nothing
				// to read an attachment out of, and the option is what the
				// delivery path checks for.
				//
				// DeclareChannelFileDelivery is the same condition said to the
				// agent: a run only gets told it can send a file where this
				// branch actually built the hop that sends it. The two lines
				// sit together on purpose — a deployment that has the storage
				// and a deployment whose agents are promised delivery must be
				// the same deployment, and the only way to keep that true is
				// for one `if` to decide both.
				wecomOutboundOpts := []wecom.OutboundOption{}
				if store != nil {
					wecomOutboundOpts = append(wecomOutboundOpts, wecom.WithAttachments(store))
					h.DeclareChannelFileDelivery(string(wecom.TypeWecom))
				}
				// The outbound subscriber reports to the same sink the
				// connection path uses, so an operator reads "replies are
				// being dropped, and for which reason" off the same dashboard
				// as "the bot cannot connect".
				wecomOutboundOpts = append(wecomOutboundOpts,
					wecom.WithOutboundMetrics(wecomMetricsOrNil(opts.WecomMetrics)))
				// Cross-replica routing. Built here rather than in main
				// because the senders registry it guards is created here, and
				// registered back onto the relay so every node's read loop
				// reaches it. See wecom/relay_outbound.go.
				if opts.WecomRelayOutbound != nil {
					opts.WecomRelayOutbound.SetMetrics(wecomMetricsOrNil(opts.WecomMetrics))
					wecomOutboundOpts = append(wecomOutboundOpts, wecom.WithRelay(opts.WecomRelayOutbound))
					slog.Info("wecom integration: cross-replica outbound routing enabled")
				}
				wecomOutbound := wecom.NewOutbound(queries, wecomSenders, slog.Default(), wecomOutboundOpts...)
				wecomOutbound.Register(bus)
				// The dispatcher has been consuming since before this router
				// existed; this is where it learns who performs a delivery.
				// Anything it read in the meantime is waiting in its queue.
				if opts.WecomRelayOutbound != nil {
					opts.WecomRelayOutbound.Attach(wecomOutbound)
				}

				// Ranges the media fetcher may dial despite looking reserved.
				// Empty by default, which leaves the SSRF guard exactly as
				// strict as it ships. A deployment behind a fake-IP proxy
				// needs it: there, every public hostname resolves into the
				// proxy's pool (198.18.0.0/15 is the common one), so WeCom's
				// own COS host is indistinguishable from a metadata endpoint
				// by address alone and every attachment is refused.
				if raw := strings.TrimSpace(os.Getenv("ORVILO_WECOM_MEDIA_ALLOW_CIDRS")); raw != "" {
					for _, err := range wecom.SetMediaAllowedPrefixes(strings.Split(raw, ",")) {
						slog.Error("wecom: ignoring malformed media allow cidr", "error", err)
					}
					slog.Warn("wecom: media guard has an operator allow-list; those ranges are reachable by a URL WeCom supplies",
						"cidrs", raw)
				}

				// Frame tracing: off unless an operator asks for it. It
				// records a bounded prefix of message text, so the fact that
				// it is on has to be visible in the log it is writing into —
				// otherwise a session gets left switched on and nobody
				// notices message content accumulating.
				if wecom.SetTrace(os.Getenv("ORVILO_WECOM_TRACE") == "1") {
					slog.Warn("wecom: frame tracing ON — records message text; unset ORVILO_WECOM_TRACE when done")
				}

				slog.Info("wecom integration enabled (smart bot, long connection)")
				// SINGLE-REPLICA CONSTRAINT: WeCom outbound (agent replies +
				// inbox pushes) is delivered only by the replica holding each
				// bot's in-process WebSocket lease. On a multi-replica
				// deployment, an EventChatDone/EventInboxNew published on another
				// replica cannot reach the lease holder, so those replies are
				// dropped. This is stated conditionally rather than gated on a
				// replica-count signal: the server has no reliable count here,
				// and REDIS_URL means "Redis configured" (it also gates rate
				// limiting), not "more than one replica". See wecom/outbound.go
				// and SELF_HOSTING.md. Remove once outbound routes to the lease
				// holder.
				if opts.WecomRelayOutbound != nil {
					slog.Info("wecom integration: cross-replica outbound routing enabled — agent replies and inbox pushes produced on a replica that does not hold the bot's WebSocket lease are forwarded to the lease holder over the realtime relay. A reply produced while NO replica holds a live connection (every one mid-reconnect) is still lost; see wecom/relay_outbound.go.")
				} else {
					slog.Warn("wecom integration: WeCom agent replies and inbox pushes are delivered only by the replica holding each bot's WebSocket lease. If you run more than one backend replica, responses produced on a replica that does not hold the lease will be dropped — run the WeCom-enabled backend as a single replica, or run a sharded/dual realtime relay (REDIS_URL), which enables cross-replica outbound routing.")
				}
			}
		}
	} else {
		slog.Info("wecom integration disabled (ORVILO_WECOM_SECRET_KEY not set)")
	}

	// Telegram integration. Same shape as Slack: BYO bot token pasted at
	// install, one getUpdates long-polling loop per active installation
	// supervised by the shared engine.Supervisor, resolvers on the generic
	// channel_* tables, outbound streaming via throttled editMessageText on
	// the event bus. Gated by ORVILO_TELEGRAM_SECRET_KEY (the at-rest token
	// encryption key); when unset the handlers return 503 and no Factory is
	// registered.
	if telegramKey, err := secretbox.LoadKey("ORVILO_TELEGRAM_SECRET_KEY"); err == nil {
		box, err := secretbox.New(telegramKey)
		if err != nil {
			slog.Error("telegram: secretbox.New failed; telegram integration disabled", "error", err)
		} else {
			telegramBindingSvc := telegram.NewBindingTokenService(queries, pool)
			h.TelegramBindingTokens = telegramBindingSvc
			telegramReplier := telegram.NewOutboundReplier(telegram.OutboundReplierConfig{
				Binding: telegramBindingSvc,
				Decrypt: box.Open,
				// The bind link (/telegram/bind) is a web-app page: app URL, not
				// the API URL. Mirrors the Slack replier.
				AppURL: signupConfig.AppURL,
				Logger: slog.Default(),
			})
			telegramTyping := telegram.NewTypingNotifier(box.Open, "", nil, slog.Default())
			channelRouter.Register(telegram.TypeTelegram, telegram.NewTelegramResolverSet(queries, pool, telegramReplier, telegramTyping))
			telegramOutbound := telegram.NewOutbound(queries, box.Open, "", nil, slog.Default())
			telegramOutbound.Register(bus)
			workers.Telegram = telegramOutbound
			h.TelegramOutbound = telegramOutbound

			// Per-installation inbound: the Supervisor builds + supervises one
			// long-polling loop per active Telegram installation.
			telegram.RegisterTelegram(channelRegistry, telegram.ChannelDeps{Decrypt: box.Open, Logger: slog.Default()})

			installSvc, ierr := telegram.NewInstallService(queries, pool, box, slog.Default())
			if ierr != nil {
				slog.Error("telegram: InstallService init failed; install disabled", "error", ierr)
			} else {
				h.TelegramInstall = installSvc
			}
			slog.Info("telegram integration enabled (per-installation long polling)")
		}
	} else {
		slog.Info("telegram integration disabled (ORVILO_TELEGRAM_SECRET_KEY not set)")
	}

	// Native Weixin/iLink integration. This is the personal-WeChat QR + HTTP
	// long-poll adapter, not the separate
	// corporate WeCom WebSocket adapter. It reuses the generic channel tables,
	// shared engine session/dedup pipeline, and the same installation lease.
	// The at-rest key gates all provider wiring; handlers remain registered and
	// return a clear 503 when an operator has not opted in.
	if weixinKey, err := secretbox.LoadKey("ORVILO_WEIXIN_SECRET_KEY"); err == nil {
		box, err := secretbox.New(weixinKey)
		if err != nil {
			slog.Error("weixin: secretbox.New failed; weixin integration disabled", "error", err)
		} else {
			weixinBinding := weixin.NewBindingTokenService(queries, pool)
			weixinReplier := weixin.NewOutboundReplier(weixin.OutboundReplierConfig{
				Binding: weixinBinding, Decrypt: box.Open, AppURL: signupConfig.AppURL, Logger: slog.Default(),
			})
			channelRouter.Register(weixin.TypeWeixin, weixin.NewResolverSet(
				queries, pool, pool, weixinReplier, box.Seal,
			))
			weixinOutbound := weixin.NewOutbound(queries, box.Open, slog.Default())
			weixinOutbound.Register(bus)
			weixin.RegisterWeixin(channelRegistry, weixin.ChannelDeps{
				Decrypt: box.Open, Pool: pool, Logger: slog.Default(),
			})
			slog.Info("weixin integration enabled (iLink QR + HTTP long polling)")
		}
	} else {
		slog.Info("weixin integration disabled (ORVILO_WEIXIN_SECRET_KEY not set)")
	}

	// Composio integration (MUL-3720). Gated by COMPOSIO_API_KEY plus the
	// composio_mcp_apps feature flag. The env var is the project-scoped key the
	// standalone SDK authenticates Composio with (sent as x-api-key; the project
	// is resolved from the key, so NO project id is configured). When unset or
	// flag-disabled the whole block is skipped and the composio HTTP handlers
	// return 503; existing deployments are unaffected. An operator opts in by
	// setting COMPOSIO_API_KEY plus a callback base
	// (COMPOSIO_CALLBACK_BASE_URL, falling back to ORVILO_PUBLIC_URL). The
	// toolkit→auth-config mapping is NOT configured here — it is resolved
	// dynamically from the project's /auth_configs at request time, so enabling
	// a toolkit is a dashboard action, not a redeploy. State signing uses
	// COMPOSIO_STATE_SECRET, or a key derived from JWT_SECRET when that is unset.
	if composioAPIKey := strings.TrimSpace(os.Getenv("COMPOSIO_API_KEY")); composioAPIKey != "" {
		if !featureflags.ComposioMCPAppsEnabled(context.Background(), opts.FeatureFlags) {
			slog.Info("composio integration disabled (feature flag off)")
		} else {
			sdkClient, err := composiosdk.NewClient(composiosdk.Options{APIKey: composioAPIKey})
			if err != nil {
				slog.Error("composio: SDK client init failed; composio integration disabled", "error", err)
			} else {
				stateSecret := composioStateSecret()
				callbackBase := composioCallbackBaseURL(signupConfig.PublicURL)
				switch {
				case len(stateSecret) == 0:
					slog.Error("composio: no state secret (set COMPOSIO_STATE_SECRET or JWT_SECRET); composio integration disabled")
				case callbackBase == "":
					slog.Error("composio: no callback base url (set COMPOSIO_CALLBACK_BASE_URL or ORVILO_PUBLIC_URL); composio integration disabled")
				default:
					svc, serr := composiointeg.NewService(sdkClient, queries, composiointeg.Config{
						StateSecret:     stateSecret,
						CallbackBaseURL: callbackBase,
						FrontendBaseURL: signupConfig.AppURL,
					})
					if serr != nil {
						slog.Error("composio: service init failed; composio integration disabled", "error", serr)
					} else {
						h.Composio = svc
						// Stage 3 (MUL-3721) hook: feed the per-task MCP
						// overlay builder into TaskService so every Enqueue*
						// path attaches the initiator user's Composio session
						// URL to the task row before the daemon claims it.
						// taskSvc already exists by this point — it was
						// constructed inside NewHandler — and exposes its
						// Composio field for exactly this kind of late wiring,
						// so no Handler-level mutation is needed.
						if h.TaskService != nil {
							h.TaskService.Composio = svc
						}
						slog.Info("composio integration enabled")
					}
				}
			}
		}
	} else {
		slog.Info("composio integration disabled (COMPOSIO_API_KEY not set)")
	}

	// VCS at-rest encryption: the box encrypts per-workspace access tokens and
	// webhook secrets for token-based providers (Forgejo / Gitea / GitLab).
	// Without it, connect/webhook handlers return 503 (so a misconfigured
	// self-host never stores plaintext secrets).
	if vcsKey, err := secretbox.LoadKey("ORVILO_VCS_SECRET_KEY"); err == nil {
		box, err := secretbox.New(vcsKey)
		if err != nil {
			slog.Error("vcs: secretbox.New failed; vcs integration disabled", "error", err)
		} else {
			h.VCSSecretBox = box
			slog.Info("vcs integration enabled")
		}
	} else {
		slog.Info("vcs integration disabled (ORVILO_VCS_SECRET_KEY not set)")
	}

	// Linear OAuth credentials and the at-rest key form one fail-closed unit.
	// Feature-flag exposure is separate, allowing operators to provision and
	// validate secrets before enabling the product surface.
	if linearKey, err := secretbox.LoadKey("ORVILO_LINEAR_SECRET_KEY"); err == nil {
		box, boxErr := secretbox.New(linearKey)
		if boxErr != nil {
			slog.Error("linear: secretbox.New failed; integration disabled", "error", boxErr)
		} else {
			h.LinearSecretBox = box
			h.LinearClientID = strings.TrimSpace(os.Getenv("LINEAR_CLIENT_ID"))
			h.LinearClientSecret = strings.TrimSpace(os.Getenv("LINEAR_CLIENT_SECRET"))
			h.LinearWebhookSecret = strings.TrimSpace(os.Getenv("LINEAR_WEBHOOK_SECRET"))
			h.LinearPullEnabled = envBool("ORVILO_LINEAR_PULL_IMPORT_ENABLED", true)
			h.LinearPushEnabled = envBool("ORVILO_LINEAR_PUSH_ENABLED", false)
			if h.LinearClientID == "" || h.LinearClientSecret == "" {
				slog.Warn("linear OAuth credentials incomplete; integration disabled")
				h.LinearSecretBox = nil
			} else {
				provider := linearapi.NewHTTPClient(nil)
				h.LinearProvider = provider
				workers.Linear = linearsync.NewWorker(pool, pool, box, provider, h.LinearClientID, h.LinearClientSecret, h.LinearPullEnabled, h.LinearPushEnabled, handler.NewLinearEventSink(bus))
				h.LinearWorker = workers.Linear
				if h.LinearWebhookSecret == "" {
					slog.Warn("linear webhook secret is not configured; OAuth/sync polling remain available")
				}
				linearWorker := workers.Linear
				bus.Subscribe(protocol.EventIssueCreated, func(events.Event) { linearWorker.Wake() })
				bus.Subscribe(protocol.EventIssueUpdated, func(events.Event) { linearWorker.Wake() })
				bus.Subscribe(protocol.EventIssueDeleted, func(events.Event) { linearWorker.Wake() })
				slog.Info("linear integration configured")
			}
		}
	} else {
		slog.Info("linear integration disabled (ORVILO_LINEAR_SECRET_KEY not set)")
	}

	// Plugin secrets use a dedicated deployment key. Keeping this separate from
	// VCS and channel secrets gives operators an isolated rotation and blast
	// radius; without it, saving a `secret` config field fails closed rather
	// than storing plaintext.
	if pluginKey, err := secretbox.LoadKey("ORVILO_PLUGIN_SECRET_KEY"); err == nil {
		box, err := secretbox.New(pluginKey)
		if err != nil {
			slog.Error("plugins: secretbox.New failed; Plugin secrets disabled", "error", err)
		} else if h.PluginService != nil {
			h.PluginService.Secrets = box
			// The same deployment key, kept raw as well. Sealing and signing
			// need different things from it: a secret config value is sealed
			// and later opened, while a hook signature must be REPRODUCED on
			// demand, which a box cannot do. Each installation's signing secret
			// is derived from this rather than stored, so no row holds a usable
			// one.
			h.PluginService.DeploymentKey = pluginKey
			h.PluginSurfaceTokens, err = handler.NewPluginSurfaceTokenBox(pluginKey)
			if err != nil {
				slog.Error("plugins: surface token key derivation failed; surfaces disabled", "error", err)
			}
			slog.Info("Plugin secret encryption enabled")
		}
	} else {
		slog.Info("Plugin secrets disabled (ORVILO_PLUGIN_SECRET_KEY not set)")
	}

	// Hook engine. Event-triggered hooks are dispatched off the bus onto a
	// worker pool: Bus.Publish runs listeners inline on the publishing request's
	// goroutine, so anything that dials a third-party endpoint from there would
	// put an outside server on the critical path of creating an issue.
	if h.PluginService != nil {
		h.PluginService.Callbacks = service.NewCallbackTokens()
		// Omitted rather than sent relative: a third-party hook server cannot
		// call a path-only URL, and a broken absolute URL is harder to diagnose
		// than an absent one.
		if baseURL := pluginActionBaseURL(signupConfig.PublicURL); baseURL != "" {
			h.PluginService.CallbackBaseURL = baseURL
		} else {
			slog.Warn("plugins: ORVILO_PLUGIN_API_URL and ORVILO_PUBLIC_URL are not set; hook callbacks will carry no callback_url")
		}
		// The flag reaches the event path only through the service: a worker has
		// no request to read it from.
		h.PluginService.FeatureFlags = h.FeatureFlags
		pluginEvents := service.NewPluginEventDispatcher(h.PluginService)
		service.SubscribePluginEvents(bus, pluginEvents)
	}

	if opts.HeartbeatScheduler != nil {
		h.HeartbeatScheduler = opts.HeartbeatScheduler
	}
	// Auth caches: PAT cache is shared between the regular Auth middleware,
	// the DaemonAuth fallback (ovy_) path, and the revoke handler
	// (invalidate). DaemonTokenCache backs the DaemonAuth mdt_ path. Both
	// constructors return nil when rdb is nil — every consumer handles that
	// as "no cache, always hit DB".
	patCache := auth.NewPATCache(rdb)
	daemonTokenCache := auth.NewDaemonTokenCache(rdb)
	h.PATCache = patCache
	h.DaemonTokenCache = daemonTokenCache
	h.MembershipCache = auth.NewMembershipCache(rdb)

	// Cloud PAT verifier: validates mcn_ tokens against Orvilo Cloud
	// Fleet. Returns nil when no Cloud URL is configured — the Auth /
	// DaemonAuth middlewares treat nil as "mcn_ not supported" and
	// reject with 401, instead of falling through to ovy_/JWT paths.
	// Reuses ORVILO_CLOUD_URL (the same URL the cloud-runtime proxy uses) so a
	// deployment has one authoritative orvilo-cloud connection.
	cloudPATVerifier := auth.NewCloudPATVerifier(auth.CloudPATVerifierConfig{
		FleetBaseURL: signupConfig.CloudURL,
		Redis:        rdb,
	})

	// Empty-claim cache: lets the daemon poll path skip a Postgres
	// scan when a recent check confirmed the runtime had no queued
	// task. Returns nil when rdb is nil — TaskService treats that
	// as "no cache, always hit DB" (existing behavior).
	h.TaskService.EmptyClaim = service.NewEmptyClaimCache(rdb)
	// Stale-dispatch reclaim has a separate schedule because an empty queued
	// verdict cannot represent a claim response lost after commit. Missing or
	// failed Redis state keeps the historical PostgreSQL fallback.
	h.TaskService.ReclaimCheck = service.NewReclaimCheckCache(rdb)

	// Wire WS heartbeat after stores are finalized so the WS path uses the
	// same (possibly Redis-backed) stores as the HTTP path.
	daemonHub.SetHeartbeatHandler(h.HandleDaemonWSHeartbeat)
	// WS-first claim (MUL-4257): route daemon:rpc_request frames (e.g.
	// tasks.claim) through the same handlers as the HTTP endpoints.
	daemonHub.SetRPCHandler(h.DaemonRPCHandler)

	workers.Webhooks = handler.NewWebhookDeliveryWorker(queries, services.Automations, h.WebhookRateLimiter, h.Metrics)
	h.WebhookDeliveryWorker = workers.Webhooks
	ghClient, err := ghsnapshot.NewClientFromEnv()
	if err != nil {
		slog.Warn("github: PR snapshot pipeline disabled (invalid App private key)", "err", err)
	}
	workers.PRRefresh = ghsnapshot.NewManager(ghClient, queries, pool, handler.NewPRSnapshotEventSink(queries, bus, ghClient.Enabled()))
	h.PRRefresh = workers.PRRefresh
	workers.WorkProducts = handler.NewWorkProductDiscoveryRuntime(queries, pool, pool, workers.PRRefresh, bus)
	h.WorkProductDiscovery = workers.WorkProducts
	workers.Liveness = h.LivenessStore
	workers.Heartbeats, _ = h.HeartbeatScheduler.(heartbeatLifecycle)
	workers.RuntimeGC = handler.NewRuntimeEventPublisher(store, signupConfig.PublicURL, cfSigner != nil, services.Tasks, bus)
	h.SeatCapacityWorker = workers.SeatCapacity
	h.HostedCapacityWorker = workers.HostedCapacity
	h.ManagedSlackTokens = workers.SlackTokens
	h.ChannelMediaReconciler = workers.ChannelMedia
	return &application{
		messagingMode: messagingMode, HTTP: h, queries: queries, bus: bus, Services: services, Workers: workers, pool: pool, hub: hub, redis: rdb,
		cloudPATVerifier: cloudPATVerifier,
		routes: routeSettings{Origins: origins, Config: signupConfig, HTTPMetrics: opts.HTTPMetrics,
			RealtimeMetricsToken: os.Getenv("REALTIME_METRICS_TOKEN"),
			TrustedRateProxies:   middleware.ParseTrustedProxies(os.Getenv("RATE_LIMIT_TRUSTED_PROXIES")),
			AuthRate:             envPositiveInt("RATE_LIMIT_AUTH", 5), VerifyRate: envPositiveInt("RATE_LIMIT_AUTH_VERIFY", 20),
			HandoffRate: envPositiveInt("RATE_LIMIT_DESKTOP_HANDOFF", 20), ContactRate: envPositiveInt("RATE_LIMIT_CONTACT_SALES", 5),
			DevLoginRate: envPositiveInt("RATE_LIMIT_DEV_LOGIN", 60), DeviceAuthRate: envPositiveInt("RATE_LIMIT_DEVICE_AUTH", 120),
			PluginRate: envPositiveInt("RATE_LIMIT_PLUGIN_API", 120)},
	}
}

func assembleServices(queries *db.Queries, pool *pgxpool.Pool, hub *realtime.Hub, bus *events.Bus, analyticsClient analytics.Client, store storage.Storage, cfg handler.Config, daemonHub *daemonws.Hub) handler.Services {
	if analyticsClient == nil {
		analyticsClient = analytics.NoopClient{}
	}
	llmClient := llm.New(llm.Config{APIKey: cfg.LLMAPIKey, BaseURL: cfg.LLMBaseURL, DefaultModel: cfg.LLMDefaultModel, MaxRetries: cfg.LLMMaxRetries})
	retry := llmClient.RetryBudget()
	slog.Info("llm retry policy", "max_retries", retry.MaxRetries, "source", retry.Source, "request_timeout", retry.RequestTimeout, "enabled", llmClient.Enabled())
	tasks := service.NewTaskService(queries, pool, hub, bus, daemonHub)
	tasks.Analytics, tasks.SourceContextStorage, tasks.QuickActions = analyticsClient, store, llmClient
	coordination := service.NewAgentCoordinationService(queries, pool, bus, tasks.PublishQueuedTask)
	tasks.Coordination = coordination
	return handler.Services{Tasks: tasks, Coordination: coordination, Issues: service.NewIssueService(queries, pool, bus, analyticsClient, tasks), Automations: service.NewAutomationService(queries, pool, bus, tasks), Plugins: service.NewPluginService(queries, pool), ProviderAuthorization: service.NewProviderAuthorizationService(queries), LLM: llmClient}
}

func registerApplicationListeners(bus *events.Bus, pool *pgxpool.Pool, queries *db.Queries) {
	// Subscribers must exist before notification delivery reads them.
	registerSubscriberListeners(bus, pool)
	registerActivityListeners(bus, queries)
	registerNotificationListeners(bus, queries)
}
