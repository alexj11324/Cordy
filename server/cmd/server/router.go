package main

import (
	"context"
	"log/slog"
	"net/http"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	chimw "github.com/go-chi/chi/v5/middleware"
	"github.com/go-chi/cors"

	"github.com/orvilo-ai/orvilo/server/internal/handler"
	"github.com/orvilo-ai/orvilo/server/internal/middleware"
	"github.com/orvilo-ai/orvilo/server/internal/realtime"
	"github.com/orvilo-ai/orvilo/server/internal/storage"
	"github.com/orvilo-ai/orvilo/server/internal/util"
	publicapiv1 "github.com/orvilo-ai/orvilo/server/pkg/publicapi/v1"
)

var defaultOrigins = []string{
	"http://localhost:3000", // Next.js dev
	"http://localhost:5173", // electron-vite dev
	"http://localhost:5174", // electron-vite dev (fallback port)
}

// corsAllowedHeaders must list every header the browser clients send. A header
// missing here fails the preflight, so the request never reaches the handler at
// all — the failure looks nothing like "the server ignored my header".
// X-Client-Capabilities in particular was daemon-only (a Go client, never
// preflighted) until the web app started advertising chat-draft-restore-v1 on
// cancel.
var corsAllowedHeaders = []string{
	"Accept",
	"Authorization",
	"Content-Type",
	"Idempotency-Key",
	"If-Match",
	"X-Workspace-ID",
	"X-Workspace-Slug",
	"X-Request-ID",
	"X-Agent-ID",
	"X-Task-ID",
	"X-CSRF-Token",
	"X-Client-Platform",
	"X-Client-Version",
	"X-Client-OS",
	"X-Client-Capabilities",
	// Sent by the host page when it relays a plugin surface's Action API call.
	"X-Orvilo-Plugin-Installation",
}

// corsExposedHeaders lists response headers browser clients are allowed to read.
// Without this a custom response header is silently unreadable from JS on a
// cross-origin request (only the CORS-safelisted response headers are exposed by
// default) — the header arrives on the wire and then disappears, which looks
// exactly like the server never sent it.
//
// Referencing the handler constant rather than re-typing the string keeps a
// rename from quietly switching the signal off (MUL-5492).
var corsExposedHeaders = []string{
	"ETag",
	"X-Request-ID",
	handler.HeaderCommentsTruncated,
	handler.HeaderTimelineTruncated,
}

func registerPluginActionRoutes(r chi.Router, h *handler.Handler) {
	r.Get(publicapiv1.PathContext, h.GetPluginContext)
	r.Get(publicapiv1.PathIssue, h.GetPluginIssue)
	r.Patch(publicapiv1.PathIssue, h.PatchPluginIssue)
	r.Get(publicapiv1.PathIssueComments, h.ListPluginComments)
	r.Post(publicapiv1.PathIssueComments, h.CreatePluginComment)
	r.Get(publicapiv1.PathStorageScope, h.ListPluginStorage)
	r.Get(publicapiv1.PathStorageValue, h.GetPluginStorage)
	r.Put(publicapiv1.PathStorageValue, h.PutPluginStorage)
	r.Delete(publicapiv1.PathStorageValue, h.DeletePluginStorage)
}

func buildFingerprintMiddleware(buildVersion, buildCommit string) func(http.Handler) http.Handler {
	buildVersion = normalizeServerVersion(strings.TrimSpace(buildVersion))
	buildCommit = strings.TrimSpace(buildCommit)
	if buildCommit == "unknown" {
		buildCommit = ""
	}
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if buildVersion != "" {
				w.Header().Set("X-Orvilo-Build", buildVersion)
			}
			if buildCommit != "" {
				w.Header().Set("X-Orvilo-Commit", buildCommit)
			}
			next.ServeHTTP(w, r)
		})
	}
}

