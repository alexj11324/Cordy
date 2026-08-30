//! The Stage 2 business-integration glue between Patchbay and the standalone
//! Composio SDK. It owns Patchbay semantics: the signed-state connect
//! handshake, the local user_composio_connection mirror, idempotent
//! disconnect, and the per-user MCP session helper.
//!
//! The SDK is
//! consumed through the [`Sdk`] trait so handler/service tests can inject a
//! fake without hitting Composio.
//!
//! MVP scope (PB-3720): toolkits are discovered dynamically. The
//! toolkit→auth-config mapping is resolved at request time from Composio's
//! /auth_configs endpoint (cached briefly), so a toolkit becomes
//! connectable the moment an auth config is enabled for it in the Composio
//! dashboard — no env var and no redeploy.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Result};
use uuid::Uuid;

pub use crate::sdk as sdk_types;
use crate::sdk::{
    Client, CreateLinkRequest, CreateSessionRequest, Error as SdkError, ListAuthConfigsRequest,
    ListConnectedAccountsRequest, ListToolkitsRequest,
};
use crate::state::{sign_state, verify_state, StateClaims, StateError};

/// Service-level errors surfaced to the handler layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    /// BeginConnect when the requested toolkit has no enabled auth config
    /// in the Composio project, so there is no auth_config_id to start a
    /// connect link with.
    #[error("composio: toolkit not supported")]
    ToolkitNotSupported,
    /// CompleteCallback when Composio reported a non-success status — no
    /// active row is written.
    #[error("composio: connection was not successful")]
    ConnectNotSuccessful,
    /// Disconnect when the connection id does not belong to the user (or
    /// does not exist).
    #[error("composio: connection not found")]
    ConnectionNotFound,
    /// CompleteCallback when the connected_account_id carried on the
    /// callback cannot be confirmed (with Composio) to belong to the
    /// user/auth-config named in the signed state — i.e. a tampered or
    /// unknown account id. No local row is written.
    #[error("composio: connected account verification failed")]
    AccountVerification,
}

/// Bounds how long a connect handshake may sit between BeginConnect and
/// the Composio callback. Five minutes is generous for a hosted OAuth flow
/// while keeping the replay window small.
const DEFAULT_STATE_TTL: Duration = Duration::from_secs(5 * 60);

/// Bounds how long the resolved toolkit→auth-config map is cached before a
/// re-fetch from Composio.
const DEFAULT_AUTH_CACHE_TTL: Duration = Duration::from_secs(5 * 60);

/// Cap the paginated fetch-all loops so a pathological upstream cursor
/// cannot spin forever (at limit=1000/page these cover far more than any
/// real project).
const MAX_AUTH_CONFIG_PAGES: usize = 20;
const MAX_TOOLKIT_PAGES: usize = 20;
const LIST_PAGE_LIMIT: u32 = 1000;
const COMPOSIO_LOGO_BASE_URL: &str = "https://logos.composio.dev/api";

/// The API path Composio redirects the browser back to. It is a constant
/// (not configurable) so the SDK callback URL and the router route cannot
/// drift apart.
pub const CALLBACK_PATH: &str = "/api/integrations/composio/callback";