// NewRouter creates the fully-configured Chi router with all middleware and routes.
// rdb is optional: when non-nil the runtime local-skill request stores are
// swapped for Redis-backed implementations so multiple API nodes share the
// same pending queue (required for multi-node prod). This should be a request
// path Redis client, not the realtime relay's blocking read client. A nil rdb
// keeps the default in-memory stores which are fine for single-node dev and
// tests.
func newRouter(app *application) chi.Router {
	h, pool, hub, rdb := app.HTTP, app.pool, app.hub, app.redis
	queries, store, cfSigner := h.Queries, h.Storage, h.CFSigner
	settings := app.routes
	signupConfig, origins := settings.Config, settings.Origins
	patCache, daemonTokenCache := h.PATCache, h.DaemonTokenCache
	cloudPATVerifier := app.cloudPATVerifier
	health := newServerHealth(pool)

	r := chi.NewRouter()

	// Global middleware
	r.Use(chimw.RequestID)
	r.Use(middleware.ClientMetadata)
	r.Use(middleware.RequestLogger)
	if settings.HTTPMetrics != nil {
		r.Use(settings.HTTPMetrics.Middleware)
	}
	r.Use(buildFingerprintMiddleware(version, commit))
	r.Use(chimw.Recoverer)
	r.Use(h.PluginSurfaceHostBoundary)
	r.Use(middleware.ContentSecurityPolicy)

	// Share allowed origins with WebSocket origin checker.
	realtime.SetAllowedOrigins(origins)

	// Share the same trusted-proxy CIDRs (ORVILO_TRUSTED_PROXIES) so the
	// WebSocket origin check honors X-Forwarded-Host only from trusted proxies,
	// using one config source instead of a parallel one.
	realtime.SetTrustedProxies(signupConfig.TrustedProxies)

	r.Use(cors.Handler(cors.Options{
		AllowedOrigins:   origins,
		AllowedMethods:   []string{"GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"},
		AllowedHeaders:   corsAllowedHeaders,
		ExposedHeaders:   corsExposedHeaders,
		AllowCredentials: true,
		MaxAge:           300,
	}))

	// Health / readiness checks
	r.Get("/health", health.liveHandler)
	r.Get("/readyz", health.readyHandler)
	r.Get("/healthz", health.readyHandler)

	// Realtime subsystem metrics — connection counts, slow-client evictions,
	// and per-event-type send QPS counters. Exposed as JSON so it can be
	// scraped by ops or surfaced in the admin UI without adding a Prometheus
	// dependency. See MUL-1138 (Phase 0).
	//
	// Access is restricted (MUL-1342): when REALTIME_METRICS_TOKEN is set,
	// callers must present it via Authorization: Bearer <token>. When the
	// env var is unset the handler only serves loopback callers so local
	// dev keeps working without exposing the metrics on a public listener.
	r.Get("/health/realtime", realtimeMetricsHandler(settings.RealtimeMetricsToken))

	// WebSocket
	mc := &membershipChecker{queries: queries}
	pr := &opaqueTokenResolver{queries: queries, cache: patCache}
	slugResolver := realtime.SlugResolver(func(ctx context.Context, slug string) (string, error) {
		ws, err := queries.GetWorkspaceBySlug(ctx, slug)
		if err != nil {
			return "", err
		}
		return util.UUIDToString(ws.ID), nil
	})
	r.Get("/ws", func(w http.ResponseWriter, r *http.Request) {
		realtime.HandleWebSocket(hub, mc, pr, slugResolver, w, r)
	})

	// Local file serving (when using local storage). Served through the
	// handler so /uploads/* carries the same preview security headers as the
	// /api/attachments download endpoint; self-hosted split-origin/same-origin
	// clients can then iframe-preview PDFs/HTML fetched straight from the
	// static route instead of hitting the global frame-ancestors 'none' CSP.
	// See MUL-3821 / #4477.
	if _, ok := store.(*storage.LocalStorage); ok {
		r.Get("/uploads/*", h.ServeLocalUpload)
	}

	// Capability-authenticated attachment download (MUL-5292). Public by
	// necessity: a native download (Electron's webContents.downloadURL, a
	// cross-site webview <img>) carries neither Authorization nor a session
	// cookie, so there is nothing here for middleware.Auth to read. The
	// short-lived, single-attachment signature in the query is the credential,
	// and it is only ever minted by the AUTHENTICATED GET
	// /api/attachments/{id} after that request's membership check passed.
	// The authenticated /api/attachments/{id}/download route below is
	// unchanged — this one is purely additive.
	r.Get("/api/attachments/{id}/signed-download", h.DownloadAttachmentWithCapability)

	// Avatar serving. Public for the same reason as the capability download
	// above: the auth cookie is SameSite=Strict, so an auth-gated URL cannot
	// be a native <img src> from Desktop / mobile webview or a split-origin
	// self-hosted web app. The HMAC signature in the path is the credential.
	// It covers the storage key, only image keys resolve, and the object must
	// be avatar-class — see server/internal/handler/avatar.go (MUL-5393 /
	// #6024).
	r.Get("/api/avatars/{sig}/*", h.ServeAvatar)

	// Hosted plugin documents are capability-authenticated and intentionally
	// outside the session middleware: the configured content origin must stay
	// cookie-free. The encrypted path token binds the installation, immutable
	// version, surface and one bridge challenge.
	r.Get("/plugin-surfaces/{token}", h.ServePluginSurface)

	// Auth (public) — per-IP rate limiting.
	if rdb == nil {
		slog.Warn("auth rate limiting disabled: REDIS_URL not configured")
	}
	trustedProxies := settings.TrustedRateProxies
	authRL := middleware.RateLimit(rdb, settings.AuthRate, time.Minute, trustedProxies)
	authVerifyRL := middleware.RateLimit(rdb, settings.VerifyRate, time.Minute, trustedProxies)
	deviceAuthRL := middleware.RateLimit(rdb, settings.DeviceAuthRate, time.Minute, trustedProxies)
	desktopHandoffRL := middleware.RateLimit(rdb, settings.HandoffRate, time.Minute, trustedProxies)
	contactSalesRL := middleware.RateLimit(rdb, settings.ContactRate, time.Hour, trustedProxies)
	r.With(authRL).Post("/auth/send-code", h.SendCode)
	r.With(authRL).Post("/auth/clerk", h.ClerkLogin)
	r.With(authVerifyRL).Post("/auth/verify-code", h.VerifyCode)
	// Device authorization is public on the CLI side: the device code is
	// intentionally useless until an authenticated browser approves it. Keep
	// both issuance and polling behind a dedicated per-IP budget; the polling
	// interval is longer than the normal auth request budget.
	r.With(deviceAuthRL).Post("/api/auth/device/code", h.CreateDeviceAuthorization)
	r.With(deviceAuthRL).Post("/api/auth/device/token", h.ExchangeDeviceAuthorizationToken)
	// Google is retained only as the exchange leg for explicit Desktop/CLI
	// broker flows; the Web login page uses email send-code and does not expose
	// this endpoint as its primary sign-in path.
	r.With(authRL).Post("/auth/google", h.GoogleLogin)
	// Development sign-in. Registered only when ORVILO_DEV_LOGIN=1 and
	// APP_ENV is non-production, so an ordinary deployment does not serve the
	// path at all — see server/internal/handler/dev_login.go. It gets its own
	// limiter rather than authRL: this endpoint exists to escape login
	// throttling, and one `make dev-login` already spends two requests (the
	// POST for the token, the GET when the printed URL is opened), so the
	// 5/min auth budget would 429 the third run of the very workflow it is
	// for. There is no credential to brute-force here — the endpoint is a
	// deliberate bypass — so the limit only stops accidental hammering.
	if handler.DevLoginEnabled() {
		devLoginRL := middleware.RateLimit(rdb, settings.DevLoginRate, time.Minute, trustedProxies)
		r.With(devLoginRL).HandleFunc("/auth/dev-login", h.DevLogin)
	}
	r.With(authRL).Post("/auth/guest", h.CreateGuestAuth)
	r.With(desktopHandoffRL).Post("/api/desktop-identity/redeem", h.RedeemDesktopLocalIdentity)
	r.With(desktopHandoffRL).Post("/api/desktop-handoff/initiate", h.InitiateDesktopAuthHandoff)
	r.With(desktopHandoffRL).Post("/api/desktop-handoff/redeem", h.RedeemDesktopAuthHandoff)
	r.With(handler.RequireDesktopBrokerAuth(signupConfig.DesktopBrokerAuthToken), desktopHandoffRL).Post("/api/desktop-google/attempt", h.RegisterDesktopGoogleAttempt)
	r.With(handler.RequireDesktopBrokerAuth(signupConfig.DesktopBrokerAuthToken), desktopHandoffRL).Post("/api/desktop-google/complete", h.CompleteDesktopGoogleAttempt)
	r.With(middleware.RevokeGuestOnLogout(queries)).Post("/auth/logout", h.Logout)

	// Public API
	r.Get("/api/config", h.GetConfig)
	r.With(contactSalesRL).Post("/api/contact-sales", h.CreateContactSales)
	// Public share-link preview — no auth: shows the workspace name/slug and
	// inviter so a not-yet-logged-in visitor can see what they're joining.
	r.Get("/api/share-links/{code}", h.GetShareLinkInfo)

	// Webhook ingress for automations. Outside the authenticated group on
	// purpose: the bearer token in the URL path IS the credential. Workspace
	// context is derived from the trigger row, never from request headers.
	r.Post("/api/webhooks/automations/{token}", h.HandleAutomationWebhook)
	// GitHub App webhook (no Orvilo auth — requests are authenticated via
	// HMAC-SHA256 signature in the handler) and post-install setup callback.
	r.Post("/api/webhooks/github", h.HandleGitHubWebhook)
	r.Get("/api/github/setup", h.GitHubSetupCallback)
	// Slack managed-OAuth callback (no Orvilo auth in the path — it is hit by
	// Slack's browser redirect, which carries no session; the workspace and
	// installer are recovered from the single-use state token). It consumes the
	// state, exchanges the code, upserts the team-keyed install, then 302s the
	// browser to the redirect_url bound to the state.
	r.Get("/api/integrations/slack/oauth/callback", h.ManagedSlackOAuthCallback)
	// Slack managed Events API webhook (no Orvilo auth — authenticity is the
	// HMAC-SHA256 request signature; tenant routing is the event's api_app_id
	// + team_id). The handler nil-checks: without the Slack block above there
	// is no webhook and this 503s instead of panicking.
	r.Post("/api/integrations/slack/events", h.ManagedSlackEvents)
	// Slash invocations for managed installs (same authenticity story as the
	// events webhook; replay protection is the trigger_id claim). Nil-checks
	// like the events route above.
	r.Post("/api/integrations/slack/commands", h.ManagedSlackCommands)
	// VCS webhook for token-based providers (Forgejo / Gitea / GitLab). No Orvilo
	// auth — authenticated per-connection by the provider's signature scheme;
	// the connection id in the path selects the workspace, provider, and
	// decryption secret.
	r.Post("/api/webhooks/vcs/{connectionId}", h.HandleVCSWebhook)
	// Stripe webhook (no Orvilo auth — Stripe signs the raw body
	// with a shared secret, the orvilo-cloud upstream verifies. We
	// only forward the bytes + the Stripe-Signature header; see
	// HandleCloudBillingStripeWebhook for the rationale).
	r.Post("/api/webhooks/stripe", h.HandleCloudBillingStripeWebhook)

	// Composio OAuth callback (MUL-3843). NOT under the Auth group on purpose:
	// Composio 302-redirects the user's browser here at the end of the OAuth
	// flow, and the cookie session is frequently absent (expired session,
	// SameSite=Strict / Safari ITP stripping cross-site cookies, private
	// windows, self-hosted callbacks on a different subdomain). Identity is NOT
	// taken from the session — it comes from the HMAC-signed `state` query
	// param, which CompleteCallback verifies (signature, expiry, replay) before
	// doing anything. h.Composio == nil still returns 503. Keeping it inside the
	// Auth group made a missing cookie a hard 401, breaking the flow for exactly
	// the browsers above; the other four composio endpoints stay session-gated.
	r.Get("/api/integrations/composio/callback", h.ComposioCallback)
	// Linear redirects and webhooks carry their own short-lived state or HMAC
	// credential and therefore cannot depend on an Orvilo browser session.
	r.Get("/api/linear/oauth/callback", h.LinearOAuthCallback)
	r.Post("/api/webhooks/linear", h.HandleLinearWebhook)

	// Daemon API routes (require daemon token or valid user token)
	r.Route("/api/daemon", func(r chi.Router) {
		r.Use(middleware.DaemonAuth(queries, patCache, daemonTokenCache, cloudPATVerifier))

		r.Post("/register", h.DaemonRegister)
		r.Post("/deregister", h.DaemonDeregister)
		r.Post("/heartbeat", h.DaemonHeartbeat)
		r.Get("/ws", h.DaemonWebSocket)
		r.Get("/workspaces", h.ListDaemonWorkspaces)
		r.Get("/workspaces/{workspaceId}/repos", h.GetDaemonWorkspaceRepos)
		r.Get("/workspaces/{workspaceId}/runtime-profiles", h.DaemonListRuntimeProfiles)

		// Agent-triggered plugin hooks. The daemon's local MCP server calls
		// this when an agent picks one of its tools; the server makes the
		// signed request so the daemon never holds the signing secret.
		r.Post("/tasks/{id}/plugin-hooks", h.InvokeAgentPluginHook)
		// The broker asks for an mcp hook's credential at connection time, so
		// a secret never sits in a task record.
		r.Get("/tasks/{id}/plugin-mcp/{contributionId}/credential", h.ResolvePluginMCPCredential)

		r.Post("/runtimes/{runtimeId}/tasks/claim", h.ClaimTaskByRuntime)
		// Canonical machine-level batch claim (MUL-4257). `/claim` is a
		// transitional alias; the daemon coordinator targets the canonical
		// path.
		r.Post("/tasks/claim", h.ClaimTasksByRuntime)
		r.Post("/claim", h.ClaimTasksByRuntime)
		r.Post("/runtimes/{runtimeId}/tasks/{taskId}/prepare-lease", h.ExtendTaskPrepareLease)
		// Pre-operation gate for anything that spends the runtime's provider
		// credential. The daemon calls it with the capability lease it was
		// handed at claim time; 200 is the only answer that authorizes the
		// operation to run.
		r.Post("/runtimes/{runtimeId}/tasks/{taskId}/provider-authorization", h.AuthorizeProviderOperation)
		r.Post("/runtimes/{runtimeId}/tasks/{taskId}/skill-bundles/resolve", h.ResolveTaskSkillBundles)
		r.Get("/runtimes/{runtimeId}/tasks/pending", h.ListPendingTasksByRuntime)
		r.Post("/runtimes/{runtimeId}/update/{updateId}/result", h.ReportUpdateResult)
		r.Post("/runtimes/{runtimeId}/models/{requestId}/result", h.ReportModelListResult)
		r.Post("/runtimes/{runtimeId}/local-skills/{requestId}/result", h.ReportLocalSkillListResult)
		r.Post("/runtimes/{runtimeId}/local-skills/import/{requestId}/result", h.ReportLocalSkillImportResult)

		r.Get("/tasks/{taskId}/status", h.GetTaskStatus)
		r.Post("/tasks/{taskId}/start", h.StartTask)
		r.Post("/tasks/{taskId}/execution-provenance", h.RecordTaskExecutionProvenance)
		r.Post("/tasks/{taskId}/wait-local-directory", h.MarkTaskWaitingLocalDirectory)
		r.Post("/tasks/{taskId}/progress", h.ReportTaskProgress)
		r.Post("/tasks/{taskId}/complete", h.CompleteTask)
		r.Post("/tasks/{taskId}/fail", h.FailTask)
		r.Post("/tasks/{taskId}/usage", h.ReportTaskUsage)
		r.Post("/tasks/{taskId}/messages", h.ReportTaskMessages)
		r.Get("/tasks/{taskId}/messages", h.ListTaskMessages)
		r.Post("/tasks/{taskId}/cancel-ack", h.AckTaskCancelled)

		r.Post("/workspaces/{workspaceId}/issues/gc-check", h.BatchIssueGCCheck)
		r.Get("/issues/{issueId}/gc-check", h.GetIssueGCCheck)
		r.Get("/chat-sessions/{sessionId}/gc-check", h.GetChatSessionGCCheck)
		r.Get("/automation-runs/{runId}/gc-check", h.GetAutomationRunGCCheck)
		r.Get("/tasks/{taskId}/gc-check", h.GetTaskGCCheck)

		r.Post("/runtimes/{runtimeId}/recover-orphans", h.RecoverOrphanedTasks)
		r.Post("/runtimes/{runtimeId}/recover-orphans/preserve-results", h.RecoverOrphanedTasks)
		r.Post("/tasks/{taskId}/session", h.PinTaskSession)
	})

	// Plugin Action API. Surfaces reach it on the signed-in user's session;
	// plugin servers reach it with mpi_/mpc_ bearer tokens. Both share
	// /api/v1/plugin. The handler still resolves installation identity and
	// scopes before touching a resource.
	r.Group(func(r chi.Router) {
		r.Use(middleware.PluginAuth(queries, patCache, cloudPATVerifier))
		r.Use(middleware.PluginRateLimit(rdb, settings.PluginRate, time.Minute))
		r.Route(publicapiv1.BasePath, func(r chi.Router) {
			registerPluginActionRoutes(r, h)
			// ui / manual only. `event` is dispatched by the host off the event
			// bus; `agent` arrives over MCP rather than this HTTP endpoint.
			r.Post("/hooks/{key}", h.InvokePluginHook)
			r.NotFound(publicapiv1.NotFound)
			r.MethodNotAllowed(publicapiv1.MethodNotAllowed)
		})
	})

	r.Group(func(r chi.Router) {
		r.Use(middleware.Auth(queries, patCache, cloudPATVerifier))
		r.Use(middleware.RefreshCloudFrontCookies(cfSigner))

		// Plugin Action API. Called by the HOST PAGE on the signed-in user's
		// session after a surface asks for something over the postMessage
		// bridge — the iframe holds no credential and never reaches these
		// directly. Which installation is speaking arrives in a header the
		// host sets; the workspace is derived from that installation rather
		// than trusted from the client, and membership is then checked
		// against it. Sits in the user-scoped group for that reason: there is
		// no workspace in the path to gate on.
		// --- User-scoped routes (no workspace context required) ---
		r.Get("/api/me", h.GetMe)
		r.Patch("/api/me", h.UpdateMe)
		r.Patch("/api/me/onboarding", h.PatchOnboarding)
		r.Post("/api/me/onboarding/complete", h.CompleteOnboarding)
		r.Post("/api/me/onboarding/cloud-waitlist", h.JoinCloudWaitlist)
		r.With(handler.RequireHumanActor).Post("/api/desktop-handoff/complete", h.CompleteDesktopAuthHandoff)
		// DEPRECATED — shim routes for desktop < v3 during the rollout
		// window. v3 frontend creates the Helper agent + starter issue
		// via generic CreateAgent / CreateIssue and only calls /complete
		// here. Remove once X-Client-Version telemetry confirms zero
		// pre-v3 desktops are still calling these. Handlers live in
		// server/internal/handler/onboarding_shim.go.
		r.Post("/api/me/onboarding/runtime-bootstrap", h.BootstrapOnboardingRuntime)
		r.Post("/api/me/onboarding/no-runtime-bootstrap", h.BootstrapOnboardingNoRuntime)
		r.Post("/api/cli-token", h.IssueCliToken)
		r.Route("/api/auth/device", func(r chi.Router) {
			r.Use(handler.RequireHumanActor)
			r.Post("/inspect", h.InspectDeviceAuthorization)
			r.Post("/decision", h.DecideDeviceAuthorization)
		})
		r.Post("/api/upload-file", h.UploadFile)
		r.Post("/api/feedback", h.CreateFeedback)
		r.With(handler.RequireHumanActor).Post("/api/client-usage", h.UpsertClientUsage)

		// Guest sessions are account-scoped, not workspace-scoped. Keep the
		// human gate here as a second boundary after Auth: ovg_ guest bearers
		// may manage their own lifecycle, formal human users may claim, and
		// machine credentials must not create/read/claim/revoke sessions.
		r.Route("/api/guest-sessions", func(r chi.Router) {
			r.Use(handler.RequireHumanActor)
			r.Post("/", h.CreateGuestSession)
			r.Route("/{id}", func(r chi.Router) {
				r.Get("/", h.GetGuestSession)
				r.Post("/claim", h.ClaimGuestSession)
				r.Post("/revoke", h.RevokeGuestSession)
			})
		})

		// Note (MUL-4309): the generic OpenAI-compatible passthrough endpoints
		// (POST /api/llm/v1/chat/completions[/stream]) were intentionally
		// removed. Exposing a general LLM proxy backed by the deployment's own
		// key let any logged-in user run arbitrary completions on our dime.
		// LLM access is now server-internal only (see pkg/llm); anything the
		// web/client needs must go through a purpose-built business endpoint
		// that fixes the prompt/model server-side (e.g. chat title generation).

		// Attachment download — user-scoped (auth-only), NOT
		// workspace-scoped. The handler self-resolves the workspace
		// from the attachment row and enforces membership inside, so
		// this route is callable as a native browser <img>/<video>
		// src that cannot attach X-Workspace-Slug / X-Workspace-ID
		// headers. Persisting `/api/attachments/<id>/download` into
		// comment markdown depends on this — see MUL-3130. The
		// metadata / delete endpoints below stay workspace-scoped
		// because they are JSON-API consumers that always have
		// workspace context.
		r.Get("/api/attachments/{id}/download", h.DownloadAttachment)

		r.Route("/api/workspaces", func(r chi.Router) {
			r.Get("/", h.ListWorkspaces)
			r.Post("/", h.CreateWorkspace)
			r.Route("/{id}", func(r chi.Router) {
				// Member-level access
				r.Group(func(r chi.Router) {
					r.Use(middleware.RequireWorkspaceMemberFromURL(queries, "id"))
					r.Get("/", h.GetWorkspace)
					r.Get("/members", h.ListMembersWithUser)
					r.Post("/leave", h.LeaveWorkspace)
					r.Get("/invitations", h.ListWorkspaceInvitations)
					// Listing GitHub installations is member-visible so the
					// integrations tab no longer renders blank for non-admins;
					// the handler strips the management handle and adds a
					// can_manage hint so the UI can gate connect/disconnect.
					r.Get("/github/installations", h.ListGitHubInstallations)
					// VCS connections (Forgejo / Gitea / GitLab) — member-visible
					// for the same reason as GitHub installations; connect /
					// disconnect are admin-gated in the group below.
					r.Get("/vcs/connections", h.ListVCSConnections)
					// Custom runtime profiles — listing/reading is member-visible
					// (the Runtime page renders for everyone; create/edit/delete
					// are admin-gated below).
					r.Get("/runtime-profiles", h.ListRuntimeProfiles)
					r.Get("/runtime-profiles/{profileId}", h.GetRuntimeProfile)
					// The workspace MCP library — member-visible so an agent
					// owner can see what is available to add to their agent.
					// The payload is names and transports only; the stored
					// entries are write-only.
					r.Get("/mcp-servers", h.ListWorkspaceMcpServers)
					// Installed Plugins are member-visible so a member can
					// see what is mounted in their workspace and which scopes
					// it holds; install / configure / remove stay admin-only.
					r.Get("/plugins", h.ListPlugins)
					// One short-lived hosted surface launch. Member-visible
					// because opening an issue is what asks for it; executable
					// bytes stay off the authenticated app/API origin.
					r.Get("/plugins/{installationId}/surfaces/{surfaceKey}/launch", h.GetPluginSurfaceLaunch)
				})
				// Admin-level access
				r.Group(func(r chi.Router) {
					r.Use(middleware.RequireWorkspaceRoleFromURL(queries, "id", "owner", "admin"))
					r.Put("/", h.UpdateWorkspace)
					r.Patch("/", h.UpdateWorkspace)
					r.Post("/members", h.CreateInvitation)
					r.Route("/members/{memberId}", func(r chi.Router) {
						r.Patch("/", h.UpdateMember)
						r.Delete("/", h.DeleteMember)
					})
					r.Delete("/invitations/{invitationId}", h.RevokeInvitation)
					r.Post("/invitations/{invitationId}/resend", h.ResendInvitation)
					// Curating the shared MCP library is an admin action.
					// Creating an entry binds it to no agent; an agent owner
					// adds it to their own agent through the agent routes.
					r.Post("/mcp-servers", h.CreateWorkspaceMcpServer)
					r.Put("/mcp-servers/{serverId}", h.UpdateWorkspaceMcpServer)
					r.Delete("/mcp-servers/{serverId}", h.DeleteWorkspaceMcpServer)
					r.Post("/share-links", h.CreateShareLink)
					r.Delete("/share-links/{linkId}", h.RevokeShareLink)
					r.Get("/share-links", h.ListShareLinks)
					// Custom runtime profile mutations (admin-only).
					r.Post("/runtime-profiles", h.CreateRuntimeProfile)
					r.Patch("/runtime-profiles/{profileId}", h.UpdateRuntimeProfile)
					r.Put("/runtime-profiles/{profileId}", h.UpdateRuntimeProfile)
					r.Delete("/runtime-profiles/{profileId}", h.DeleteRuntimeProfile)
					// Publishing. The author uploads an artifact bundle and we
					// store it; a version is immutable once published, so
					// there is no update route here by design.
					r.Get("/plugins/packages", h.ListPluginPackages)
					r.Post("/plugins/packages", h.PublishPluginPackage)
					r.Post("/plugins/packages/local", h.PublishLocalPluginPackage)
					r.Delete("/plugins/packages/{packageId}", h.DeletePluginPackage)
					// Installing a Plugin is two steps on purpose: preview
					// reads the published version's manifest and returns the
					// scope list without writing anything, so the consent
					// screen has something to show before an installation
					// exists. Both steps name the same version id, which is
					// what makes the approved manifest and the running code
					// the same artifact.
					r.Post("/plugins/preview", h.PreviewPlugin)
					r.Post("/plugins", h.InstallPlugin)
					r.Get("/plugins/{installationId}/invocations", h.ListPluginInvocations)
					r.Post("/plugins/{installationId}/token", h.RotatePluginToken)
					r.Delete("/plugins/{installationId}/token", h.RevokePluginToken)
					// mcp-transport approval. Discovery adopts nothing; the PUT
					// is the grant, and it pins the tools by schema digest.
					r.Get("/plugins/{installationId}/mcp/{hookKey}/tools", h.ListPluginMCPTools)
					r.Put("/plugins/{installationId}/mcp/{hookKey}/tools", h.ApprovePluginMCPTools)
					r.Put("/plugins/{installationId}/config", h.ConfigurePlugin)
					r.Post("/plugins/{installationId}/enable", h.EnablePlugin)
					r.Post("/plugins/{installationId}/disable", h.DisablePlugin)
					r.Delete("/plugins/{installationId}", h.UninstallPlugin)
				})
				// Owner-only access
				r.With(middleware.RequireWorkspaceRoleFromURL(queries, "id", "owner")).Delete("/", h.DeleteWorkspace)

				// GitHub integration — connect / disconnect remain admin-only;
				// the read-only list endpoint lives in the member-level group
				// above so non-admins can see the workspace's connection state.
				r.Group(func(r chi.Router) {
					r.Use(middleware.RequireWorkspaceRoleFromURL(queries, "id", "owner", "admin"))
					r.Get("/github/connect", h.GitHubConnect)
					r.Get("/github/installations/{installationId}/repositories", h.ListGitHubInstallationRepositories)
					r.Delete("/github/installations/{installationId}", h.DeleteGitHubInstallation)
					// VCS connect / disconnect / webhook regeneration (admin-only).
					r.Post("/vcs/connections", h.ConnectVCS)
					r.Post("/vcs/connections/{connectionId}/rotate-webhook", h.RotateVCSConnectionWebhook)
					r.Delete("/vcs/connections/{connectionId}", h.DeleteVCSConnection)
				})

				// Lark integration. Every endpoint here only requires
				// workspace membership at the router; the real authorization
				// is per-agent and enforced inside each handler via
				// canManageAgent (agent owner OR workspace owner/admin), so an
				// agent's owner can bind/manage their own agent's Bot without
				// being a workspace admin (MUL-4213). The router can't make
				// that call itself: begin identifies the agent by an
				// `agent_id` query param and revoke by an installation id,
				// neither of which is a URL param the role middleware sees.
				//   - Listing stays member-visible (same rationale as GitHub:
				//     the Integrations tab must render for non-admins so they
				//     see "wired up by whom").
				//   - Begin / status / revoke each load the target agent and
				//     run canManageAgent (status gates on the session
				//     initiator or an admin) before doing anything.
				r.Group(func(r chi.Router) {
					r.Use(middleware.RequireWorkspaceMemberFromURL(queries, "id"))
					r.Get("/messaging/usage", h.GetMessagingQuotaUsage)
					r.Get("/lark/installations", h.ListLarkInstallations)
					r.Delete("/lark/installations/{installationId}", h.RevokeLarkInstallation)
					// Device-flow scan-to-install. Begin opens a new
					// registration session against Lark and returns
					// the QR-code URL; the frontend dialog then polls
					// /install/{sessionId}/status until success or
					// terminal failure.
					r.Post("/lark/install/begin", h.BeginLarkInstall)
					r.Get("/lark/install/{sessionId}/status", h.GetLarkInstallStatus)
				})

				// Slack integration (MUL-3666). Listing is member-visible;
				// installs (BYO paste and managed-OAuth begin) + revoke are
				// admin-only: all three connect or disconnect a workspace-level
				// bot. The OAuth callback itself is a public route (it is hit
				// by Slack's browser redirect with no workspace in the path)
				// and is registered outside this workspace group.
				r.Group(func(r chi.Router) {
					r.Use(middleware.RequireWorkspaceMemberFromURL(queries, "id"))
					r.Get("/linear", h.GetLinearConnection)
					r.Get("/linear/catalog", h.GetLinearCatalog)
					r.Get("/linear/bindings", h.ListLinearBindings)
					r.Get("/linear/members", h.ListLinearMemberBindings)
					r.Get("/linear/conflicts", h.ListLinearSyncConflicts)
					r.Get("/slack/installations", h.ListSlackInstallations)
					r.Get("/slack/automation-catalog", h.GetSlackAutomationCatalog)
					r.Get("/wecom/installations", h.ListWecomInstallations)
				})
				r.Group(func(r chi.Router) {
					r.Use(middleware.RequireWorkspaceRoleFromURL(queries, "id", "owner", "admin"))
					r.Post("/linear/connect", h.ConnectLinear)
					r.Delete("/linear", h.DisconnectLinear)
					r.Post("/linear/dry-run", h.DryRunLinearBinding)
					r.Post("/linear/bindings", h.CreateLinearBinding)
					r.Patch("/linear/bindings/{bindingId}", h.UpdateLinearBinding)
					r.Delete("/linear/bindings/{bindingId}", h.DeleteLinearBinding)
					r.Post("/linear/bindings/{bindingId}/import", h.QueueLinearInitialImport)
					r.Put("/linear/members", h.UpsertLinearMemberBinding)
					r.Delete("/linear/members/{userId}", h.DeleteLinearMemberBinding)
					r.Patch("/linear/conflicts/{conflictId}", h.ResolveLinearSyncConflict)
					r.Delete("/slack/installations/{installationId}", h.RevokeSlackInstallation)
					r.Post("/slack/install/byo", h.RegisterSlackBYO)
					r.Post("/slack/install/managed", h.BeginManagedSlackInstall)
					r.Delete("/wecom/installations/{installationId}", h.RevokeWecomInstallation)
					r.Post("/wecom/install/byo", h.RegisterWecomBYO)
				})

				r.Group(func(r chi.Router) {
					r.Use(middleware.RequireWorkspaceMemberFromURL(queries, "id"))
					r.Get("/dingtalk/installations", h.ListDingTalkInstallations)
					r.Get("/dingtalk/groups", h.ListDingTalkGroups)
					r.Get("/dingtalk/group-routes", h.ListDingTalkGroupRoutes)
					r.Patch("/dingtalk/group-routes/{routeId}", h.UpdateDingTalkGroupRoute)
					r.Delete("/dingtalk/installations/{installationId}/groups/{conversationId}", h.ForgetDingTalkGroup)
					r.Delete("/dingtalk/installations/{installationId}", h.RevokeDingTalkInstallation)
					r.Post("/dingtalk/install/byo", h.RegisterDingTalkBYO)
				})

				// Telegram integration. Same admin/member split as Slack:
				// listing is member-visible; install + revoke are admin-only.
				r.Group(func(r chi.Router) {
					r.Use(middleware.RequireWorkspaceMemberFromURL(queries, "id"))
					r.Get("/telegram/installations", h.ListTelegramInstallations)
				})
				r.Group(func(r chi.Router) {
					r.Use(middleware.RequireWorkspaceRoleFromURL(queries, "id", "owner", "admin"))
					r.Delete("/telegram/installations/{installationId}", h.RevokeTelegramInstallation)
					r.Post("/telegram/install", h.RegisterTelegramBot)
				})
				// Native Weixin QR installation. The router only requires
				// membership; the handler checks agent-owner or workspace-admin
				// authority because agent_id is a query parameter and status is
				// bound to the initiating actor.
				r.Group(func(r chi.Router) {
					r.Use(middleware.RequireWorkspaceMemberFromURL(queries, "id"))
					r.Get("/weixin/installations", h.ListWeixinInstallations)
					r.Post("/weixin/install/begin", h.BeginWeixinInstall)
					r.Get("/weixin/install/{sessionId}/status", h.GetWeixinInstallStatus)
					r.Delete("/weixin/installations/{installationId}", h.RevokeWeixinInstallation)
				})
			})
		})

		// Lark binding-token redemption. NOT workspace-scoped because
		// the redeemer hits this BEFORE they have any workspace
		// context — the redemption itself is what mints their
		// lark_user_binding row. Identity comes from the session;
		// the token only proves "this open_id requested binding," and
		// is combined with the logged-in user to create the mapping.
		r.Post("/api/lark/binding/redeem", h.RedeemLarkBindingToken)
		// Slack binding-token redemption. Same rationale as Lark: NOT
		// workspace-scoped because the redeemer hits this before they have any
		// workspace context — the redemption itself mints their binding row. The
		// logged-in user (from the session) is bound to the Slack id the token
		// carries.
		r.Post("/api/slack/binding/redeem", h.RedeemSlackBindingToken)
		// DingTalk binding redemption is user-scoped for the same reason as
		// Slack: the token is redeemed before workspace context is selected.
		r.Post("/api/dingtalk/binding/redeem", h.RedeemDingTalkBindingToken)
		// WeCom smart-bot binding-token redemption. Same rationale as
		// Lark/Slack: the session is the source of truth for the redeemer's
		// Orvilo identity; the token only carries the WeCom userid to bind.
		r.Post("/api/wecom/binding/redeem", h.RedeemWecomBindingToken)
		// Telegram binding-token redemption. Same rationale: not
		// workspace-scoped, identity from the session, token proves only
		// "this Telegram user id requested binding".
		r.Post("/api/telegram/binding/redeem", h.RedeemTelegramBindingToken)
		// Weixin binding redemption is user-scoped: the session identity is
		// the Orvilo user, while the bearer token carries only the iLink id.
		r.Post("/api/weixin/binding/redeem", h.RedeemWeixinBindingToken)

		// Composio integration (MUL-3720). User-scoped (no workspace context):
		// a connection belongs to a user. These four require a logged-in
		// session; the OAuth callback is the outlier and lives outside the Auth
		// group (registered above with the other public OAuth/webhook routes —
		// see MUL-3843). All return 503 when COMPOSIO_API_KEY is unset.
		r.Route("/api/integrations/composio", func(r chi.Router) {
			r.Post("/connect/init", h.ComposioConnectInit)
			r.Get("/toolkits", h.ListComposioToolkits)
			r.Get("/connections", h.ListComposioConnections)
			r.Delete("/connections/{id}", h.DeleteComposioConnection)
		})

		// User-scoped invitation routes (no workspace context required)
		r.Get("/api/invitations", h.ListMyInvitations)
		r.Get("/api/invitations/{id}", h.GetMyInvitation)
		r.Post("/api/invitations/{id}/accept", h.AcceptInvitation)
		r.Post("/api/invitations/{id}/decline", h.DeclineInvitation)
		r.Post("/api/share-links/join", h.JoinByShareLink)

		r.Route("/api/tokens", func(r chi.Router) {
			r.Get("/", h.ListPersonalAccessTokens)
			r.Post("/", h.CreatePersonalAccessToken)
			r.Post("/current/renew", h.RenewCurrentPersonalAccessToken)
			r.Delete("/{id}", h.RevokePersonalAccessToken)
		})

		// Cloud Billing proxy. Same upstream service / port as
		// cloud-runtime — orvilo-cloud's Fleet and Billing share
		// :8080 and the same chi router. All routes here forward
		// to /api/v1/billing/* with X-User-ID stamped from the
		// authenticated context.
		//
		// User-scoped (account-level), NOT workspace-scoped — sits
		// outside the RequireWorkspaceMember group so a user can
		// inspect their balance, top up, and open the Billing Portal
		// without an active workspace selected. The upstream owner
		// model is single-user; X-Workspace-ID would be ignored even
		// if we sent it. The Stripe webhook is the public outlier
		// and lives outside the entire Auth group (see above).
		//
		// IMPORTANT — task-token actors are blocked here. The Auth
		// middleware happily turns an mat_ task token into a normal
		// X-User-ID stamp (so agents can comment, claim issues, etc.
		// as their owner), but billing is account-level and a running
		// agent reading its owner's balance / opening a checkout
		// session is the kind of lateral-movement we're explicitly
		// trying to prevent. handler.RequireHumanActor checks the
		// authoritative server-set X-Actor-Source header and 403s
		// any task-token request. See actor_guards.go for the full
		// rationale.
		r.Route("/api/cloud-billing", func(r chi.Router) {
			r.Use(handler.RequireHumanActor)

			r.Get("/balance", h.GetCloudBillingBalance)
			r.Get("/transactions", h.ListCloudBillingTransactions)
			r.Get("/batches", h.ListCloudBillingBatches)
			r.Get("/topups", h.ListCloudBillingTopups)
			r.Get("/price-tiers", h.ListCloudBillingPriceTiers)
			r.Post("/checkout-sessions", h.CreateCloudBillingCheckoutSession)
			r.Get("/checkout-sessions/{sessionId}", h.GetCloudBillingCheckoutSession)
			r.Post("/portal-sessions", h.CreateCloudBillingPortalSession)
		})

		// Workspace subscriptions use the same cloud transport and Stripe
		// webhook as the existing owner-credit billing surface, but every request
		// is workspace-scoped. Summary and prices are member-readable. Local role
		// checks cheaply reject unauthorized writes; Cloud remains the final
		// authority and validates every mutation before external writes. Handlers
		// also enforce billing_workspace_subscriptions so a route refactor cannot
		// accidentally bypass the rollout flag.
		r.Route("/api/cloud-subscriptions", func(r chi.Router) {
			r.Use(handler.RequireHumanActor)
			r.Group(func(r chi.Router) {
				r.Use(middleware.RequireWorkspaceMember(queries))
				r.Get("/summary", h.GetCloudWorkspaceSubscriptionSummary)
				r.Get("/prices", h.GetCloudWorkspaceSubscriptionPrices)
			})
			r.Group(func(r chi.Router) {
				r.Use(middleware.RequireWorkspaceRole(queries, "owner", "admin"))
				r.Post("/checkout-sessions", h.CreateCloudWorkspaceSubscriptionCheckout)
				r.Post("/seats/purchase-preview", h.PreviewCloudWorkspaceSubscriptionSeatPurchase)
				r.Post("/seats/purchases", h.PurchaseCloudWorkspaceSubscriptionSeats)
				r.Post("/seats/reconcile", h.ReconcileCloudWorkspaceSubscriptionSeats)
				r.Post("/portal-sessions", h.CreateCloudWorkspaceSubscriptionPortal)
			})
		})

		// Rust-compatible authorization control plane. These routes are
		// workspace-header scoped (like the Rust WorkspaceContext), rather than
		// nested under /api/workspaces/{id}; the old workspace-nested provider
		// paths below remain compatibility aliases for clients shipped during the
		// Go migration.
		r.Route("/api/authorization", func(r chi.Router) {
			r.Use(middleware.RequireWorkspaceMember(queries))
			r.With(handler.RequireHumanActor).Get("/provider-grants", h.ListProviderAuthorizationGrants)
			r.With(handler.RequireHumanActor).Post("/provider-grants", h.CreateProviderAuthorizationGrant)
			r.With(handler.RequireHumanActor).Delete("/provider-grants/{grantId}", h.RevokeProviderAuthorizationGrant)
			r.With(handler.RequireHumanActor).Get("/decisions/{decisionId}", h.ExplainProviderAuthorizationDecision)
			// Lease validation is intentionally task-token capable; the handler
			// requires the server-stamped X-Actor-Source/task headers.
			r.Post("/provider-leases/validate", h.ValidateProviderLease)
		})

		// --- Workspace-scoped routes (all require workspace membership) ---
		r.Group(func(r chi.Router) {
			r.Use(middleware.RequireWorkspaceMember(queries))

			// Executor frequency
			r.Get("/api/executor-frequency", h.GetExecutorFrequency)

			// Issues
			r.Route("/api/issues", func(r chi.Router) {
				r.Get("/limit-usage", h.GetIssueLimitUsage)
				r.Post("/table/groups", h.ListIssueTableGroups)
				r.Post("/table/rows", h.ListIssueTableRows)
				r.Post("/table/facets", h.ListIssueTableFacets)
				r.Get("/search", h.SearchIssues)
				r.Get("/child-progress", h.ChildIssueProgress)
				r.Get("/children", h.ListChildrenByParents)
				r.Get("/grouped", h.ListGroupedIssues)
				r.Get("/", h.ListIssues)
				// POST twin of GET /api/issues for oversized filter sets
				// (agents-working ids facet) — see QueryIssues.
				r.Post("/query", h.QueryIssues)
				r.Post("/", h.CreateIssue)
				r.Post("/quick-create", h.QuickCreateIssue)
				r.Post("/preview-trigger", h.PreviewIssueTrigger)
				r.Post("/batch-update", h.BatchUpdateIssues)
				r.Post("/batch-delete", h.BatchDeleteIssues)
				r.Route("/{id}", func(r chi.Router) {
					r.Get("/", h.GetIssue)
					r.Put("/", h.UpdateIssue)
					r.Post("/move", h.MoveIssue)
					r.Delete("/", h.DeleteIssue)
					r.Post("/comments/trigger-preview", h.PreviewCommentTriggers)
					r.Post("/comments", h.CreateComment)
					r.Get("/comments", h.ListComments)
					r.Get("/timeline", h.ListTimeline)
					r.Get("/subscribers", h.ListIssueSubscribers)
					r.Post("/subscribe", h.SubscribeToIssue)
					r.Post("/unsubscribe", h.UnsubscribeFromIssue)
					r.Post("/unsubscribe/subtree", h.UnsubscribeFromIssueSubtree)
					r.Get("/active-task", h.GetActiveTaskForIssue)
					r.Post("/tasks/{taskId}/cancel", h.CancelTask)
					r.Post("/rerun", h.RerunIssue)
					r.Post("/quick-actions/{quickActionId}/run", h.RunQuickAction)
					r.Post("/quick-actions/{quickActionId}/render", h.RenderQuickAction)
					r.Get("/task-runs", h.ListTasksByIssue)
					r.Get("/usage", h.GetIssueUsage)
					r.Post("/reactions", h.AddIssueReaction)
					r.Delete("/reactions", h.RemoveIssueReaction)
					r.Get("/attachments", h.ListAttachments)
					r.Get("/children", h.ListChildIssues)
					r.Get("/labels", h.ListLabelsForIssue)
					r.Post("/labels", h.AttachLabel)
					r.Delete("/labels/{labelId}", h.DetachLabel)
					r.Get("/metadata", h.ListIssueMetadata)
					r.Put("/metadata/{key}", h.SetIssueMetadataKey)
					r.Delete("/metadata/{key}", h.DeleteIssueMetadataKey)
					r.Put("/properties/{propertyId}", h.SetIssueProperty)
					r.Delete("/properties/{propertyId}", h.DeleteIssueProperty)
					r.Get("/work-products", h.ListWorkProductsForIssue)
					r.Post("/work-products", h.AttachExistingWorkProduct)
					r.Delete("/work-products/{workProductId}", h.DetachWorkProduct)
					r.Get("/pull-requests", h.ListIssuePullRequests)
					r.Post("/pull-requests", h.AttachIssuePullRequest)
					r.Get("/dependency-graph", h.GetIssueDependencyGraph)
					r.Post("/dependency-graph/apply", h.ApplyIssueDependencyGraph)
				})
			})
			r.Route("/api/dependency-graphs", func(r chi.Router) {
				r.Get("/", h.ListDependencyGraphs)
				r.Route("/{id}", func(r chi.Router) {
					r.Get("/", h.GetDependencyGraphByID)
					r.Post("/retire", h.RetireDependencyGraph)
				})
			})

			// Task messages (user-facing, not daemon auth)
			r.Get("/api/tasks/{taskId}/messages", h.ListTaskMessagesByUser)
			r.With(handler.RequireHumanActor).Post("/api/tasks/{taskId}/retry-source-context", h.RetrySourceContextQuickCreate)

			// Issue quick actions (definitions; running one lives under
			// /api/issues/{id}/quick-actions/{quickActionId}/run)
			r.Route("/api/quick-actions", func(r chi.Router) {
				r.Get("/", h.ListQuickActions)
				r.Post("/", h.CreateQuickAction)
				r.Route("/{id}", func(r chi.Router) {
					r.Patch("/", h.UpdateQuickAction)
					r.Delete("/", h.DeleteQuickAction)
				})
			})

			// Custom issue properties (definitions; values live under /api/issues/{id}/properties)
			r.Route("/api/properties", func(r chi.Router) {
				r.Get("/", h.ListProperties)
				r.Post("/", h.CreateProperty)
				r.Route("/{id}", func(r chi.Router) {
					r.Get("/", h.GetProperty)
					r.Patch("/", h.UpdateProperty)
				})
			})

			// Labels
			r.Route("/api/labels", func(r chi.Router) {
				r.Get("/", h.ListLabels)
				r.Post("/", h.CreateLabel)
				r.Route("/{id}", func(r chi.Router) {
					r.Get("/", h.GetLabel)
					r.Put("/", h.UpdateLabel)
					r.Delete("/", h.DeleteLabel)
				})
			})

			// Issue status catalog (MUL-6243). Reads are open to any member —
			// every client needs the catalog to render a status. Writes are
			// gated to workspace owner/admin inside the handlers.
			r.Route("/api/issue-statuses", func(r chi.Router) {
				r.Get("/", h.ListIssueStatuses)
				r.Post("/", h.CreateIssueStatus)
				r.Patch("/reorder", h.ReorderIssueStatuses)
				r.Route("/{id}", func(r chi.Router) {
					r.Patch("/", h.UpdateIssueStatus)
					r.Delete("/", h.ArchiveIssueStatus)
				})
			})

			// Workspace issue category policies. Reads are open to any member;
			// updates are gated to workspace owner/admin inside the handler.
			r.Route("/api/issue-category-policies", func(r chi.Router) {
				r.Get("/", h.ListIssueCategoryPolicies)
				r.Put("/{category}", h.UpdateIssueCategoryPolicy)
			})

			// Projects
			r.Route("/api/projects", func(r chi.Router) {
				r.Get("/search", h.SearchProjects)
				r.Get("/", h.ListProjects)
				r.Post("/", h.CreateProject)
				r.Route("/{id}", func(r chi.Router) {
					r.Get("/", h.GetProject)
					r.Put("/", h.UpdateProject)
					r.Delete("/", h.DeleteProject)
					r.Get("/resources", h.ListProjectResources)
					r.Post("/resources", h.CreateProjectResource)
					r.Put("/resources/{resourceId}", h.UpdateProjectResource)
					r.Delete("/resources/{resourceId}", h.DeleteProjectResource)
				})
			})

			// Teams
			r.Route("/api/teams", func(r chi.Router) {
				r.Get("/", h.ListTeams)
				r.Post("/", h.CreateTeam)
				r.Route("/{id}", func(r chi.Router) {
					r.Get("/", h.GetTeam)
					r.Put("/", h.UpdateTeam)
					r.Delete("/", h.DeleteTeam)
					r.Get("/members", h.ListTeamMembers)
					r.Get("/members/status", h.ListTeamMemberStatus)
					r.Post("/members", h.AddTeamMember)
					r.Delete("/members", h.RemoveTeamMember)
					r.Patch("/members/role", h.UpdateTeamMemberRole)
				})
			})

			// Team leader evaluation (writes to activity_log)
			r.Post("/api/issues/{id}/team-evaluated", h.RecordTeamLeaderEvaluation)

			// Automations
			r.Route("/api/automations", func(r chi.Router) {
				r.Get("/", h.ListAutomations)
				r.Post("/", h.CreateAutomation)
				r.Get("/cron-preview", h.CronPreview)
				r.Get("/usage", h.GetAutomationQuotaUsage)
				r.Get("/runs", h.ListWorkspaceAutomationRuns)
				r.Route("/{id}", func(r chi.Router) {
					r.Get("/", h.GetAutomation)
					r.Patch("/", h.UpdateAutomation)
					r.Delete("/", h.DeleteAutomation)
					r.Post("/trigger", h.TriggerAutomation)
					r.Get("/runs", h.ListAutomationRuns)
					r.Get("/runs/{runId}", h.GetAutomationRun)
					r.Get("/github-catalog", h.GetAutomationGitHubCatalog)
					r.Get("/deliveries", h.ListAutomationDeliveries)
					r.Get("/memories", h.ListAutomationMemories)
					r.Get("/memories/{name}", h.GetAutomationMemory)
					r.Put("/memories/{name}", h.PutAutomationMemory)
					r.Delete("/memories/{name}", h.DeleteAutomationMemory)
					r.Get("/deliveries/{deliveryId}", h.GetAutomationDelivery)
					r.Post("/deliveries/{deliveryId}/replay", h.ReplayAutomationDelivery)
					r.Post("/triggers", h.CreateAutomationTrigger)
					r.Route("/triggers/{triggerId}", func(r chi.Router) {
						r.Patch("/", h.UpdateAutomationTrigger)
						r.Delete("/", h.DeleteAutomationTrigger)
						r.Post("/rotate-webhook-token", h.RotateAutomationTriggerWebhookToken)
						r.Put("/signing-secret", h.SetAutomationTriggerSigningSecret)
					})
					r.Post("/collaborators", h.AddAutomationCollaborator)
					r.Delete("/collaborators/{userId}", h.RemoveAutomationCollaborator)
				})
			})

			// Pins
			r.Route("/api/pins", func(r chi.Router) {
				r.Get("/", h.ListPins)
				r.Post("/", h.CreatePin)
				r.Put("/reorder", h.ReorderPins)
				r.Delete("/{itemType}/{itemId}", h.DeletePin)
			})

			// Saved issue views (MUL-4796).
			r.Get("/api/issue-view-preferences", h.GetIssueViewPreference)
			r.Put("/api/issue-view-preferences", h.PutIssueViewPreference)
			r.Route("/api/issue-views", func(r chi.Router) {
				r.Get("/", h.ListIssueViews)
				r.Post("/", h.CreateIssueView)
				r.Route("/{id}", func(r chi.Router) {
					r.Get("/", h.GetIssueViewByID)
					r.Patch("/", h.UpdateIssueView)
					r.Delete("/", h.DeleteIssueView)
				})
			})

			// Attachments
			r.Get("/api/attachments/{id}", h.GetAttachmentByID)
			// /api/attachments/{id}/download is registered in the
			// outer Auth-only group above so it can be loaded as a
			// native <img>/<video> src without workspace headers
			// (MUL-3130). The handler self-resolves the workspace
			// from the attachment row.
			r.Get("/api/attachments/{id}/content", h.GetAttachmentContent)
			r.Delete("/api/attachments/{id}", h.DeleteAttachment)

			// Comments
			r.Route("/api/comments/{commentId}", func(r chi.Router) {
				r.With(handler.RequireHumanActor).Get("/sub-issue-preview", h.PreviewCommentSubIssue)
				r.With(handler.RequireHumanActor).Post("/sub-issues", h.CreateCommentSubIssue)
				r.Put("/", h.UpdateComment)
				r.Delete("/", h.DeleteComment)
				r.Post("/resolve", h.ResolveComment)
				r.Delete("/resolve", h.UnresolveComment)
				r.Post("/reactions", h.AddReaction)
				r.Delete("/reactions", h.RemoveReaction)
			})

			// Agents
			r.Route("/api/agents", func(r chi.Router) {
				r.Get("/", h.ListAgents)
				r.Post("/", h.CreateAgent)
				// The workspace's built-in Chief of Staff. Server-owned: the
				// caller supplies only a runtime and a language, so a client
				// cannot mint an agent carrying `system_key` and thereby claim
				// the system instruction layer. Idempotent per workspace.
				r.Post("/patrick", h.CreatePatrickAgent)
				r.Route("/{id}", func(r chi.Router) {
					r.Get("/", h.GetAgent)
					r.Put("/", h.UpdateAgent)
					r.Post("/archive", h.ArchiveAgent)
					r.Post("/restore", h.RestoreAgent)
					r.Post("/cancel-tasks", h.CancelAgentTasks)
					r.Get("/tasks", h.ListAgentTasks)
					r.Get("/dingtalk/groups", h.ListDingTalkGroupsForAgent)
					r.Get("/skills", h.ListAgentSkills)
					r.Put("/skills", h.SetAgentSkills)
					r.Post("/skills/add", h.AddAgentSkills)
					r.Get("/labels", h.ListLabelsForAgent)
					r.Post("/labels", h.AttachLabelToAgent)
					r.Delete("/labels/{labelId}", h.DetachLabelFromAgent)
					r.Put("/skills/{skillId}/enabled", h.SetAgentSkillEnabled)
					r.Put("/runtime-skills/enabled", h.SetAgentRuntimeSkillEnabled)
					r.Delete("/skills/{skillId}", h.RemoveAgentSkill)
					// Workspace MCP servers assigned to this agent. Mirrors
					// the skills routes above: a library entry does nothing
					// until it is added here, and the binding carries its own
					// enabled toggle.
					r.Get("/mcp-servers", h.ListAgentMcpServers)
					r.Post("/mcp-servers", h.AddAgentMcpServer)
					r.Put("/mcp-servers/{serverId}/enabled", h.SetAgentMcpServerEnabled)
					r.Delete("/mcp-servers/{serverId}", h.RemoveAgentMcpServer)
					// Dedicated env-management endpoint. Admits the agent
					// owner or a workspace owner/admin; agent actors are
					// denied. Every reveal / write is audited to
					// activity_log. See MUL-2600, MUL-5438 and
					// internal/handler/agent_env.go.
					r.Get("/env", h.GetAgentEnv)
					r.Put("/env", h.UpdateAgentEnv)
				})
			})

			r.Route("/api/agent-builder/sessions", func(r chi.Router) {
				// The creation studio's unfinished drafts. Builder sessions are
				// invisible to every chat list (their carrier is kind='system'),
				// so this is the only route back to one.
				r.Get("/", h.ListAgentBuilderSessions)
				r.Post("/", h.CreateAgentBuilderSession)
				r.Patch("/{sessionId}/runtime", h.SwitchAgentBuilderRuntime)
				// Autosaved configuration, including edits the user has typed
				// but not sent. Read back through the list above.
				r.Put("/{sessionId}/draft", h.SaveAgentBuilderDraft)
			})

			// Skills
			r.Route("/api/skills", func(r chi.Router) {
				r.Get("/", h.ListSkills)
				r.Post("/", h.CreateSkill)
				r.Get("/search", h.SearchSkills)
				r.Post("/import", h.ImportSkill)
				r.Route("/{id}", func(r chi.Router) {
					r.Get("/", h.GetSkill)
					r.Put("/", h.UpdateSkill)
					r.Delete("/", h.DeleteSkill)
					r.Post("/refresh", h.RefreshSkill)
					r.Get("/labels", h.ListLabelsForSkill)
					r.Post("/labels", h.AttachLabelToSkill)
					r.Delete("/labels/{labelId}", h.DetachLabelFromSkill)
					r.Get("/files", h.ListSkillFiles)
					r.Put("/files", h.UpsertSkillFile)
					r.Delete("/files/{fileId}", h.DeleteSkillFile)
				})
			})

			// Dashboard — workspace-wide token + run-time rollups for the
			// "/{slug}/dashboard" page. Optional ?project_id filter scopes
			// the rollup to a single project.
			r.Route("/api/dashboard", func(r chi.Router) {
				r.Get("/usage/daily", h.GetDashboardUsageDaily)
				r.Get("/usage/by-agent", h.GetDashboardUsageByAgent)
				r.Get("/agent-runtime", h.GetDashboardAgentRunTime)
				r.Get("/runtime/daily", h.GetDashboardRunTimeDaily)
				r.Get("/failures/daily", h.GetDashboardFailuresDaily)
				r.Get("/failures/by-agent", h.GetDashboardFailuresByAgent)
			})

			// Runtimes
			r.Route("/api/runtimes", func(r chi.Router) {
				r.Get("/", h.ListAgentRuntimes)
				r.Route("/{runtimeId}", func(r chi.Router) {
					r.Patch("/", h.UpdateAgentRuntime)
					r.Get("/usage", h.GetRuntimeUsage)
					r.Get("/usage/by-agent", h.GetRuntimeUsageByAgent)
					r.Get("/usage/by-hour", h.GetRuntimeUsageByHour)
					r.Get("/activity", h.GetRuntimeTaskActivity)
					r.Post("/update", h.InitiateUpdate)
					r.Get("/update/{updateId}", h.GetUpdate)
					r.Post("/models", h.InitiateListModels)
					r.Get("/models/{requestId}", h.GetModelListRequest)
					r.Post("/local-skills", h.InitiateListLocalSkills)
					r.Get("/local-skills/{requestId}", h.GetLocalSkillListRequest)
					r.Post("/local-skills/import", h.InitiateImportLocalSkill)
					r.Get("/local-skills/import/{requestId}", h.GetLocalSkillImportRequest)
					r.Delete("/", h.DeleteAgentRuntime)
					// Confirmed variant of DELETE: unbind every agent bound to
					// this runtime (they keep their configuration and chats and
					// need a new runtime to run again), cancel their tasks,
					// detach their task history, then delete the runtime — all
					// in one transaction. Used by the DeleteRuntimeDialog when
					// the strict DELETE refused with
					// `runtime_has_active_agents` and the user confirmed.
					r.Post("/unbind-agents-and-delete", h.UnbindAgentsAndDeleteRuntime)
					// Legacy path for installed clients built against the
					// archive-and-delete contract (MUL-5559 renamed the
					// behaviour, not just the route). Same handler.
					r.Post("/archive-agents-and-delete", h.UnbindAgentsAndDeleteRuntime)
				})
			})

			// Cloud Runtime fleet proxy. The remote service URL is configured
			// on SaaS API nodes only; self-hosted deployments return 503.
			r.Route("/api/cloud-runtime", func(r chi.Router) {
				r.Get("/", h.GetCloudRuntimeService)
				r.Get("/healthz", h.GetCloudRuntimeHealth)
				r.Get("/readyz", h.GetCloudRuntimeReady)
				r.Get("/nodes", h.ListCloudRuntimeNodes)
				r.Post("/nodes", h.CreateCloudRuntimeNode)
				r.Delete("/nodes", h.DeleteCloudRuntimeNode)
				r.Post("/nodes/start", h.StartCloudRuntimeNode)
				r.Post("/nodes/stop", h.StopCloudRuntimeNode)
				r.Post("/nodes/reboot", h.RebootCloudRuntimeNode)
				r.Post("/nodes/status", h.GetCloudRuntimeNodeStatus)
				r.Post("/nodes/exec", h.ExecCloudRuntimeNode)
			})

			r.Route("/api/provider-authorizations", func(r chi.Router) {
				r.Use(handler.RequireHumanActor)
				r.Get("/", h.ListProviderAuthorizationGrants)
				r.Post("/", h.CreateProviderAuthorizationGrant)
				r.Get("/decisions/{decisionId}", h.ExplainProviderAuthorizationDecision)
				r.Delete("/leases/{leaseId}", h.RevokeProviderCapabilityLease)
				r.Delete("/{grantId}", h.RevokeProviderAuthorizationGrant)
			})

			// Tasks (user-facing, with ownership check)
			r.Get("/api/tasks/{taskId}/agent-thread", h.GetAgentThread)
			r.Post("/api/tasks/{taskId}/agent-thread/continue", h.ContinueAgentThread)
			r.Post("/api/tasks/{taskId}/agent-thread/queued-tasks/{queuedTaskId}/prioritize", h.PrioritizeAgentThreadTask)
			r.Post("/api/tasks/{taskId}/cancel", h.CancelTaskByUser)

			// Workspace-wide agent task snapshot for presence derivation:
			// every active task + each agent's most recent terminal task.
			r.Get("/api/agent-task-snapshot", h.ListWorkspaceAgentTaskSnapshot)

			// Independent workspace-level list backing the issues-header
			// "agents working" chip and its executor-id Table filter.
			r.Get("/api/working-agents", h.ListWorkspaceWorkingAgents)

			// Workspace-wide daily agent activity (last 30d, anchored on
			// completed_at). Backs the Agents-list sparkline (trailing 7d
			// slice) AND the agent detail "Last 30 days" panel.
			r.Get("/api/agent-activity-30d", h.GetWorkspaceAgentActivity30d)

			// Workspace-wide 30-day run counts per agent for the Agents-list RUNS column.
			r.Get("/api/agent-run-counts", h.GetWorkspaceAgentRunCounts)

			r.Route("/api/chat/sessions", func(r chi.Router) {
				r.Post("/", h.CreateChatSession)
				r.Get("/", h.ListChatSessions)
				r.Route("/{sessionId}", func(r chi.Router) {
					r.Get("/", h.GetChatSession)
					r.Patch("/", h.UpdateChatSession)
					r.Patch("/pin", h.SetChatSessionPinned)
					r.Patch("/archive", h.SetChatSessionArchived)
					r.Delete("/", h.DeleteChatSession)
					r.Post("/messages", h.SendChatMessage)
					r.Post("/onboarding", h.StartPatrickOnboarding)
					// Explicit "refresh" of a turn's quick actions: re-runs the
					// daemon suggestion pass for the latest assistant reply (MUL-5149).
					r.Post("/quick-actions/regenerate", h.RegenerateChatQuickActions)
					r.Get("/messages", h.ListChatMessages)
					r.Get("/messages/page", h.ListChatMessagesPage)
					r.Get("/pending-task", h.GetPendingChatTask)
					r.Delete("/queued-tasks", h.ClearQueuedChatTasks)
					r.Post("/queued-tasks/{taskId}/prioritize", h.PrioritizeQueuedChatTask)
					r.Post("/read", h.MarkChatSessionRead)
					// Deferred-cancellation draft restores (#5219):
					// creator-only fetch + idempotent consume.
					r.Get("/draft-restores", h.ListChatDraftRestores)
					r.Delete("/draft-restores/{restoreId}", h.ConsumeChatDraftRestore)
				})
			})
			r.Get("/api/chat/pending-tasks", h.ListPendingChatTasks)
			r.Get("/api/chat/pending-tasks/has-any", h.HasPendingChatTasks)

			// Quick-agent bar: per-user pinned agents for one-tap new chats.
			r.Get("/api/chat/pinned-agents", h.ListChatPinnedAgents)
			r.Post("/api/chat/pinned-agents", h.PinChatAgent)
			r.Delete("/api/chat/pinned-agents/{agentId}", h.UnpinChatAgent)

			// Agent-facing channel reads (MUL-3871). The caller's task-scoped token
			// resolves to its own chat session; no session/channel id is passed, so
			// an agent can only read its own conversation. `history` is the channel
			// overview (top-level messages + thread metadata); `thread` reads one
			// thread (?id for a specific one, else the thread the session is in).
			r.Get("/api/chat/history", h.GetChatChannelHistory)
			r.Get("/api/chat/thread", h.GetChatThread)

			// Work products & provenance
			r.Route("/api/work-products", func(r chi.Router) {
				r.Get("/unassociated", h.ListUnassociatedWorkProducts)
				r.Get("/", h.ListWorkProducts)
				r.Post("/", h.CreateWorkProduct)
				r.Route("/{id}", func(r chi.Router) {
					r.Get("/", h.GetWorkProduct)
				})
			})
			r.Get("/api/tasks/{taskId}/work-products", h.ListWorkProductsForTask)
			r.Route("/api/tasks/{taskId}/provenance", func(r chi.Router) {
				r.Get("/", h.GetProvenanceByTask)
				r.Post("/", h.UpsertProvenance)
			})
			r.Get("/api/provenance", h.ListProvenanceByWorkspace)

			// Workspace channels
			r.Route("/api/workspace-channels", func(r chi.Router) {
				r.Get("/", h.ListWorkspaceChannels)
				r.Post("/", h.CreateWorkspaceChannel)
				r.Route("/{id}", func(r chi.Router) {
					r.Get("/", h.GetWorkspaceChannel)
					r.Get("/messages", h.ListWorkspaceChannelMessages)
					r.Post("/messages", h.CreateWorkspaceChannelMessage)
				})
			})

			// Inbox
			r.Route("/api/inbox", func(r chi.Router) {
				r.Get("/", h.ListInbox)
				// Archived notifications, for the inbox's "Archived" sub-view.
				// Separate from "/" so the main list keeps its contract and
				// never carries the unbounded archive.
				r.Get("/archived", h.ListArchivedInbox)
				r.Get("/unread-count", h.CountUnreadInbox)
				// Cross-workspace unread summary: account-level, keyed on the
				// user. Backs the workspace-switcher dot for OTHER workspaces.
				r.Get("/unread-summary", h.UnreadInboxSummary)
				r.Post("/mark-all-read", h.MarkAllInboxRead)
				r.Post("/archive-all", h.ArchiveAllInbox)
				r.Post("/archive-all-read", h.ArchiveAllReadInbox)
				r.Post("/archive-completed", h.ArchiveCompletedInbox)
				r.Post("/{id}/read", h.MarkInboxRead)
				r.Post("/{id}/unread", h.MarkInboxUnread)
				r.Post("/{id}/archive", h.ArchiveInboxItem)
				r.Post("/{id}/unarchive", h.UnarchiveInboxItem)
			})

			// Notification preferences
			r.Route("/api/notification-preferences", func(r chi.Router) {
				r.Get("/", h.GetNotificationPreferences)
				r.Patch("/", h.PatchNotificationPreferences)
				r.Put("/", h.UpdateNotificationPreferences)
			})
		})
	})

	return r
}