/// The persistence seam for the local connection mirror; satisfied by the
/// patchbay-db query wrappers in the wiring slice, faked in tests.
#[async_trait::async_trait]
pub trait Store: Send + Sync {
    async fn upsert_user_composio_connection(&self, p: UpsertConnectionParams) -> Result<()>;
    async fn list_active_user_composio_connections(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<ComposioConnectionRow>>;
    async fn get_user_composio_connection(
        &self,
        id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<ComposioConnectionRow>>;
    async fn mark_user_composio_connection_revoked(&self, id: Uuid, user_id: Uuid) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct UpsertConnectionParams {
    pub user_id: Uuid,
    pub toolkit_slug: String,
    pub auth_config_id: String,
    pub connected_account_id: String,
    /// Invariant: composio_user_id == Patchbay user id.
    pub composio_user_id: String,
}

/// A local `user_composio_connection` row as the service sees it.
#[derive(Debug, Clone)]
pub struct ComposioConnectionRow {
    pub id: Uuid,
    pub toolkit_slug: String,
    pub status: String,
    pub connected_account_id: String,
    pub connected_at_unix: i64,
}

/// The subset of `*sdk.Client` the service depends on. Declared as a trait
/// so tests can inject a fake without hitting Composio.
#[async_trait::async_trait]
pub trait Sdk: Send + Sync {
    async fn create_link(
        &self,
        req: CreateLinkRequest,
    ) -> std::result::Result<sdk_types::CreateLinkResponse, SdkError>;
    async fn list_connected_accounts(
        &self,
        req: ListConnectedAccountsRequest,
    ) -> std::result::Result<sdk_types::ListConnectedAccountsResponse, SdkError>;
    async fn list_auth_configs(
        &self,
        req: ListAuthConfigsRequest,
    ) -> std::result::Result<sdk_types::ListAuthConfigsResponse, SdkError>;
    async fn list_toolkits(
        &self,
        req: ListToolkitsRequest,
    ) -> std::result::Result<sdk_types::ListToolkitsResponse, SdkError>;
    async fn revoke_connection(
        &self,
        connected_account_id: &str,
    ) -> std::result::Result<(), SdkError>;
    async fn delete_connected_account(
        &self,
        connected_account_id: &str,
    ) -> std::result::Result<(), SdkError>;
    async fn create_session(
        &self,
        req: CreateSessionRequest,
    ) -> std::result::Result<sdk_types::CreateSessionResponse, SdkError>;
    fn mcp_auth_headers(&self) -> Vec<(String, String)>;
}

#[async_trait::async_trait]
impl Sdk for Client {
    async fn create_link(
        &self,
        req: CreateLinkRequest,
    ) -> std::result::Result<sdk_types::CreateLinkResponse, SdkError> {
        Client::create_link(self, req).await
    }
    async fn list_connected_accounts(
        &self,
        req: ListConnectedAccountsRequest,
    ) -> std::result::Result<sdk_types::ListConnectedAccountsResponse, SdkError> {
        Client::list_connected_accounts(self, req).await
    }
    async fn list_auth_configs(
        &self,
        req: ListAuthConfigsRequest,
    ) -> std::result::Result<sdk_types::ListAuthConfigsResponse, SdkError> {
        Client::list_auth_configs(self, req).await
    }
    async fn list_toolkits(
        &self,
        req: ListToolkitsRequest,
    ) -> std::result::Result<sdk_types::ListToolkitsResponse, SdkError> {
        Client::list_toolkits(self, req).await
    }
    async fn revoke_connection(&self, id: &str) -> std::result::Result<(), SdkError> {
        Client::revoke_connection(self, id).await
    }
    async fn delete_connected_account(&self, id: &str) -> std::result::Result<(), SdkError> {
        Client::delete_connected_account(self, id).await
    }
    async fn create_session(
        &self,
        req: CreateSessionRequest,
    ) -> std::result::Result<sdk_types::CreateSessionResponse, SdkError> {
        Client::create_session(self, req).await
    }
    fn mcp_auth_headers(&self) -> Vec<(String, String)> {
        Client::mcp_auth_headers(self)
    }
}

/// Configures a [`Service`].
pub struct ServiceConfig {
    /// Signs the connect-state HMAC. Required (non-empty).
    pub state_secret: Vec<u8>,
    /// Absolute, public base URL of THIS API, no trailing slash. Required.
    pub callback_base_url: String,
    /// Web app base used to build the post-callback browser redirect. May
    /// be empty → site-relative redirect paths.
    pub frontend_base_url: String,
    /// Overrides the default connect-state lifetime. Zero uses default.
    pub state_ttl: Duration,
    /// Overrides how long the toolkit→auth-config map is cached.
    pub auth_config_ttl: Duration,
}

/// The Composio business-integration service.
pub struct Service {
    sdk: Arc<dyn Sdk>,
    store: Arc<dyn Store>,
    secret: Vec<u8>,
    callback_url: String,
    frontend_url: String,
    state_ttl: Duration,

    inner: Mutex<AuthCacheInner>,
    auth_cache_ttl: Duration,
}

struct AuthCacheInner {
    /// Resolved toolkit_slug → auth_config_id map for the project. Rebuilt
    /// from Composio's /auth_configs endpoint on first use and whenever the
    /// expiry has passed.
    cache: Option<HashMap<String, String>>,
    exp: SystemTime,
    now: Box<dyn Fn() -> SystemTime + Send + Sync>,
}

impl Service {
    /// Validates its inputs and returns a ready service. It errors when a
    /// required dependency is missing so a misconfigured boot fails loudly
    /// instead of returning 500s at request time.
    pub fn new(sdk: Arc<dyn Sdk>, store: Arc<dyn Store>, cfg: ServiceConfig) -> Result<Self> {
        if cfg.state_secret.is_empty() {
            anyhow::bail!("composio: StateSecret is required");
        }
        let base = cfg.callback_base_url.trim().trim_end_matches('/');
        if base.is_empty() {
            anyhow::bail!("composio: CallbackBaseURL is required");
        }
        let ttl = if cfg.state_ttl.is_zero() {
            DEFAULT_STATE_TTL
        } else {
            cfg.state_ttl
        };
        let auth_ttl = if cfg.auth_config_ttl.is_zero() {
            DEFAULT_AUTH_CACHE_TTL
        } else {
            cfg.auth_config_ttl
        };
        Ok(Self {
            sdk,
            store,
            secret: cfg.state_secret,
            callback_url: format!("{base}{CALLBACK_PATH}"),
            frontend_url: cfg
                .frontend_base_url
                .trim()
                .trim_end_matches('/')
                .to_string(),
            state_ttl: ttl,
            inner: Mutex::new(AuthCacheInner {
                cache: None,
                exp: SystemTime::UNIX_EPOCH,
                now: Box::new(SystemTime::now),
            }),
            auth_cache_ttl: auth_ttl,
        })
    }

    /// Overrides the clock (tests).
    pub fn set_now(&self, now: Box<dyn Fn() -> SystemTime + Send + Sync>) {
        self.inner.lock().unwrap().now = now;
    }

    /// Validates the toolkit, mints a signed state, and asks Composio for a
    /// hosted Connect Link. The returned redirect URL is where the caller
    /// sends the user's browser.
    ///
    /// The composio_user_id sent to Composio is the Patchbay user id verbatim
    /// — the invariant the rest of the integration relies on.
    pub async fn begin_connect(&self, user_id: Uuid, toolkit_slug: &str) -> Result<String> {
        let slug = toolkit_slug.to_lowercase();
        let slug = slug.trim();
        let auth_config_id = self.auth_config_for_toolkit(slug).await?;
        if auth_config_id.is_empty() {
            return Err(ServiceError::ToolkitNotSupported.into());
        }
        if user_id.is_nil() {
            anyhow::bail!("composio: invalid user id");
        }
        let composio_user_id = user_id.to_string();

        let now = (self.inner.lock().unwrap().now)();
        let exp = unix_of(now).saturating_add(self.state_ttl.as_secs() as i64);
        let state = sign_state(
            &self.secret,
            &StateClaims {
                user_id: composio_user_id.clone(),
                toolkit_slug: slug.to_string(),
                auth_config_id: auth_config_id.clone(),
                exp,
            },
        )
        .map_err(|e| anyhow!("composio: sign state: {e}"))?;

        // Composio appends its own status / connected_account_id query
        // params to the callback URL and preserves ours, so the signed
        // state rides back to us on the redirect.
        let callback_url = format!(
            "{}?state={}",
            self.callback_url,
            urlencode_component(&state)
        );

        let resp = self
            .sdk
            .create_link(CreateLinkRequest {
                auth_config_id,
                user_id: composio_user_id,
                callback_url,
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow!("composio: create link: {e}"))?;
        Ok(resp.redirect_url)
    }

    /// Verifies the signed state and, on a successful Composio status,
    /// upserts the local connection row. It returns the toolkit slug from
    /// the state so the handler can build the right redirect even on the
    /// not-successful path.
    ///
    /// Idempotency: the upsert is keyed on (user_id, connected_account_id),
    /// so a duplicate callback re-activates the same row instead of
    /// creating a second.
    pub async fn complete_callback(
        &self,
        state: &str,
        status: &str,
        connected_account_id: &str,
    ) -> std::result::Result<String, CompleteCallbackFailure> {
        let now = (self.inner.lock().unwrap().now)();
        let claims = verify_state(&self.secret, state, now).map_err(|e| match e {
            StateError::Malformed => CompleteCallbackFailure::State(e),
            other => CompleteCallbackFailure::State(other),
        })?;

        if !status.trim().eq_ignore_ascii_case("success") {
            // Honor the state for the redirect slug, but do not write an
            // active row.
            return Err(CompleteCallbackFailure::NotSuccessful {
                toolkit_slug: claims.toolkit_slug.clone(),
                source: ServiceError::ConnectNotSuccessful,
            });
        }
        if connected_account_id.trim().is_empty() {
            return Err(CompleteCallbackFailure::Other {
                toolkit_slug: claims.toolkit_slug.clone(),
                error: anyhow!("composio: callback missing connected_account_id"),
            });
        }
        let Ok(user_id) = Uuid::parse_str(&claims.user_id) else {
            return Err(CompleteCallbackFailure::Other {
                toolkit_slug: claims.toolkit_slug.clone(),
                error: anyhow!("composio: state has invalid user id"),
            });
        };

        // The auth_config_id was resolved at BeginConnect and signed into
        // the state, so we compare against THAT exact value rather than
        // re-resolving here. Defense-in-depth: confirm with Composio that
        // this account actually belongs to the state's user and was created
        // under the toolkit's auth config. Any mismatch fails closed with
        // AccountVerification.
        let auth_config_id = claims.auth_config_id.clone();
        if let Err(e) = self
            .verify_account_ownership(connected_account_id, &claims.user_id, &auth_config_id)
            .await
        {
            return Err(CompleteCallbackFailure::Verification {
                toolkit_slug: claims.toolkit_slug.clone(),
                source: e
                    .downcast::<ServiceError>()
                    .unwrap_or(ServiceError::AccountVerification),
            });
        }

        self.store
            .upsert_user_composio_connection(UpsertConnectionParams {
                user_id,
                toolkit_slug: claims.toolkit_slug.clone(),
                auth_config_id: auth_config_id.clone(),
                connected_account_id: connected_account_id.to_string(),
                composio_user_id: claims.user_id.clone(),
            })
            .await
            .map_err(|e| CompleteCallbackFailure::Other {
                toolkit_slug: claims.toolkit_slug.clone(),
                error: anyhow!("composio: upsert connection: {e:#}"),
            })?;
        Ok(claims.toolkit_slug)
    }

    /// Returns the user's active connections.
    pub async fn list_connections(&self, user_id: Uuid) -> Result<Vec<Connection>> {
        let rows = self
            .store
            .list_active_user_composio_connections(user_id)
            .await?;
        Ok(rows.iter().map(row_to_connection).collect())
    }

    /// Builds an originator-scoped task MCP overlay from the same service and
    /// connection store used by the HTTP integration routes.
    pub async fn build_task_overlay(
        &self,
        capability_user_id: Option<Uuid>,
        toolkit_allowlist: &[String],
        display_name_for_slug: impl Fn(&str) -> String,
    ) -> Result<crate::dispatch::OverlayResult> {
        let rows = match capability_user_id {
            Some(user_id) => {
                self.store
                    .list_active_user_composio_connections(user_id)
                    .await?
            }
            None => Vec::new(),
        };
        crate::dispatch::build_task_overlay(
            self,
            capability_user_id,
            toolkit_allowlist,
            &rows,
            display_name_for_slug,
        )
        .await
    }

    /// Revokes and deletes the connection at Composio, then marks the local
    /// row revoked. It is idempotent: a Composio 404 (already gone) is
    /// treated as success, and re-revoking an already-revoked local row is
    /// a no-op.
    ///
    /// A connection id that does not belong to the user (or does not exist
    /// at all) returns ConnectionNotFound so the handler can 404 without
    /// leaking existence across users.
    pub async fn disconnect(&self, user_id: Uuid, connection_id: Uuid) -> Result<()> {
        let Some(row) = self
            .store
            .get_user_composio_connection(connection_id, user_id)
            .await?
        else {
            return Err(ServiceError::ConnectionNotFound.into());
        };

        // Already disconnected locally: a repeat DELETE is a pure no-op.
        // Short-circuiting keeps disconnect idempotent even when the
        // upstream now answers revoke/delete with a NON-404 error: the
        // account is already gone, so re-hitting Composio could turn a
        // second DELETE into a 502 and break the 204-idempotent contract.
        if !row.status.eq_ignore_ascii_case("active") {
            return Ok(());
        }

        // Revoke the upstream grant first, then delete the Composio record.
        // Both tolerate a 404 so a repeated disconnect still resolves the
        // local row.
        if let Err(e) = self.sdk.revoke_connection(&row.connected_account_id).await {
            if !e.is_not_found() {
                return Err(anyhow!("composio: revoke connection: {e}"));
            }
        }
        if let Err(e) = self
            .sdk
            .delete_connected_account(&row.connected_account_id)
            .await
        {
            if !e.is_not_found() {
                return Err(anyhow!("composio: delete connected account: {e}"));
            }
        }

        self.store
            .mark_user_composio_connection_revoked(connection_id, user_id)
            .await
            .map_err(|e| anyhow!("composio: mark revoked: {e}"))?;
        Ok(())
    }

    /// Opens a Composio tool-router (MCP) session scoped to the user's
    /// active connections. It returns `None` when the user has no active
    /// connections — callers treat that as "no MCP overlay for this user".
    ///
    /// Single-account constraint (v1): the MVP connect flow assumes AT MOST
    /// ONE active connection per (user, toolkit). Rows arrive newest-first,
    /// so we keep the FIRST occurrence per toolkit (the most recently
    /// connected account) instead of letting a later map write silently
    /// select an older one.
    pub async fn create_mcp_session(&self, user_id: Uuid) -> Result<Option<McpSession>> {
        let rows = self
            .store
            .list_active_user_composio_connections(user_id)
            .await?;
        if rows.is_empty() {
            return Ok(None);
        }

        let mut connected_accounts = serde_json::Map::new();
        for row in &rows {
            // Keep the first (newest) account per toolkit; ignore older
            // duplicates.
            if connected_accounts.contains_key(&row.toolkit_slug) {
                continue;
            }
            connected_accounts.insert(
                row.toolkit_slug.clone(),
                serde_json::json!([row.connected_account_id]),
            );
        }

        let resp = self
            .sdk
            .create_session(CreateSessionRequest {
                user_id: user_id.to_string(),
                toolkits: None,
                connected_accounts: Some(serde_json::Value::Object(connected_accounts)),
            })
            .await
            .map_err(|e| anyhow!("composio: create session: {e}"))?;
        Ok(Some(McpSession {
            url: resp.mcp.url,
            headers: self.sdk.mcp_auth_headers(),
        }))
    }

    /// Builds the browser redirect target for the callback handler. On
    /// success it points at the settings page (Integrations tab) with the
    /// connected toolkit slug; on failure it carries a stable error code.
    /// When FrontendBaseURL is unset it returns a site-relative path.
    pub fn callback_redirect(&self, slug: &str, success: bool) -> String {
        let path = if success {
            format!(
                "/settings?tab=integrations&connected={}",
                urlencode_component(slug)
            )
        } else {
            "/settings?tab=integrations&error=composio_connect_failed".to_string()
        };
        format!("{}{path}", self.frontend_url)
    }

    /// Returns only the Composio toolkits the project can actually connect
    /// (those with an enabled auth config). Toolkits with no enabled auth
    /// config are filtered out entirely (PB-4009): a card the user can't
    /// act on is noise. A resolver error is NOT masked into an
    /// everything-not-connectable catalog — we return the error so the
    /// handler can surface a 502.
    pub async fn list_toolkits(&self) -> Result<Vec<ToolkitView>> {
        let connectable = self
            .auth_config_map()
            .await
            .map_err(|e| anyhow!("composio: resolve connectable toolkits: {e:#}"))?;

        let mut out: Vec<ToolkitView> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor = String::new();
        for _page in 0..MAX_TOOLKIT_PAGES {
            let resp = self
                .sdk
                .list_toolkits(ListToolkitsRequest {
                    limit: LIST_PAGE_LIMIT,
                    cursor: cursor.clone(),
                    sort_by: "usage".into(),
                    ..Default::default()
                })
                .await
                .map_err(|e| anyhow!("composio: list toolkits: {e}"))?;
            for tk in &resp.items {
                let slug = tk.slug.trim().to_lowercase();
                if slug.is_empty() || seen.contains(&slug) {
                    continue;
                }
                seen.insert(slug.clone());
                // Filter out toolkits with no enabled auth config: the user
                // has no working action for them.
                if !connectable.contains_key(&slug) {
                    continue;
                }
                let category = tk.categories.first().cloned().unwrap_or_default();
                out.push(ToolkitView {
                    slug: tk.slug.clone(),
                    name: tk.name.clone(),
                    logo_url: toolkit_logo_url(&slug, &tk.logo_url),
                    category,
                    // Every surfaced toolkit is connectable by construction.
                    // The wire field is kept for backward compatibility with
                    // older desktop clients that branch on it.
                    connectable: true,
                });
            }
            if resp.next_cursor.is_empty() {
                break;
            }
            cursor = resp.next_cursor;
        }
        Ok(out)
    }

    /// Returns the chosen auth_config_id for a toolkit slug, or "" when the
    /// project has no enabled auth config for it.
    async fn auth_config_for_toolkit(&self, slug: &str) -> Result<String> {
        let slug = slug.trim().to_lowercase();
        if slug.is_empty() {
            return Ok(String::new());
        }
        let m = self.auth_config_map().await?;
        Ok(m.get(&slug).cloned().unwrap_or_default())
    }

    /// Returns the toolkit_slug → auth_config_id map for the project,
    /// rebuilding it from Composio when the cache is empty or expired. A
    /// stale snapshot is served on refresh failure so a transient blip does
    /// not make every toolkit suddenly un-connectable.
    async fn auth_config_map(&self) -> Result<HashMap<String, String>> {
        // Check the cache under the lock, then drop it before any await —
        // the fetch runs unlocked, and the winner publishes its result.
        {
            let inner = self.inner.lock().unwrap();
            let now = (inner.now)();
            if let (Some(cache), exp) = (&inner.cache, inner.exp) {
                if now < exp {
                    return Ok(cache.clone());
                }
            }
        }
        match Self::fetch_auth_config_map(&*self.sdk).await {
            Ok(m) => {
                let mut inner = self.inner.lock().unwrap();
                let now = (inner.now)();
                inner.cache = Some(m.clone());
                inner.exp = now + self.auth_cache_ttl;
                Ok(m)
            }
            Err(e) => {
                // Serve a stale snapshot if we have one.
                let inner = self.inner.lock().unwrap();
                if let Some(cache) = &inner.cache {
                    return Ok(cache.clone());
                }
                Err(e)
            }
        }
    }

    /// Pages through the project's ENABLED auth configs and reduces them to
    /// one chosen auth_config_id per toolkit slug ([`better_auth_config`]
    /// picks the winner when a toolkit has several).
    async fn fetch_auth_config_map(sdk: &dyn Sdk) -> Result<HashMap<String, String>> {
        #[derive(Debug, Clone)]
        struct AuthCandidate {
            id: String,
            managed: bool,
            updated: String,
        }
        let mut best: HashMap<String, AuthCandidate> = HashMap::new();
        let mut cursor = String::new();
        for _page in 0..MAX_AUTH_CONFIG_PAGES {
            let resp = sdk
                .list_auth_configs(ListAuthConfigsRequest {
                    show_disabled: false,
                    limit: LIST_PAGE_LIMIT,
                    cursor: cursor.clone(),
                    ..Default::default()
                })
                .await
                .map_err(|e| anyhow!("composio: list auth configs: {e}"))?;
            for ac in &resp.items {
                if ac.id.is_empty() || ac.status.eq_ignore_ascii_case("DISABLED") {
                    continue;
                }
                let slug = ac.toolkit.slug.trim().to_lowercase();
                if slug.is_empty() {
                    continue;
                }
                let cand = AuthCandidate {
                    id: ac.id.clone(),
                    managed: ac.is_composio_managed,
                    updated: ac.last_updated_at.clone(),
                };
                match best.get(&slug) {
                    Some(cur) => {
                        let cur_tuple = (cur.id.clone(), cur.managed, cur.updated.clone());
                        if !better_auth_config(
                            &(cand.id.clone(), cand.managed, cand.updated.clone()),
                            &cur_tuple,
                        ) {
                            continue;
                        }
                        best.insert(slug, cand);
                    }
                    None => {
                        best.insert(slug, cand);
                    }
                }
            }
            if resp.next_cursor.is_empty() {
                break;
            }
            cursor = resp.next_cursor;
        }
        Ok(best.into_iter().map(|(slug, c)| (slug, c.id)).collect())
    }

    /// Confirms with Composio that connectedAccountID really belongs to
    /// expectedUserID and was created under expectedAuthConfigID, so a
    /// tampered or cross-toolkit connected_account_id on the callback
    /// cannot smuggle another account into the local mirror. It fails
    /// closed: an upstream error, an unknown account, an owner mismatch, an
    /// EMPTY expected auth config, or an auth-config mismatch all fail.
    async fn verify_account_ownership(
        &self,
        connected_account_id: &str,
        expected_user_id: &str,
        expected_auth_config_id: &str,
    ) -> Result<()> {
        let resp = self
            .sdk
            .list_connected_accounts(ListConnectedAccountsRequest {
                connected_account_ids: vec![connected_account_id.to_string()],
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow!("composio: verify connected account: {e}"))?;
        let acct = resp.items.iter().find(|a| a.id == connected_account_id);
        let Some(acct) = acct else {
            return Err(ServiceError::AccountVerification.into());
        };
        if acct.user_id != expected_user_id {
            return Err(ServiceError::AccountVerification.into());
        }
        // Fail closed: the account MUST have been created under the exact
        // auth config the connect link used. An empty expected value is
        // rejected rather than skipped — skipping is the fail-open hole.
        let account_auth_config_id = if acct.auth_config_id.is_empty() {
            acct.auth_config.id.as_str()
        } else {
            acct.auth_config_id.as_str()
        };
        if expected_auth_config_id.is_empty() || account_auth_config_id != expected_auth_config_id {
            return Err(ServiceError::AccountVerification.into());
        }
        Ok(())
    }
}

impl crate::dispatch::SessionSpawner for Service {
    async fn create_session(
        &self,
        user_id: String,
        toolkits_enable: &[String],
        pinned: &BTreeMap<String, Vec<String>>,
    ) -> crate::dispatch::SessionResult {
        let response = self
            .sdk
            .create_session(CreateSessionRequest {
                user_id,
                toolkits: Some(serde_json::json!({"enable": toolkits_enable})),
                connected_accounts: Some(serde_json::to_value(pinned)?),
            })
            .await
            .map_err(|error| anyhow!("composio: create session: {error}"))?;
        Ok(Some((response.mcp.url, self.sdk.mcp_auth_headers())))
    }
}

/// Failure taxonomy of [`Service::complete_callback`] — the caller needs
/// the toolkit slug for the redirect even on failure paths.
#[derive(Debug, thiserror::Error)]
pub enum CompleteCallbackFailure {
    #[error("{0}")]
    State(#[source] StateError),
    #[error("{source}")]
    NotSuccessful {
        toolkit_slug: String,
        #[source]
        source: ServiceError,
    },
    #[error("{source}")]
    Verification {
        toolkit_slug: String,
        source: ServiceError,
    },
    #[error("{error}")]
    Other {
        toolkit_slug: String,
        #[source]
        error: anyhow::Error,
    },
}

impl CompleteCallbackFailure {
    pub fn toolkit_slug(&self) -> Option<&str> {
        match self {
            Self::State(_) => None,
            Self::NotSuccessful { toolkit_slug, .. }
            | Self::Verification { toolkit_slug, .. }
            | Self::Other { toolkit_slug, .. } => Some(toolkit_slug),
        }
    }
}

/// The API-facing view of a local connection row. The Composio
/// connected_account_id and auth_config_id are intentionally omitted — they
/// are server-internal handles, not API surface.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Connection {
    pub id: String,
    #[serde(rename = "toolkit_slug")]
    pub toolkit_slug: String,
    pub status: String,
    pub connected_at: String,
    #[serde(rename = "last_used_at", skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
}

/// The result of create_mcp_session: the streamable MCP URL plus the
/// headers an MCP client must attach. Headers carry the Composio x-api-key,
/// so callers must route them through the redact pipeline before logging.
#[derive(Debug, Clone)]
pub struct McpSession {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

/// The API-facing descriptor for one Composio toolkit. `connectable` is
/// always true on the wire since PB-4009 (only connectable toolkits are
/// returned); the field is retained for backward compatibility with older
/// desktop clients that branch on it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolkitView {
    pub slug: String,
    pub name: String,
    #[serde(rename = "logo", skip_serializing_if = "String::is_empty")]
    pub logo_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub category: String,
    pub connectable: bool,
}

fn toolkit_logo_url(slug: &str, upstream_logo_url: &str) -> String {
    if !upstream_logo_url.is_empty() {
        return upstream_logo_url.to_string();
    }
    let slug = slug.trim().to_lowercase();
    if slug.is_empty() {
        return String::new();
    }
    format!("{COMPOSIO_LOGO_BASE_URL}/{}", urlencode_component(&slug))
}

/// Maps a DB row to the API-facing Connection view.
fn row_to_connection(row: &ComposioConnectionRow) -> Connection {
    Connection {
        id: row.id.to_string(),
        toolkit_slug: row.toolkit_slug.clone(),
        status: row.status.clone(),
        connected_at: chrono::DateTime::from_timestamp(row.connected_at_unix, 0)
            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_default(),
        last_used_at: None,
    }
}

/// Reports whether candidate `a` should win over the currently selected
/// `b` for the same toolkit. A custom (bring-your-own OAuth) config beats a
/// Composio-managed one — it is the white-label path the product wants —
/// and among configs of the same kind the most recently updated wins.
pub fn better_auth_config(a: &(String, bool, String), b: &(String, bool, String)) -> bool {
    if a.1 != b.1 {
        return !a.1;
    }
    a.2 > b.2
}

fn unix_of(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn urlencode_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::{
        CreateLinkResponse, ListAuthConfigsResponse, ListConnectedAccountsResponse,
        ListToolkitsResponse,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    // ── test doubles ─────────────────────────────────────────────────────

    #[derive(Default)]
    struct FakeSdk {
        links: Mutex<Vec<CreateLinkRequest>>,
        accounts: Mutex<Vec<sdk_types::ConnectedAccount>>,
        sessions: Mutex<Vec<CreateSessionRequest>>,
        fail_revoke_non404: AtomicU64,
    }

    impl FakeSdk {
        fn last_link(&self) -> CreateLinkRequest {
            self.links.lock().unwrap().last().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Sdk for FakeSdk {
        async fn create_link(
            &self,
            req: CreateLinkRequest,
        ) -> std::result::Result<sdk_types::CreateLinkResponse, SdkError> {
            self.links.lock().unwrap().push(req);
            Ok(CreateLinkResponse {
                redirect_url: "https://connect.test/1".into(),
                ..Default::default()
            })
        }
        async fn list_connected_accounts(
            &self,
            _req: ListConnectedAccountsRequest,
        ) -> std::result::Result<ListConnectedAccountsResponse, SdkError> {
            Ok(ListConnectedAccountsResponse {
                items: self.accounts.lock().unwrap().clone(),
                ..Default::default()
            })
        }
        async fn list_auth_configs(
            &self,
            _req: ListAuthConfigsRequest,
        ) -> std::result::Result<ListAuthConfigsResponse, SdkError> {
            use crate::sdk::{AuthConfig, Toolkit};
            Ok(ListAuthConfigsResponse {
                items: vec![
                    AuthConfig {
                        id: "ac_managed".into(),
                        status: "ENABLED".into(),
                        is_composio_managed: true,
                        last_updated_at: "2026-01-02T00:00:00Z".into(),
                        toolkit: Toolkit {
                            slug: "github".into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    AuthConfig {
                        id: "ac_custom".into(),
                        status: "ENABLED".into(),
                        is_composio_managed: false,
                        last_updated_at: "2026-01-01T00:00:00Z".into(),
                        toolkit: Toolkit {
                            slug: "github".into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                ],
                ..Default::default()
            })
        }
        async fn list_toolkits(
            &self,
            _req: ListToolkitsRequest,
        ) -> std::result::Result<ListToolkitsResponse, SdkError> {
            use crate::sdk::Toolkit;
            Ok(ListToolkitsResponse {
                items: vec![
                    Toolkit {
                        slug: "github".into(),
                        name: "GitHub".into(),
                        ..Default::default()
                    },
                    Toolkit {
                        slug: "notion".into(),
                        name: "Notion".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            })
        }
        async fn revoke_connection(&self, _id: &str) -> std::result::Result<(), SdkError> {
            if self.fail_revoke_non404.load(Ordering::SeqCst) == 1 {
                return Err(SdkError::Transport("upstream 502".into()));
            }
            Ok(())
        }
        async fn delete_connected_account(&self, _id: &str) -> std::result::Result<(), SdkError> {
            Ok(())
        }
        async fn create_session(
            &self,
            req: CreateSessionRequest,
        ) -> std::result::Result<sdk_types::CreateSessionResponse, SdkError> {
            self.sessions.lock().unwrap().push(req);
            Ok(sdk_types::CreateSessionResponse {
                session_id: "sess".into(),
                mcp: crate::sdk::McpDescriptor {
                    r#type: "http".into(),
                    url: "https://mcp.test/sess".into(),
                },
            })
        }
        fn mcp_auth_headers(&self) -> Vec<(String, String)> {
            vec![("x-api-key".into(), "key".into())]
        }
    }

    #[derive(Default)]
    struct FakeStore {
        rows: Mutex<Vec<ComposioConnectionRow>>,
        listed_users: Mutex<Vec<Uuid>>,
    }

    const USER: Uuid = uuid::uuid!("0198c0de-0000-7000-8000-000000000001");

    #[async_trait::async_trait]
    impl Store for FakeStore {
        async fn upsert_user_composio_connection(&self, p: UpsertConnectionParams) -> Result<()> {
            let mut rows = self.rows.lock().unwrap();
            if let Some(existing) = rows
                .iter_mut()
                .find(|r| r.toolkit_slug == p.toolkit_slug && USER == p.user_id)
            {
                existing.connected_account_id = p.connected_account_id;
                existing.status = "active".into();
                return Ok(());
            }
            rows.push(ComposioConnectionRow {
                id: Uuid::now_v7(),
                toolkit_slug: p.toolkit_slug,
                status: "active".into(),
                connected_account_id: p.connected_account_id,
                connected_at_unix: 1_700_000_000,
            });
            Ok(())
        }
        async fn list_active_user_composio_connections(
            &self,
            user_id: Uuid,
        ) -> Result<Vec<ComposioConnectionRow>> {
            self.listed_users.lock().unwrap().push(user_id);
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.status == "active")
                .cloned()
                .collect())
        }
        async fn get_user_composio_connection(
            &self,
            id: Uuid,
            _user_id: Uuid,
        ) -> Result<Option<ComposioConnectionRow>> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .find(|r| r.id == id)
                .cloned())
        }
        async fn mark_user_composio_connection_revoked(
            &self,
            id: Uuid,
            _user_id: Uuid,
        ) -> Result<()> {
            if let Some(r) = self.rows.lock().unwrap().iter_mut().find(|r| r.id == id) {
                r.status = "revoked".into();
            }
            Ok(())
        }
    }

    fn fixed_now() -> Box<dyn Fn() -> SystemTime + Send + Sync> {
        Box::new(|| SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
    }

    fn service() -> (Service, Arc<FakeSdk>, Arc<FakeStore>) {
        let sdk = Arc::new(FakeSdk::default());
        let store = Arc::new(FakeStore::default());
        let svc = Service::new(
            sdk.clone(),
            store.clone(),
            ServiceConfig {
                state_secret: b"secret".to_vec(),
                callback_base_url: "https://patchbay.test/".into(),
                frontend_base_url: "https://patchbay.test".into(),
                state_ttl: Duration::ZERO,
                auth_config_ttl: Duration::ZERO,
            },
        )
        .unwrap();
        svc.set_now(fixed_now());
        (svc, sdk, store)
    }

    // ── tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn begin_connect_rejects_unknown_toolkit_and_signs_known_one() {
        let (svc, sdk, _) = service();
        assert!(matches!(
            svc.begin_connect(USER, "unknown-toolkit").await,
            Err(e) if e.to_string().contains("toolkit not supported")
        ));
        let url = svc.begin_connect(USER, " GitHub ").await.unwrap();
        assert_eq!(url, "https://connect.test/1");
        let link = sdk.last_link();
        // Custom (non-managed) config beats managed regardless of recency.
        assert_eq!(link.auth_config_id, "ac_custom");
        assert!(link
            .callback_url
            .starts_with("https://patchbay.test/api/integrations/composio/callback?state="));
    }

    #[tokio::test]
    async fn complete_callback_happy_path_upserts_row() {
        let (svc, sdk, store) = service();
        svc.begin_connect(USER, "github").await.unwrap();
        let link = sdk.last_link();
        let state = link
            .callback_url
            .split_once("state=")
            .unwrap()
            .1
            .to_string();
        // The SDK fake must report the account under the signed auth config.
        sdk.accounts
            .lock()
            .unwrap()
            .push(sdk_types::ConnectedAccount {
                id: "ca_1".into(),
                user_id: USER.to_string(),
                auth_config_id: "ac_custom".into(),
                ..Default::default()
            });
        let slug = svc
            .complete_callback(&state, "success", "ca_1")
            .await
            .unwrap();
        assert_eq!(slug, "github");
        assert_eq!(store.rows.lock().unwrap().len(), 1);

        // Duplicate callback re-activates the same row (idempotent upsert).
        let _ = svc
            .complete_callback(&state, "success", "ca_1")
            .await
            .unwrap();
        assert_eq!(store.rows.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn complete_callback_fails_closed_on_cross_account() {
        let (svc, sdk, store) = service();
        svc.begin_connect(USER, "github").await.unwrap();
        let link = sdk.last_link();
        let state = link
            .callback_url
            .split_once("state=")
            .unwrap()
            .1
            .to_string();

        // Wrong user.
        sdk.accounts
            .lock()
            .unwrap()
            .push(sdk_types::ConnectedAccount {
                id: "ca_1".into(),
                user_id: "someone-else".into(),
                auth_config_id: "ac_custom".into(),
                ..Default::default()
            });
        let err = svc
            .complete_callback(&state, "success", "ca_1")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("verification failed"));
        assert!(store.rows.lock().unwrap().is_empty());

        // Empty expected auth config would fail too — but our state always
        // carries one; simulate drift by wrong auth config on the account.
        sdk.accounts.lock().unwrap()[0] = sdk_types::ConnectedAccount {
            id: "ca_1".into(),
            user_id: USER.to_string(),
            auth_config_id: "ac_OTHER".into(),
            ..Default::default()
        };
        let err = svc
            .complete_callback(&state, "success", "ca_1")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("verification failed"));
        assert!(store.rows.lock().unwrap().is_empty());

        // Unknown account id.
        let err = svc
            .complete_callback(&state, "success", "ca_missing")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("verification failed"));
    }

    #[tokio::test]
    async fn complete_callback_not_successful_keeps_slug_but_no_row() {
        let (svc, sdk, store) = service();
        svc.begin_connect(USER, "github").await.unwrap();
        let link = sdk.last_link();
        let (_, state) = link.callback_url.split_once("state=").unwrap();
        let err = svc
            .complete_callback(state, "denied", "")
            .await
            .unwrap_err();
        assert_eq!(err.toolkit_slug(), Some("github"));
        assert!(matches!(err, CompleteCallbackFailure::NotSuccessful { .. }));
        assert!(store.rows.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn disconnect_is_idempotent_and_ownership_checked() {
        let (svc, _sdk, store) = service();
        // Not found.
        assert!(matches!(
            svc.disconnect(USER, Uuid::now_v7()).await,
            Err(e) if e.to_string().contains("connection not found")
        ));
        // Active row revokes fine.
        store.rows.lock().unwrap().push(ComposioConnectionRow {
            id: Uuid::now_v7(),
            toolkit_slug: "github".into(),
            status: "active".into(),
            connected_account_id: "ca_9".into(),
            connected_at_unix: 0,
        });
        let id = store.rows.lock().unwrap()[0].id;
        svc.disconnect(USER, id).await.unwrap();
        assert_eq!(store.rows.lock().unwrap()[0].status, "revoked");
        // Second disconnect is a no-op success even though upstream might
        // answer non-404.
        svc.disconnect(USER, id).await.unwrap();
    }

    #[tokio::test]
    async fn mcp_session_pins_first_account_per_toolkit() {
        let (svc, sdk, store) = service();
        // No connections → None.
        assert!(svc.create_mcp_session(USER).await.unwrap().is_none());

        store.rows.lock().unwrap().extend([
            ComposioConnectionRow {
                id: Uuid::now_v7(),
                toolkit_slug: "github".into(),
                status: "active".into(),
                connected_account_id: "ca_new".into(),
                connected_at_unix: 2,
            },
            ComposioConnectionRow {
                id: Uuid::now_v7(),
                toolkit_slug: "github".into(),
                status: "active".into(),
                connected_account_id: "ca_old".into(),
                connected_at_unix: 1,
            },
        ]);
        let sess = svc.create_mcp_session(USER).await.unwrap().unwrap();
        assert_eq!(sess.url, "https://mcp.test/sess");
        assert_eq!(
            sess.headers,
            vec![("x-api-key".to_string(), "key".to_string())]
        );
        let sent = sdk.sessions.lock().unwrap().last().unwrap().clone();
        let accounts = sent.connected_accounts.unwrap();
        // Newest-wins: only ca_new is pinned under github.
        assert_eq!(accounts["github"], serde_json::json!(["ca_new"]));
    }

    #[tokio::test]
    async fn task_overlay_loads_only_the_capability_users_connections() {
        const SHARED_AGENT_OWNER: Uuid = uuid::uuid!("0198c0de-0000-7000-8000-000000000099");
        assert_ne!(USER, SHARED_AGENT_OWNER);
        let (svc, sdk, store) = service();
        store.rows.lock().unwrap().push(ComposioConnectionRow {
            id: Uuid::now_v7(),
            toolkit_slug: "github".into(),
            status: "active".into(),
            connected_account_id: "callers-account".into(),
            connected_at_unix: 2,
        });

        let result = svc
            .build_task_overlay(Some(USER), &["github".into()], str::to_string)
            .await
            .unwrap();

        assert!(!result.mcp_overlay.is_empty());
        assert_eq!(store.listed_users.lock().unwrap().last(), Some(&USER));
        assert_eq!(
            sdk.sessions.lock().unwrap().last().unwrap().user_id,
            USER.to_string()
        );
    }

    #[tokio::test]
    async fn list_toolkits_filters_to_connectable_and_marks_true() {
        let (svc, _sdk, _store) = service();
        let views = svc.list_toolkits().await.unwrap();
        // Only github has an enabled auth config in the fake.
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].slug, "github");
        assert!(views[0].connectable);
        assert!(views[0]
            .logo_url
            .starts_with("https://logos.composio.dev/api/github"));
    }

    #[test]
    fn callback_redirect_shapes_match_go() {
        let (svc, _, _) = service();
        assert_eq!(
            svc.callback_redirect("github", true),
            "https://patchbay.test/settings?tab=integrations&connected=github"
        );
        assert_eq!(
            svc.callback_redirect("x", false),
            "https://patchbay.test/settings?tab=integrations&error=composio_connect_failed"
        );
    }

    #[test]
    fn better_auth_config_prefers_custom_then_recency() {
        let managed_new = ("ac_m".to_string(), true, "2026-02-01".to_string());
        let custom_old = ("ac_c".to_string(), false, "2026-01-01".to_string());
        assert!(better_auth_config(&custom_old, &managed_new));
        let a = ("ac_a".to_string(), true, "2026-03-01".to_string());
        let b = ("ac_b".to_string(), true, "2026-03-02".to_string());
        assert!(!better_auth_config(&a, &b));
        assert!(better_auth_config(&b, &a));
    }
}
