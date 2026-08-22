//! PluginService core — port of `server/internal/service/plugin.go` +
//! `plugin_hook.go`, Slice 1: the error taxonomy, the pure-function anchors,
//! and the HMAC hook-signing family.
//!
//! Later slices add install/lifecycle (InstallPlugin/SetConfig/Uninstall) and
//! the hook engine (InvokeHook/callHookEndpoint plus the DB-backed rate
//! limiter and circuit breaker). The free functions here take their inputs as
//! parameters (deployment key, manifest JSON, recent counts) so those slices
//! can wrap them in a service struct without reshaping this layer.

use std::error::Error as StdError;
use std::fmt;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

use cordy_plugincontract::{
    CapabilityUnavailable, ConfigField, Hook, Manifest, CONFIG_BOOL, CONFIG_ENUM, CONFIG_NUMBER,
    CONFIG_SECRET, CONFIG_STRING,
};

use crate::plugin_skill;

// ---------------------------------------------------------------------------
// Error taxonomy — plugin.go 129-160
// ---------------------------------------------------------------------------

/// The closed set of failure classes the plugin surface reports. Mirrors Go's
/// `PluginErrorKind` string values byte-for-byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginErrorKind {
    Invalid,
    NotFound,
    Conflict,
    Forbidden,
    Incompatible,
    Quota,
    Unavailable,
}

impl PluginErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Forbidden => "forbidden",
            Self::Incompatible => "incompatible",
            Self::Quota => "quota",
            Self::Unavailable => "unavailable",
        }
    }
}

impl fmt::Display for PluginErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Port of Go's `PluginError`: a classified message with an optional wrapped
/// cause. `Display` renders `"message"` or `"message: source"`, matching Go's
/// `Error()`.
#[derive(Debug)]
pub struct PluginError {
    pub kind: PluginErrorKind,
    pub message: String,
    source: Option<Box<dyn StdError + Send + Sync>>,
}

impl PluginError {
    pub fn new(kind: PluginErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source<E>(kind: PluginErrorKind, message: impl Into<String>, source: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        Self {
            kind,
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            Some(source) => write!(f, "{}: {}", self.message, source),
            None => write!(f, "{}", self.message),
        }
    }
}

impl StdError for PluginError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_ref()
            .map(|e| e.as_ref() as &(dyn StdError + 'static))
    }
}

/// Mirrors Go's `pluginErrf`. Call sites pass an already-formatted string:
/// `plugin_errf(kind, format!("hook {key:?} exceeded ..."))`.
pub fn plugin_errf(kind: PluginErrorKind, message: impl Into<String>) -> PluginError {
    PluginError::new(kind, message)
}

// ---------------------------------------------------------------------------
// Dev origins — plugin.go 248-282
// ---------------------------------------------------------------------------

/// Reads the opt-in dev-origin list. Anything that is not a bare origin is
/// dropped rather than half-honored: a path or query here would read as a
/// broader grant than it is.
pub fn parse_dev_origins(raw: &str) -> Vec<String> {
    let mut origins = Vec::new();
    for candidate in raw.split(',') {
        let candidate = candidate.trim().trim_end_matches('/');
        if candidate.is_empty() {
            continue;
        }
        let Ok(parsed) = url::Url::parse(candidate) else {
            continue;
        };
        // url::Url normalizes a bare origin's path to "/", so both empty and
        // "/" mean "no path".
        let path = parsed.path();
        if parsed.host_str().is_none()
            || (!path.is_empty() && path != "/")
            || parsed.query().is_some()
        {
            continue;
        }
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            continue;
        }
        origins.push(origin_of(&parsed));
    }
    origins
}

/// `scheme://host[:port]`, matching Go's `scheme + "://" + parsed.Host`.
/// `url::Url::host_str()` omits the port, so it is re-appended explicitly.
fn origin_of(parsed: &url::Url) -> String {
    let mut origin = format!(
        "{}://{}",
        parsed.scheme(),
        parsed.host_str().unwrap_or_default()
    );
    if let Some(port) = parsed.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    origin
}

/// Reports whether `source_url`'s origin is on the opted-in list.
pub fn is_dev_origin(origins: &[String], source_url: &str) -> bool {
    if origins.is_empty() {
        return false;
    }
    let Ok(parsed) = url::Url::parse(source_url) else {
        return false;
    };
    if parsed.host_str().is_none() {
        return false;
    }
    let origin = origin_of(&parsed);
    origins.contains(&origin)
}

// ---------------------------------------------------------------------------
// Manifest helpers — plugin.go 188-203, 389-420, 565-595
// ---------------------------------------------------------------------------

/// Flattens the manifest config schema into declaration order so the client
/// renders the same form the server validates against.
pub fn config_fields_for_manifest(manifest: &Manifest) -> Vec<ConfigField> {
    manifest.config.fields.clone()
}

/// Renders the capability gap for the consent screen. Takes the typed error
/// directly: Go needed `errors.As` because its signature accepted any error.
pub fn capability_message(unavailable: &CapabilityUnavailable) -> String {
    if unavailable.missing.is_empty() {
        return "This plugin declares capabilities that are not enabled yet".to_string();
    }
    format!(
        "This plugin declares capabilities that are not enabled yet: {}",
        unavailable.missing.join(", ")
    )
}

/// Returns the scopes the consent screen must newly ask for: everything in
/// `wanted` that `granted` does not already cover.
pub fn added_scopes(granted: &[String], wanted: &[String]) -> Vec<String> {
    wanted
        .iter()
        .filter(|scope| !granted.contains(scope))
        .cloned()
        .collect()
}

/// Decodes the stored scope list. `None` mirrors Go's nil: absent or corrupt
/// storage reads as "no scopes", never as an install-time panic.
pub fn decode_scopes(raw: &[u8]) -> Option<Vec<String>> {
    if raw.is_empty() {
        return None;
    }
    serde_json::from_slice(raw).ok()
}

/// Granted scopes must match the manifest exactly: partial consent would leave
/// the plugin silently broken, and consenting to a scope the manifest does not
/// request would grant access the administrator was never shown a reason for.
pub fn require_exact_scopes(
    manifest_scopes: &[String],
    granted_scopes: &[String],
) -> Result<(), PluginError> {
    if manifest_scopes.len() != granted_scopes.len() {
        return Err(plugin_errf(
            PluginErrorKind::Conflict,
            "granted_scopes must match the manifest scopes exactly",
        ));
    }
    for scope in granted_scopes {
        if !manifest_scopes.contains(scope) {
            return Err(plugin_errf(
                PluginErrorKind::Conflict,
                format!("granted_scopes contains {scope:?}, which the manifest does not request"),
            ));
        }
    }
    Ok(())
}

/// Drops every config key the stored manifest no longer declares. Values are
/// re-encoded with sorted keys, matching Go's map marshaling.
pub fn prune_config(raw: &[u8], manifest: &Manifest) -> String {
    if raw.is_empty() {
        return "{}".to_string();
    }
    let Ok(values) = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(raw)
    else {
        return "{}".to_string();
    };
    let pruned: serde_json::Map<String, serde_json::Value> = values
        .into_iter()
        .filter(|(key, _)| manifest.config.field(key).is_some())
        .collect();
    serde_json::to_string(&pruned).unwrap_or_else(|_| "{}".to_string())
}

/// Bridge for Go's `util.ParseUUID`. Kept as a named anchor so later slices'
/// call sites read like the Go originals.
pub fn parse_uuid_value(value: &str) -> Result<Uuid, uuid::Error> {
    Uuid::parse_str(value)
}

/// Bridge for Go's `util.UUIDToString`.
pub fn uuid_string(value: Uuid) -> String {
    value.to_string()
}

/// Error decoding the consented manifest snapshot off an installation row.
#[derive(Debug, thiserror::Error)]
#[error("decode stored plugin manifest: {0}")]
pub struct StoredManifestError(#[from] pub serde_json::Error);

/// Reads back the consented snapshot. Callers must use this, never a freshly
/// fetched manifest: what the source URL serves today is not what the
/// administrator approved. Takes the row's manifest bytes so the function
/// stays pure; slice 2 passes `installation.manifest` through.
pub fn parse_installation_manifest(manifest_json: &[u8]) -> Result<Manifest, StoredManifestError> {
    let manifest: Manifest = serde_json::from_slice(manifest_json)?;
    Ok(manifest)
}

// ---------------------------------------------------------------------------
// Config value normalization — plugin.go 694-740
// ---------------------------------------------------------------------------

/// Bounds one plain (non-secret) config value before storage.
pub const MAX_CONFIG_VALUE_BYTES: usize = 4096;

/// Validates one config value against its field's declared type. Secret fields
/// fall through to the unsupported arm here: they are handled by the encrypted
/// store in the SetConfig slice, never normalized into `config`.
pub fn normalize_config_value(
    field: &ConfigField,
    value: &serde_json::Value,
) -> Result<serde_json::Value, PluginError> {
    let label = format!("config field {:?}", field.key);
    match field.field_type.as_str() {
        CONFIG_STRING => {
            let text = value.as_str().ok_or_else(|| {
                plugin_errf(
                    PluginErrorKind::Invalid,
                    format!("{label} must be a string"),
                )
            })?;
            if text.len() > MAX_CONFIG_VALUE_BYTES {
                return Err(plugin_errf(
                    PluginErrorKind::Invalid,
                    format!("{label} exceeds {MAX_CONFIG_VALUE_BYTES} bytes"),
                ));
            }
            Ok(serde_json::Value::String(text.to_string()))
        }
        CONFIG_NUMBER => {
            let number = value.as_f64().ok_or_else(|| {
                plugin_errf(
                    PluginErrorKind::Invalid,
                    format!("{label} must be a number"),
                )
            })?;
            let encoded = serde_json::Number::from_f64(number).ok_or_else(|| {
                plugin_errf(
                    PluginErrorKind::Invalid,
                    format!("{label} must be a number"),
                )
            })?;
            Ok(serde_json::Value::Number(encoded))
        }
        CONFIG_BOOL => {
            let flag = value.as_bool().ok_or_else(|| {
                plugin_errf(
                    PluginErrorKind::Invalid,
                    format!("{label} must be a boolean"),
                )
            })?;
            Ok(serde_json::Value::Bool(flag))
        }
        CONFIG_ENUM => {
            let text = value.as_str().ok_or_else(|| {
                plugin_errf(
                    PluginErrorKind::Invalid,
                    format!("{label} must be a string"),
                )
            })?;
            if field.options.iter().any(|option| option == text) {
                return Ok(serde_json::Value::String(text.to_string()));
            }
            Err(plugin_errf(
                PluginErrorKind::Invalid,
                format!("{label} must be one of the declared options"),
            ))
        }
        _ => Err(plugin_errf(
            PluginErrorKind::Invalid,
            format!("{label} has an unsupported type"),
        )),
    }
}

/// Reports a Postgres 23505, the only class of write conflict the plugin
/// surface can produce. Unlike the task enqueue path this does not filter by
/// constraint name: any unique violation here means the same thing.
pub fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .map(|db_err| db_err.code().as_deref() == Some("23505"))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Hook lookup — plugin_hook.go 148-170
// ---------------------------------------------------------------------------

/// Returns the named hook from an installation's consented manifest.
///
/// Read from the installation row, never from the manifest the source URL
/// serves right now: the admin consented to a specific set of endpoints, and a
/// plugin that later repoints its own hook at somewhere else must go through
/// the upgrade flow to have that take effect.
pub fn find_hook(manifest_json: &[u8], hook_key: &str) -> Result<Hook, PluginError> {
    let manifest = parse_installation_manifest(manifest_json)
        .map_err(|_| plugin_errf(PluginErrorKind::Invalid, "plugin manifest is unreadable"))?;
    manifest
        .contributes
        .hooks
        .iter()
        .find(|hook| hook.key == hook_key)
        .cloned()
        .ok_or_else(|| {
            plugin_errf(
                PluginErrorKind::NotFound,
                format!("this Plugin has no hook named {hook_key:?}"),
            )
        })
}

/// Reports whether the hook declared this trigger. A trigger the manifest did
/// not list is not a call site the host may invent.
pub fn hook_allows_trigger(hook: &Hook, trigger: &str) -> bool {
    hook.triggers.iter().any(|declared| declared == trigger)
}

// ---------------------------------------------------------------------------
// HMAC signing family — plugin_hook.go 395-469
// ---------------------------------------------------------------------------

type HmacSha256 = Hmac<Sha256>;

/// Version prefix on the presented signature header value.
pub const HOOK_SIGNATURE_VERSION: &str = "v1";

/// Prefix under which the derived key is shown to plugin authors.
pub const HOOK_SECRET_PREFIX: &str = "whsec_";

/// Domain-separation info string baked into key derivation.
const HOOK_SIGNING_INFO: &[u8] = b"cordy-plugin-hook-signature:v1:";

/// Accepted clock drift between signing time and verification time.
pub const HOOK_TIMESTAMP_TOLERANCE_SECS: i64 = 5 * 60;

/// Derives this installation's signing secret from the deployment key.
///
/// Derived rather than stored, and this is the asymmetry that decides it: the
/// host must PRODUCE this value to sign with it, so a one-way hash is not an
/// option, and storing a recoverable copy of it in every installation row
/// would put a usable secret in reach of any database read. The install token
/// goes the other way — the plugin produces it and the host only ever verifies
/// — so that one is stored hashed. Same deployment key, opposite directions.
fn hook_signing_key(deployment_key: &[u8], installation_id: Uuid) -> Result<[u8; 32], PluginError> {
    if deployment_key.is_empty() {
        return Err(plugin_errf(
            PluginErrorKind::Unavailable,
            "hooks are disabled: CORDY_PLUGIN_SECRET_KEY is not configured",
        ));
    }
    let mut mac = HmacSha256::new_from_slice(deployment_key).map_err(|_| {
        plugin_errf(
            PluginErrorKind::Unavailable,
            "hook signing key rejected by HMAC",
        )
    })?;
    mac.update(HOOK_SIGNING_INFO);
    mac.update(uuid_string(installation_id).as_bytes());
    Ok(mac.finalize().into_bytes().into())
}

/// The same derived key in the form an author configures on their own server.
/// Shown once at install time next to the install token.
pub fn hook_signing_secret(
    deployment_key: &[u8],
    installation_id: Uuid,
) -> Result<String, PluginError> {
    let key = hook_signing_key(deployment_key, installation_id)?;
    Ok(format!("{}{}", HOOK_SECRET_PREFIX, hex::encode(key)))
}

/// The signed string joins the timestamp and the body with a separator that
/// cannot appear in the timestamp. Signing the body alone would let a captured
/// request be replayed forever; signing a plain concatenation would let a
/// crafted timestamp and body pair swap bytes between the two fields.
fn sign_with_key(key: &[u8], timestamp: &str, body: &[u8]) -> Result<[u8; 32], PluginError> {
    // Unreachable for HMAC (any key length is accepted); kept because the crate API is fallible.
    let Ok(mut mac) = HmacSha256::new_from_slice(key) else {
        return Err(plugin_errf(
            PluginErrorKind::Unavailable,
            "hook signing key rejected by HMAC",
        ));
    };
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    Ok(mac.finalize().into_bytes().into())
}

/// Signs one outbound hook payload. Returns lowercase hex of
/// HMAC-SHA256(key, timestamp + "." + body).
pub fn sign_hook_payload(
    deployment_key: &[u8],
    installation_id: Uuid,
    timestamp: &str,
    body: &[u8],
) -> Result<String, PluginError> {
    let key = hook_signing_key(deployment_key, installation_id)?;
    Ok(hex::encode(sign_with_key(&key, timestamp, body)?))
}

/// Why [`verify_hook_signature`] refused a presented signature. Each variant
/// carries the exact message Go returned, so logs stay greppable across the
/// port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VerifyHookSignatureError {
    #[error("signing secret is not valid hex")]
    SecretNotHex,
    #[error("timestamp is not an integer")]
    TimestampNotInteger,
    #[error("timestamp is outside the accepted window")]
    OutsideWindow,
    #[error("signature does not match")]
    Mismatch,
}

/// Constant-time byte comparison: a byte-at-a-time comparison leaks how much
/// of a guessed signature was right, which is enough to find the rest. Length
/// mismatch returns early, matching `subtle.ConstantTimeCompare` semantics.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The receiver's half, exported so a plugin written in any language and our
/// own tests check signatures the same way — a scheme only one side can
/// implement is a scheme nobody can review.
pub fn verify_hook_signature(
    secret_hex: &str,
    timestamp: &str,
    body: &[u8],
    presented: &str,
    now: DateTime<Utc>,
) -> Result<(), VerifyHookSignatureError> {
    let raw_hex = secret_hex
        .strip_prefix(HOOK_SECRET_PREFIX)
        .unwrap_or(secret_hex);
    let key = hex::decode(raw_hex).map_err(|_| VerifyHookSignatureError::SecretNotHex)?;
    let seconds: i64 = timestamp
        .parse()
        .map_err(|_| VerifyHookSignatureError::TimestampNotInteger)?;
    let drift = now.timestamp() - seconds;
    if drift.abs() > HOOK_TIMESTAMP_TOLERANCE_SECS {
        return Err(VerifyHookSignatureError::OutsideWindow);
    }
    // Unreachable: secret already hex-decoded and HMAC accepts any key length.
    let expected = hex::encode(
        sign_with_key(&key, timestamp, body).map_err(|_| VerifyHookSignatureError::SecretNotHex)?,
    );
    let version_prefix = format!("{}=", HOOK_SIGNATURE_VERSION);
    let presented = presented
        .strip_prefix(version_prefix.as_str())
        .unwrap_or(presented);
    if !constant_time_eq(expected.as_bytes(), presented.as_bytes()) {
        return Err(VerifyHookSignatureError::Mismatch);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Hook telemetry helpers — plugin_hook.go 471-569 (signature-only ports)
// ---------------------------------------------------------------------------

/// Caps how often one hook may be called.
pub const HOOK_RATE_LIMIT: i64 = 120;

/// Window over which [`HOOK_RATE_LIMIT`] applies.
pub const HOOK_RATE_WINDOW_SECS: i64 = 60;

/// Failures within the breaker window that suspend event delivery.
pub const HOOK_BREAKER_THRESHOLD: i64 = 5;

/// Window over which [`HOOK_BREAKER_THRESHOLD`] counts failures.
pub const HOOK_BREAKER_WINDOW_SECS: i64 = 5 * 60;

/// Pure half of Go's `checkHookRate`: the caller performs the DB count over
/// [`HOOK_RATE_WINDOW_SECS`] and hands the total here. A failing telemetry
/// read must not take the feature down, so the DB layer maps errors to a
/// count of 0 before calling this.
pub fn check_hook_rate(recent_count: i64, hook_key: &str) -> Result<(), PluginError> {
    if recent_count >= HOOK_RATE_LIMIT {
        return Err(plugin_errf(
            PluginErrorKind::Quota,
            format!("hook {hook_key:?} exceeded {HOOK_RATE_LIMIT} calls per minute"),
        ));
    }
    Ok(())
}

/// Maps a failed hook call onto the invocation status column. Port note: the
/// Go version also maps `context.DeadlineExceeded` to `"timeout"`; that branch
/// arrives with the invoke slice, which owns the deadline plumbing.
pub fn hook_failure_status(err: &PluginError) -> &'static str {
    match err.kind {
        PluginErrorKind::Forbidden | PluginErrorKind::Quota | PluginErrorKind::Incompatible => {
            "refused"
        }
        _ => "failed",
    }
}

/// Keeps the host's own description and drops anything the remote end
/// supplied. An endpoint that echoes its input could otherwise write issue
/// content into a table that has no deletion path for it. Same port note as
/// [`hook_failure_status`] regarding the timeout branch.
pub fn redact_hook_error(err: &PluginError) -> String {
    truncate_str(&err.message, 500).to_string()
}

/// The exact-host check the consent screen promises. Never a suffix match: one
/// scope string must mean the same thing here as it does in the surface CSP
/// and on the authorization screen.
pub fn host_in_net_scopes(host: &str, domains: &[String]) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    domains
        .iter()
        .any(|domain| domain.to_lowercase() == host.to_lowercase())
}

/// Byte-bounded truncation that stays on a UTF-8 char boundary. Go slices raw
/// bytes; Rust must not split a code point mid-sequence, so the limit floors
/// to the nearest boundary when they disagree.
pub fn truncate_str(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

// ---------------------------------------------------------------------------
// PluginService — plugin.go 37-84, 211-243, 309-356, 358-387, 426-826
// ---------------------------------------------------------------------------

/// Installs from CORDY_PLUGIN_DIR instead of the network. Self-hosted operators
/// and plugin authors need a loop that does not require publishing to a public
/// HTTPS host first.
pub const LOCAL_SOURCE_PREFIX: &str = "local:";

/// Bounds one secret value before encryption — plugin.go 700.
pub const MAX_PLUGIN_SECRET_BYTES: usize = 8192;

/// What the consent screen renders. Deliberately the raw manifest text plus the
/// scope list: there is no signature, no trust tier, and no publisher
/// verification in this model, so the administrator reading the scopes IS the
/// trust decision.
#[derive(Debug, serde::Serialize)]
pub struct PluginPreview {
    pub manifest: Manifest,
    pub scopes: Vec<String>,
    pub config_schema: Vec<ConfigField>,
    /// Describes what changes if this replaces an existing install.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub installed: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub installed_version: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub added_scopes: Vec<String>,
}

/// Owns plugin installation, configuration, and plugin-owned state.
///
/// Port note on the field set: Go's `Queries`/`TxStarter` pair collapses into
/// one sqlx pool — every query takes an executor, and `pool.begin()` is the tx
/// starter. The hook engine fields (callbacks/base URL/feature flags/hook
/// client) arrive with the invoke slice.
pub struct PluginService {
    pub pool: sqlx::PgPool,
    /// Encrypts `secret` config values. None disables secret writes so a
    /// misconfigured deployment fails closed instead of storing plaintext.
    pub secrets: Option<cordy_util::secretbox::SecretBox>,
    /// CORDY_PLUGIN_DIR. Empty disables local sources.
    pub local_dir: String,
    /// CORDY_PLUGIN_DEV_ORIGINS parsed. See [`parse_dev_origins`].
    pub dev_origins: Vec<String>,
    /// Gates which declared contributions this build can actually run.
    pub host: cordy_plugincontract::Capabilities,
    /// The raw CORDY_PLUGIN_SECRET_KEY, used to derive each installation's
    /// hook signing secret. Held separately from secrets because signing needs
    /// a key it can reproduce, not a sealed box.
    pub deployment_key: Vec<u8>,
}

impl PluginService {
    /// Mirrors `NewPluginService`: reads both env vars per construction, the
    /// same per-call freshness Go's `os.Getenv` gives (the daemon is a separate
    /// process, so values are not cached at init).
    pub fn new_from_env(pool: sqlx::PgPool) -> Self {
        let dev_origins =
            parse_dev_origins(&std::env::var("CORDY_PLUGIN_DEV_ORIGINS").unwrap_or_default());
        Self {
            pool,
            secrets: None,
            local_dir: std::env::var("CORDY_PLUGIN_DIR")
                .unwrap_or_default()
                .trim()
                .to_string(),
            dev_origins,
            host: cordy_plugincontract::host_capabilities(),
            deployment_key: cordy_util::secretbox::load_key("CORDY_PLUGIN_SECRET_KEY")
                .unwrap_or_default(),
        }
    }

    /// Test/builder constructor: everything explicit, nothing read from env.
    pub fn with_pool(pool: sqlx::PgPool) -> Self {
        Self {
            pool,
            secrets: None,
            local_dir: String::new(),
            dev_origins: Vec::new(),
            host: cordy_plugincontract::host_capabilities(),
            deployment_key: Vec::new(),
        }
    }

    fn is_dev_origin_url(&self, raw: &str) -> bool {
        is_dev_origin(&self.dev_origins, raw)
    }

    // -- FetchManifest — plugin.go 211-243 ----------------------------------

    /// Resolves a source URL to a parsed manifest plus its canonical bytes.
    ///
    /// Network sources reuse the remote MCP endpoint guard: public HTTPS only,
    /// no userinfo/query/fragment, private and metadata address ranges refused,
    /// and the dialer re-checks the resolved address so a DNS rebind between
    /// validation and connection cannot reach an internal host.
    pub async fn fetch_manifest(
        &self,
        source_url: &str,
    ) -> Result<(Manifest, Vec<u8>), PluginError> {
        let source_url = source_url.trim();
        if source_url.is_empty() {
            return Err(plugin_errf(
                PluginErrorKind::Invalid,
                "source_url is required",
            ));
        }
        if source_url.len() > 2048 {
            return Err(plugin_errf(
                PluginErrorKind::Invalid,
                "source_url is too long",
            ));
        }

        let raw = if let Some(name) = source_url.strip_prefix(LOCAL_SOURCE_PREFIX) {
            self.read_local_manifest(name)?
        } else if self.is_dev_origin_url(source_url) {
            fetch_dev_manifest(source_url).await?
        } else {
            fetch_remote_manifest(source_url).await?
        };

        cordy_plugincontract::parse_manifest(&raw).map_err(|e| {
            PluginError::with_source(PluginErrorKind::Invalid, "plugin manifest is invalid", e)
        })
    }

    // -- readLocalManifest — plugin.go 309-328 -------------------------------

    fn read_local_manifest(&self, name: &str) -> Result<Vec<u8>, PluginError> {
        if self.local_dir.is_empty() {
            return Err(plugin_errf(
                PluginErrorKind::Invalid,
                "local plugin sources require CORDY_PLUGIN_DIR",
            ));
        }
        // The name indexes a directory the operator already controls, but it
        // still arrives over the API, so it must not be able to escape the
        // configured root or name a dotfile path.
        if !valid_local_name(name) {
            return Err(plugin_errf(
                PluginErrorKind::Invalid,
                "local plugin source must be a single directory name under CORDY_PLUGIN_DIR",
            ));
        }
        let path = std::path::Path::new(&self.local_dir)
            .join(name)
            .join(cordy_plugincontract::MANIFEST_FILENAME);
        match std::fs::read(&path) {
            Ok(raw) => Ok(raw),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(plugin_errf(
                PluginErrorKind::NotFound,
                format!(
                    "no {} found for local plugin {name:?}",
                    cordy_plugincontract::MANIFEST_FILENAME
                ),
            )),
            Err(e) => Err(PluginError::with_source(
                PluginErrorKind::Invalid,
                "read local plugin manifest",
                e,
            )),
        }
    }

    // -- readLocalFile — plugin.go 834-854 -----------------------------------

    /// Reads a file from inside a local plugin directory.
    ///
    /// Same containment as [`Self::read_local_manifest`], plus one more: the
    /// joined path is re-checked against the plugin's own directory after
    /// cleaning. `entry` is validated at manifest-parse time to be relative and
    /// traversal-free, but this is the layer that touches the filesystem and it
    /// does not get to assume that.
    pub(crate) fn read_local_file(&self, name: &str, entry: &str) -> Result<Vec<u8>, PluginError> {
        if self.local_dir.is_empty() {
            return Err(plugin_errf(
                PluginErrorKind::Invalid,
                "local plugin sources require CORDY_PLUGIN_DIR",
            ));
        }
        if !valid_local_name(name) {
            return Err(plugin_errf(
                PluginErrorKind::Invalid,
                "local plugin source must be a single directory name under CORDY_PLUGIN_DIR",
            ));
        }
        let root = std::path::Path::new(&self.local_dir).join(name);
        // lexical_cleanpath equivalent: Rust's Path components already drop
        // "." and resolve ".." textually where possible, but the containment
        // check below is what actually refuses escapes.
        let mut clean = std::path::PathBuf::new();
        for component in root.join(entry).components() {
            match component {
                std::path::Component::ParentDir => {
                    if !clean.pop() {
                        return Err(plugin_errf(
                            PluginErrorKind::Invalid,
                            format!("plugin resource {entry:?} escapes its directory"),
                        ));
                    }
                }
                std::path::Component::CurDir | std::path::Component::RootDir => {}
                other => clean.push(other),
            }
        }
        if !clean.starts_with(&root) {
            return Err(plugin_errf(
                PluginErrorKind::Invalid,
                format!("plugin resource {entry:?} escapes its directory"),
            ));
        }
        match std::fs::read(&clean) {
            Ok(raw) => Ok(raw),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(plugin_errf(
                PluginErrorKind::NotFound,
                format!("plugin resource {entry:?} was not found"),
            )),
            Err(e) => Err(PluginError::with_source(
                PluginErrorKind::Invalid,
                "read plugin resource",
                e,
            )),
        }
    }

    // -- PreviewPlugin — plugin.go 361-387 -----------------------------------

    /// Fetches and parses a manifest without writing anything. It is the first
    /// half of the two-step install: the administrator must see the scopes
    /// before an installation row exists.
    pub async fn preview_plugin(
        &self,
        workspace_id: Uuid,
        source_url: &str,
    ) -> Result<PluginPreview, PluginError> {
        let (manifest, _) = self.fetch_manifest(source_url).await?;
        if let Err(unavailable) = manifest.check_capabilities(&self.host) {
            return Err(PluginError::with_source(
                PluginErrorKind::Incompatible,
                capability_message(&unavailable),
                Box::new(unavailable),
            ));
        }

        let mut preview = PluginPreview {
            scopes: manifest.scopes.clone(),
            config_schema: config_fields_for_manifest(&manifest),
            manifest,
            installed: false,
            installed_version: String::new(),
            added_scopes: Vec::new(),
        };
        let key = preview.manifest.key.clone();
        let existing = cordy_db::queries::plugin::get_workspace_plugin_installation_by_key(
            &self.pool,
            workspace_id,
            &key,
        )
        .await;
        match existing {
            Ok(Some(existing)) => {
                preview.installed = true;
                preview.installed_version = existing.version.clone();
                preview.added_scopes = added_scopes(
                    &decode_scopes(&json_bytes(&existing.granted_scopes)).unwrap_or_default(),
                    &preview.manifest.scopes,
                );
            }
            Ok(None) => {}
            Err(e) => {
                return Err(PluginError::with_source(
                    PluginErrorKind::Unavailable,
                    "load existing installation",
                    crate::plugin::box_anyhow(e),
                ))
            }
        }
        Ok(preview)
    }

    // -- InstallPlugin — plugin.go 426-530 ------------------------------------

    /// The second half of the install. granted_scopes must match the manifest
    /// exactly: partial consent would leave the plugin silently broken, and
    /// consenting to a scope the manifest does not request would grant access
    /// the administrator was never shown a reason for.
    pub async fn install_plugin(
        &self,
        workspace_id: Uuid,
        user_id: Uuid,
        source_url: &str,
        granted_scopes: &[String],
    ) -> Result<cordy_db::models::PluginInstallation, PluginError> {
        let (manifest, canonical) = self.fetch_manifest(source_url).await?;
        if let Err(unavailable) = manifest.check_capabilities(&self.host) {
            return Err(PluginError::with_source(
                PluginErrorKind::Incompatible,
                capability_message(&unavailable),
                Box::new(unavailable),
            ));
        }
        require_exact_scopes(&manifest.scopes, granted_scopes)?;
        let scopes_json = serde_json::to_value(&manifest.scopes).map_err(|e| {
            PluginError::with_source(PluginErrorKind::Invalid, "encode granted scopes", e)
        })?;

        let existing = cordy_db::queries::plugin::get_workspace_plugin_installation_by_key(
            &self.pool,
            workspace_id,
            &manifest.key,
        )
        .await
        .map_err(|e| {
            PluginError::with_source(
                PluginErrorKind::Unavailable,
                "load existing installation",
                crate::plugin::box_anyhow(e),
            )
        })?;

        let Some(existing) = existing else {
            // A transaction even though it is one row: the skill resources land
            // in the same commit. An installation whose skills half-arrived is
            // worse than one that failed, because the missing half is invisible.
            let mut tx = self.pool.begin().await.map_err(|e| {
                PluginError::with_source(PluginErrorKind::Unavailable, "begin install", e)
            })?;
            let installation = cordy_db::queries::plugin::create_plugin_installation(
                &mut *tx,
                workspace_id,
                &manifest.key,
                source_url,
                &manifest.version,
                &canonical_json_value(&canonical),
                &scopes_json,
                user_id,
            )
            .await
            .map_err(|e| {
                if is_unique_violation_anyhow(&e) {
                    // Two admins installing the same plugin at once: one
                    // loses the unique index. That is a conflict to retry,
                    // not a broken backend.
                    plugin_errf(
                        PluginErrorKind::Conflict,
                        "this plugin is already installed in this workspace",
                    )
                } else {
                    PluginError::with_source(
                        PluginErrorKind::Unavailable,
                        "install plugin",
                        crate::plugin::box_anyhow(e),
                    )
                }
            })?
            .expect("INSERT .. RETURNING always yields a row");
            plugin_skill::install_skill_resources_in_tx(
                self,
                &mut tx,
                &installation,
                &manifest,
                source_url,
                user_id,
            )
            .await?;
            tx.commit().await.map_err(|e| {
                PluginError::with_source(PluginErrorKind::Unavailable, "commit install", e)
            })?;
            return Ok(installation);
        };

        // Upgrade in place. Values whose fields the new manifest dropped are
        // pruned rather than left as state nothing can reach — and that applies
        // to secrets too, which is the case where unreachable residue is
        // ciphertext. The whole upgrade is one transaction so a snapshot can
        // never land with the previous version's secrets still attached.
        let config = prune_config(&json_bytes(&existing.config), &manifest);
        let orphaned_secrets = self.orphaned_secret_keys(existing.id, &manifest).await?;

        let mut tx = self.pool.begin().await.map_err(|e| {
            PluginError::with_source(PluginErrorKind::Unavailable, "begin upgrade", e)
        })?;

        for key in &orphaned_secrets {
            cordy_db::queries::plugin::delete_plugin_secret(&mut *tx, existing.id, key)
                .await
                .map_err(|e| {
                    PluginError::with_source(
                        PluginErrorKind::Unavailable,
                        "prune plugin secret",
                        crate::plugin::box_anyhow(e),
                    )
                })?;
        }
        let updated = cordy_db::queries::plugin::update_plugin_installation_manifest(
            &mut *tx,
            existing.id,
            source_url,
            &manifest.version,
            &canonical_json_value(&canonical),
            &scopes_json,
            &serde_json::from_str::<serde_json::Value>(&config)
                .unwrap_or(serde_json::Value::Object(Default::default())),
        )
        .await
        .map_err(|e| {
            PluginError::with_source(
                PluginErrorKind::Unavailable,
                "upgrade plugin",
                crate::plugin::box_anyhow(e),
            )
        })?
        .expect("UPDATE .. RETURNING on an existing row yields a row");
        // Re-run on upgrade so a changed SKILL.md takes effect and a dropped
        // one is pruned. Same transaction as the manifest snapshot: the two
        // must never disagree about what this version contributes.
        plugin_skill::install_skill_resources_in_tx(
            self, &mut tx, &updated, &manifest, source_url, user_id,
        )
        .await?;
        tx.commit().await.map_err(|e| {
            PluginError::with_source(PluginErrorKind::Unavailable, "commit upgrade", e)
        })?;
        Ok(updated)
    }

    /// Returns stored secrets the new manifest no longer declares as a secret
    /// field — including a field that changed type away from secret.
    async fn orphaned_secret_keys(
        &self,
        installation_id: Uuid,
        manifest: &Manifest,
    ) -> Result<Vec<String>, PluginError> {
        let stored =
            cordy_db::queries::plugin::list_plugin_secret_keys(&self.pool, installation_id)
                .await
                .map_err(|e| {
                    PluginError::with_source(
                        PluginErrorKind::Unavailable,
                        "list plugin secrets",
                        crate::plugin::box_anyhow(e),
                    )
                })?;
        Ok(stored
            .into_iter()
            .filter(|row| {
                manifest
                    .config
                    .field(&row.key)
                    .map(|field| field.field_type != CONFIG_SECRET)
                    .unwrap_or(true)
            })
            .map(|row| row.key)
            .collect())
    }

    // -- SetConfig — plugin.go 600-690 ----------------------------------------

    /// Validates values against the stored manifest schema and splits them by
    /// destination: plain values go on the installation row, secrets go to the
    /// encrypted table. A secret value never lands in `config`.
    pub async fn set_config(
        &self,
        installation: &cordy_db::models::PluginInstallation,
        values: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<cordy_db::models::PluginInstallation, PluginError> {
        let manifest =
            parse_installation_manifest(&json_bytes(&installation.manifest)).map_err(|_| {
                plugin_errf(
                    PluginErrorKind::Invalid,
                    "stored plugin manifest is unreadable",
                )
            })?;

        let mut plain = serde_json::Map::new();
        let mut secrets: Vec<(String, String)> = Vec::new();
        for (key, value) in values {
            let Some(field) = manifest.config.field(key) else {
                return Err(plugin_errf(
                    PluginErrorKind::Invalid,
                    format!("unknown config field {key:?}"),
                ));
            };
            if field.field_type == CONFIG_SECRET {
                let Some(text) = value.as_str() else {
                    return Err(plugin_errf(
                        PluginErrorKind::Invalid,
                        format!("config field {key:?} must be a string"),
                    ));
                };
                if text.len() > MAX_PLUGIN_SECRET_BYTES {
                    return Err(plugin_errf(
                        PluginErrorKind::Invalid,
                        format!("config field {key:?} exceeds {MAX_PLUGIN_SECRET_BYTES} bytes"),
                    ));
                }
                secrets.push((key.clone(), text.to_string()));
                continue;
            }
            let normalized = normalize_config_value(field, value)?;
            plain.insert(key.clone(), normalized);
        }

        // Merge over the stored values so a partial update does not silently
        // clear fields the form did not submit.
        let mut merged = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(
            &json_bytes(&installation.config),
        )
        .unwrap_or_default();
        for (key, value) in plain {
            merged.insert(key, value);
        }
        let encoded = serde_json::Value::Object(merged);

        if !secrets.is_empty() && self.secrets.is_none() {
            return Err(plugin_errf(
                PluginErrorKind::Unavailable,
                "plugin secrets are disabled: CORDY_PLUGIN_SECRET_KEY is not configured",
            ));
        }

        // Secrets and plain values are two tables with no foreign key between
        // them, so one transaction is what keeps a saved form from landing
        // half-applied.
        let mut tx = self.pool.begin().await.map_err(|e| {
            PluginError::with_source(PluginErrorKind::Unavailable, "begin configure", e)
        })?;

        for (key, text) in &secrets {
            // An empty submission clears the secret rather than storing "".
            if text.is_empty() {
                cordy_db::queries::plugin::delete_plugin_secret(&mut *tx, installation.id, key)
                    .await
                    .map_err(|e| {
                        PluginError::with_source(
                            PluginErrorKind::Unavailable,
                            "clear plugin secret",
                            crate::plugin::box_anyhow(e),
                        )
                    })?;
                continue;
            }
            let box_ref = self.secrets.as_ref().expect("checked above");
            let ciphertext = box_ref.seal(text.as_bytes()).map_err(|e| {
                PluginError::with_source(PluginErrorKind::Unavailable, "encrypt plugin secret", e)
            })?;
            cordy_db::queries::plugin::upsert_plugin_secret(
                &mut *tx,
                installation.id,
                key,
                &ciphertext,
            )
            .await
            .map_err(|e| {
                PluginError::with_source(
                    PluginErrorKind::Unavailable,
                    "store plugin secret",
                    crate::plugin::box_anyhow(e),
                )
            })?;
        }

        let updated = cordy_db::queries::plugin::update_plugin_installation_config(
            &mut *tx,
            installation.id,
            &encoded,
        )
        .await
        .map_err(|e| {
            PluginError::with_source(
                PluginErrorKind::Unavailable,
                "store plugin config",
                crate::plugin::box_anyhow(e),
            )
        })?
        .expect("UPDATE .. RETURNING on an existing row yields a row");
        tx.commit().await.map_err(|e| {
            PluginError::with_source(PluginErrorKind::Unavailable, "commit configure", e)
        })?;
        Ok(updated)
    }

    // -- ConfiguredSecretKeys / SetEnabled / Uninstall / InstallationForWorkspace

    /// Reports which secret fields hold a value. It returns names only — no
    /// endpoint ever returns a secret value, not even masked.
    pub async fn configured_secret_keys(
        &self,
        installation_id: Uuid,
    ) -> Result<Vec<String>, PluginError> {
        let rows = cordy_db::queries::plugin::list_plugin_secret_keys(&self.pool, installation_id)
            .await
            .map_err(|e| {
                PluginError::with_source(
                    PluginErrorKind::Unavailable,
                    "list plugin secrets",
                    crate::plugin::box_anyhow(e),
                )
            })?;
        Ok(rows.into_iter().map(|row| row.key).collect())
    }

    /// Toggles an installation. Disabling hides every contribution immediately
    /// but keeps storage and secrets, so re-enabling is not a reinstall.
    pub async fn set_enabled(
        &self,
        installation: &cordy_db::models::PluginInstallation,
        enabled: bool,
    ) -> Result<cordy_db::models::PluginInstallation, PluginError> {
        cordy_db::queries::plugin::set_plugin_installation_enabled(
            &self.pool,
            installation.id,
            enabled,
        )
        .await
        .map_err(|e| {
            PluginError::with_source(
                PluginErrorKind::Unavailable,
                "update plugin state",
                crate::plugin::box_anyhow(e),
            )
        })?
        .ok_or_else(|| plugin_errf(PluginErrorKind::Unavailable, "update plugin state"))
    }

    /// Removes the installation with its storage and secrets. There are no
    /// foreign keys or cascades by repository policy, so the deletes share one
    /// transaction: a partial uninstall would strand plugin-owned rows nothing
    /// can reach or clean up later.
    pub async fn uninstall(
        &self,
        installation: &cordy_db::models::PluginInstallation,
    ) -> Result<(), PluginError> {
        let mut tx = self.pool.begin().await.map_err(|e| {
            PluginError::with_source(PluginErrorKind::Unavailable, "begin uninstall", e)
        })?;

        macro_rules! delete_step {
            ($expr:expr, $msg:literal) => {
                $expr.await.map_err(|e| {
                    PluginError::with_source(
                        PluginErrorKind::Unavailable,
                        $msg,
                        crate::plugin::box_anyhow(e),
                    )
                })?
            };
        }
        let id = installation.id;
        delete_step!(
            cordy_db::queries::plugin::delete_plugin_storage_by_installation(&mut *tx, id),
            "delete plugin storage"
        );
        delete_step!(
            cordy_db::queries::plugin::delete_plugin_secrets_by_installation(&mut *tx, id),
            "delete plugin secrets"
        );
        // Hook records go too. They name an installation that is about to stop
        // existing, and keeping them would leave rows nothing can attribute —
        // the same reason storage and secrets are cleared here rather than
        // swept later.
        delete_step!(
            cordy_db::queries::plugin::delete_plugin_invocations_by_installation(&mut *tx, id),
            "delete plugin invocations"
        );
        // The skills this installation contributed go with it. Scoped by
        // plugin_installation_id, so a skill a person wrote is untouched even
        // if it happens to share a name with something the plugin once provided.
        delete_step!(
            cordy_db::queries::plugin::delete_plugin_skills_by_installation(&mut *tx, id),
            "delete plugin skills"
        );
        delete_step!(
            cordy_db::queries::plugin::delete_plugin_installation(&mut *tx, id),
            "delete plugin installation"
        );
        tx.commit().await.map_err(|e| {
            PluginError::with_source(PluginErrorKind::Unavailable, "commit uninstall", e)
        })?;
        Ok(())
    }

    /// Loads an installation and confirms it belongs to the workspace in the
    /// URL, so an id from another workspace cannot be operated on.
    pub async fn installation_for_workspace(
        &self,
        workspace_id: Uuid,
        installation_id: &str,
    ) -> Result<cordy_db::models::PluginInstallation, PluginError> {
        let parsed = parse_uuid_value(installation_id)
            .map_err(|_| plugin_errf(PluginErrorKind::NotFound, "plugin installation not found"))?;
        let installation = cordy_db::queries::plugin::get_workspace_plugin_installation(
            &self.pool,
            workspace_id,
            parsed,
        )
        .await
        .map_err(|e| {
            PluginError::with_source(
                PluginErrorKind::Unavailable,
                "load plugin installation",
                crate::plugin::box_anyhow(e),
            )
        })?;
        installation
            .ok_or_else(|| plugin_errf(PluginErrorKind::NotFound, "plugin installation not found"))
    }
}

fn valid_local_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && !name.starts_with('.')
}

/// Wraps canonical manifest bytes as a JSON value for the generated queries'
/// `&serde_json::Value` binds. The canonical bytes were produced by serde, so
/// they are valid JSON by construction.
fn canonical_json_value(canonical: &[u8]) -> serde_json::Value {
    serde_json::from_slice(canonical).unwrap_or(serde_json::Value::Null)
}

/// Serializes a JSON column value to bytes for the byte-slice helpers
/// (`decode_scopes`, `prune_config`, `parse_installation_manifest`). The Go
/// port reads these columns as []byte; sqlx maps them to Value.
pub(crate) fn json_bytes(value: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_default()
}

/// Wraps an anyhow error for PluginError::with_source. anyhow::Error does not
/// implement StdError (it IS the error), so the chain's Display is captured
/// into an io::Error — matching Go, whose %w wrapping also only carried the
/// string form.
pub(crate) fn box_anyhow(err: anyhow::Error) -> std::io::Error {
    // The first cause is the underlying sqlx/io error when present.
    let message = err.to_string();
    let root = err
        .chain()
        .nth(1)
        .map(|cause| cause.to_string())
        .unwrap_or_default();
    if root.is_empty() {
        std::io::Error::other(message)
    } else {
        std::io::Error::other(format!("{message}: {root}"))
    }
}

/// The opted-in path: an ordinary bounded GET with no SSRF guard, because the
/// operator named this exact origin.
pub(crate) async fn fetch_dev_manifest(source_url: &str) -> Result<Vec<u8>, PluginError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| {
            PluginError::with_source(PluginErrorKind::Invalid, "build manifest request", e)
        })?;
    let response = client.get(source_url).send().await.map_err(|e| {
        PluginError::with_source(PluginErrorKind::Unavailable, "fetch plugin manifest", e)
    })?;
    let status = response.status();
    if status != reqwest::StatusCode::OK {
        return Err(plugin_errf(
            PluginErrorKind::Unavailable,
            format!("plugin manifest request returned HTTP {}", status.as_u16()),
        ));
    }
    let raw = response.bytes().await.map_err(|e| {
        PluginError::with_source(PluginErrorKind::Unavailable, "read plugin manifest", e)
    })?;
    bound_manifest_bytes(&raw)
}

async fn fetch_remote_manifest(source_url: &str) -> Result<Vec<u8>, PluginError> {
    let endpoint = cordy_remotemcp::validate_public_https_endpoint(source_url, &[], None)
        .await
        .map_err(|e| {
            PluginError::with_source(
                PluginErrorKind::Invalid,
                "source_url must be a plain public HTTPS URL",
                e,
            )
        })?;
    let client = cordy_remotemcp::new_secure_http_client(&endpoint);
    let request = http::Request::builder()
        .method(http::Method::GET)
        .uri(endpoint.as_str())
        .header("Accept", "application/json")
        .body(cordy_remotemcp::RequestBody::from(Vec::new()))
        .map_err(|e| {
            PluginError::with_source(PluginErrorKind::Invalid, "build manifest request", e)
        })?;
    let response = client.send(request).await.map_err(|e| {
        PluginError::with_source(PluginErrorKind::Unavailable, "fetch plugin manifest", e)
    })?;
    let status = response.status();
    if !(status.is_success()) {
        return Err(plugin_errf(
            PluginErrorKind::Unavailable,
            format!("plugin manifest request returned HTTP {}", status.as_u16()),
        ));
    }
    bound_manifest_bytes(&response.into_body())
}

/// Applies Go's `LimitReader(MaxManifestSize+1)` + length check pair: read one
/// byte past the cap so an oversized body is detected as too large rather than
/// silently truncated.
fn bound_manifest_bytes(raw: &[u8]) -> Result<Vec<u8>, PluginError> {
    if raw.len() > cordy_plugincontract::MAX_MANIFEST_SIZE {
        return Err(plugin_errf(
            PluginErrorKind::Invalid,
            format!(
                "plugin manifest exceeds {} bytes",
                cordy_plugincontract::MAX_MANIFEST_SIZE
            ),
        ));
    }
    Ok(raw.to_vec())
}

// crate-internal accessors used by plugin_skill.rs / plugin_hook.rs

pub(crate) fn read_local_file_pub(
    service: &PluginService,
    name: &str,
    entry: &str,
) -> Result<Vec<u8>, PluginError> {
    service.read_local_file(name, entry)
}

pub(crate) async fn fetch_remote_manifest_pub(source_url: &str) -> Result<Vec<u8>, PluginError> {
    fetch_remote_manifest(source_url).await
}

/// Unique-violation check over the generated queries' anyhow errors: sqlx
/// surfaces the database error inside the chain, and 23505 is the only class of
/// write conflict the plugin surface can produce.
pub(crate) fn is_unique_violation_anyhow(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<sqlx::Error>()
            .map(is_unique_violation)
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordy_plugincontract::{
        HookTransport, TRIGGER_AGENT, TRIGGER_EVENT, TRIGGER_MANUAL, TRIGGER_UI,
    };

    fn deployment_key() -> Vec<u8> {
        (1u8..=32).collect()
    }

    fn installation_id() -> Uuid {
        parse_uuid_value("11111111-1111-4111-8111-111111111111").unwrap()
    }

    fn second_installation_id() -> Uuid {
        parse_uuid_value("22222222-2222-4222-8222-222222222222").unwrap()
    }

    fn at(epoch_secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(epoch_secs, 0).unwrap()
    }

    const TIMESTAMP: &str = "1700000000";

    fn field(field_type: &str, options: &[&str]) -> ConfigField {
        ConfigField {
            key: "theme".to_string(),
            field_type: field_type.to_string(),
            label: "Theme".to_string(),
            description: String::new(),
            required: false,
            options: options.iter().map(|o| o.to_string()).collect(),
            placeholder: String::new(),
            multiline: false,
        }
    }

    #[test]
    fn verify_hook_signature_rejects_tampered_body_and_replay() {
        let secret = hook_signing_secret(&deployment_key(), installation_id()).unwrap();
        let body = br#"{"hook_key":"summarize","input":{"issue_id":"abc"}}"#;
        let signature =
            sign_hook_payload(&deployment_key(), installation_id(), TIMESTAMP, body).unwrap();
        let signed_at = at(1700000000);

        assert!(verify_hook_signature(&secret, TIMESTAMP, body, &signature, signed_at).is_ok());

        let tampered = br#"{"hook_key":"summarize","input":{"issue_id":"someone-elses"}}"#;
        assert_eq!(
            verify_hook_signature(&secret, TIMESTAMP, tampered, &signature, signed_at),
            Err(VerifyHookSignatureError::Mismatch),
            "a body edited after signing must not verify",
        );

        let late = signed_at + chrono::Duration::hours(2);
        assert_eq!(
            verify_hook_signature(&secret, TIMESTAMP, body, &signature, late),
            Err(VerifyHookSignatureError::OutsideWindow),
            "a replayed request outside the tolerance window must not verify",
        );
    }

    #[test]
    fn hook_signing_secret_is_per_installation() {
        let first_secret = hook_signing_secret(&deployment_key(), installation_id()).unwrap();
        let second_secret =
            hook_signing_secret(&deployment_key(), second_installation_id()).unwrap();
        assert_ne!(
            first_secret, second_secret,
            "two installations must not share a signing secret",
        );

        let body = br#"{"hook_key":"k"}"#;
        let signature = sign_hook_payload(&deployment_key(), installation_id(), TIMESTAMP, body)
            .expect("signing with a deployment key must succeed");
        assert_eq!(
            verify_hook_signature(&second_secret, TIMESTAMP, body, &signature, at(1700000000)),
            Err(VerifyHookSignatureError::Mismatch),
            "one installation's signature must not verify against another's secret",
        );
    }

    #[test]
    fn hook_signing_secret_is_stable_across_calls() {
        let first = hook_signing_secret(&deployment_key(), installation_id()).unwrap();
        let second = hook_signing_secret(&deployment_key(), installation_id()).unwrap();
        assert_eq!(
            first, second,
            "the same deployment key and installation must derive the same secret",
        );
        let body = first
            .strip_prefix(HOOK_SECRET_PREFIX)
            .expect("signing secret should be prefixed for recognisability");
        assert!(hex::decode(body).is_ok(), "signing secret body must be hex");
    }

    #[test]
    fn hook_signing_fails_closed_without_deployment_key() {
        let err = sign_hook_payload(&[], installation_id(), TIMESTAMP, b"{}")
            .expect_err("signing without a deployment key must fail");
        assert_eq!(err.kind, PluginErrorKind::Unavailable);
    }

    #[test]
    fn hook_allows_trigger_only_when_declared() {
        let hook = Hook {
            triggers: vec![TRIGGER_MANUAL.to_string()],
            transport: HookTransport::default(),
            ..empty_hook()
        };
        assert!(hook_allows_trigger(&hook, TRIGGER_MANUAL));
        for undeclared in [TRIGGER_UI, TRIGGER_EVENT, TRIGGER_AGENT] {
            assert!(
                !hook_allows_trigger(&hook, undeclared),
                "{undeclared} was not declared and must not be allowed",
            );
        }
    }

    const HOOKS_MANIFEST_JSON: &str = r#"{
        "manifest_version": 1,
        "key": "com.example.hooks",
        "name": "Hooks",
        "description": "d",
        "version": "1.0.0",
        "author": {"name": "example"},
        "scopes": ["net:example.com"],
        "contributes": {"hooks": [{
            "key": "summarize",
            "name": "Summarize",
            "description": "Summarize the thread.",
            "triggers": ["manual"],
            "transport": {"type": "http", "url": "https://example.com/hooks/summarize"}
        }]}
    }"#;

    #[test]
    fn find_hook_reads_the_consented_manifest() {
        let manifest_json = HOOKS_MANIFEST_JSON.as_bytes();

        let hook = find_hook(manifest_json, "summarize").expect("declared hook must resolve");
        assert_eq!(hook.transport.url, "https://example.com/hooks/summarize");

        let err = find_hook(manifest_json, "not_declared")
            .expect_err("an undeclared hook key must not resolve");
        assert_eq!(err.kind, PluginErrorKind::NotFound);
    }

    #[test]
    fn normalize_config_value_roundtrips_each_declared_type() {
        let string_field = field(CONFIG_STRING, &[]);
        assert_eq!(
            normalize_config_value(&string_field, &serde_json::json!("dark")).unwrap(),
            serde_json::json!("dark"),
        );

        let number_field = field(CONFIG_NUMBER, &[]);
        assert_eq!(
            normalize_config_value(&number_field, &serde_json::json!(3.5)).unwrap(),
            serde_json::json!(3.5),
        );

        let bool_field = field(CONFIG_BOOL, &[]);
        assert_eq!(
            normalize_config_value(&bool_field, &serde_json::json!(true)).unwrap(),
            serde_json::json!(true),
        );

        let enum_field = field(CONFIG_ENUM, &["dark", "light"]);
        assert_eq!(
            normalize_config_value(&enum_field, &serde_json::json!("light")).unwrap(),
            serde_json::json!("light"),
        );
    }

    #[test]
    fn normalize_config_value_rejects_mismatched_values() {
        let string_field = field(CONFIG_STRING, &[]);
        let err = normalize_config_value(&string_field, &serde_json::json!(1))
            .err()
            .unwrap();
        assert_eq!(err.kind, PluginErrorKind::Invalid);
        assert!(err.message.contains("must be a string"));

        let too_long = "x".repeat(MAX_CONFIG_VALUE_BYTES + 1);
        let err = normalize_config_value(&string_field, &serde_json::json!(too_long))
            .err()
            .unwrap();
        assert!(
            err.message.contains("exceeds 4096 bytes"),
            "{}",
            err.message
        );

        let enum_field = field(CONFIG_ENUM, &["dark", "light"]);
        let err = normalize_config_value(&enum_field, &serde_json::json!("neon"))
            .err()
            .unwrap();
        assert!(err.message.contains("one of the declared options"));

        let secret_field = field(cordy_plugincontract::CONFIG_SECRET, &[]);
        let err = normalize_config_value(&secret_field, &serde_json::json!("s"))
            .err()
            .unwrap();
        assert!(err.message.contains("unsupported type"));
    }

    #[test]
    fn prune_config_keeps_declared_keys_only() {
        let manifest: Manifest = serde_json::from_str(
            r#"{
                "manifest_version": 1,
                "key": "k",
                "name": "n",
                "version": "1.0.0",
                "author": {"name": "a"},
                "scopes": [],
                "config": {
                    "theme": {"type": "string", "label": "Theme"},
                    "count": {"type": "number", "label": "Count"}
                }
            }"#,
        )
        .unwrap();

        let pruned = prune_config(br#"{"theme":"dark","rogue":true,"count":2}"#, &manifest);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&pruned).unwrap(),
            serde_json::json!({"theme": "dark", "count": 2}),
        );

        assert_eq!(prune_config(b"", &manifest), "{}");
        assert_eq!(prune_config(b"not json", &manifest), "{}");
    }

    #[test]
    fn config_fields_preserve_declaration_order() {
        let manifest: Manifest = serde_json::from_str(HOOKS_MANIFEST_JSON).unwrap();
        assert!(config_fields_for_manifest(&manifest).is_empty());

        let with_config: Manifest = serde_json::from_str(
            r#"{
                "manifest_version": 1,
                "key": "k",
                "name": "n",
                "version": "1.0.0",
                "author": {"name": "a"},
                "scopes": [],
                "config": {
                    "theme": {"type": "string", "label": "Theme"},
                    "count": {"type": "number", "label": "Count"}
                }
            }"#,
        )
        .unwrap();
        let fields = config_fields_for_manifest(&with_config);
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].key, "theme");
        assert_eq!(fields[1].key, "count");
    }

    #[test]
    fn require_exact_scopes_accepts_only_exact_sets() {
        let scopes = vec!["issues:read".to_string(), "net:example.com".to_string()];
        assert!(require_exact_scopes(&scopes, &scopes.clone()).is_ok());

        let err = require_exact_scopes(&scopes, &["issues:read".to_string()])
            .err()
            .unwrap();
        assert_eq!(err.kind, PluginErrorKind::Conflict);
        assert_eq!(
            err.message,
            "granted_scopes must match the manifest scopes exactly"
        );

        let err = require_exact_scopes(
            &scopes,
            &["issues:read".to_string(), "tasks:write".to_string()],
        )
        .err()
        .unwrap();
        assert_eq!(err.kind, PluginErrorKind::Conflict);
        assert!(err.message.contains("tasks:write"), "{}", err.message);
    }

    #[test]
    fn added_scopes_reports_only_new_grants() {
        let granted = vec!["issues:read".to_string()];
        let wanted = vec!["issues:read".to_string(), "comments:write".to_string()];
        assert_eq!(
            added_scopes(&granted, &wanted),
            vec!["comments:write".to_string()]
        );
        assert!(added_scopes(&[], &[]).is_empty());
    }

    #[test]
    fn decode_scopes_reads_nil_for_absent_or_corrupt_storage() {
        assert_eq!(decode_scopes(b""), None);
        assert_eq!(decode_scopes(b"{"), None);
        assert_eq!(
            decode_scopes(br#"["issues:read"]"#),
            Some(vec!["issues:read".to_string()]),
        );
    }

    #[test]
    fn host_in_net_scopes_matches_exact_hosts_only() {
        let domains = vec!["Example.COM".to_string()];
        assert!(host_in_net_scopes("example.com", &domains));
        assert!(host_in_net_scopes("EXAMPLE.com.", &domains));
        assert!(!host_in_net_scopes("evil-example.com", &domains));
        assert!(!host_in_net_scopes("sub.example.com", &domains));
    }

    #[test]
    fn parse_dev_origins_keeps_bare_http_origins_only() {
        let origins = parse_dev_origins(
            "http://localhost:3000/, https://team.dev , ftp://team.dev, \
             https://p.dev/path?q=1,, http://ok.io",
        );
        assert_eq!(
            origins,
            vec![
                "http://localhost:3000".to_string(),
                "https://team.dev".to_string(),
                "http://ok.io".to_string(),
            ],
        );
        assert!(is_dev_origin(&origins, "https://team.dev/hooks/x"));
        assert!(!is_dev_origin(&origins, "https://other.dev/hooks/x"));
        assert!(!is_dev_origin(&[], "https://team.dev"));
    }

    #[test]
    fn check_hook_rate_refuses_at_limit() {
        assert!(check_hook_rate(HOOK_RATE_LIMIT - 1, "summarize").is_ok());
        let err = check_hook_rate(HOOK_RATE_LIMIT, "summarize").err().unwrap();
        assert_eq!(err.kind, PluginErrorKind::Quota);
        assert!(
            err.message.contains("120 calls per minute"),
            "{}",
            err.message
        );
    }

    #[test]
    fn hook_failure_status_classifies_refusals() {
        for kind in [
            PluginErrorKind::Forbidden,
            PluginErrorKind::Quota,
            PluginErrorKind::Incompatible,
        ] {
            assert_eq!(hook_failure_status(&plugin_errf(kind, "no")), "refused");
        }
        assert_eq!(
            hook_failure_status(&plugin_errf(PluginErrorKind::Invalid, "no")),
            "failed"
        );
    }

    #[test]
    fn redact_hook_error_truncates_host_message_only() {
        let err = plugin_errf(PluginErrorKind::Invalid, "x".repeat(600));
        assert_eq!(redact_hook_error(&err).len(), 500);
        assert_eq!(
            redact_hook_error(&plugin_errf(PluginErrorKind::Invalid, "short")),
            "short",
        );
    }

    #[test]
    fn truncate_stays_on_char_boundaries() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello", 4), "hell");
        assert_eq!(truncate_str("héllo", 3), "hé");
        assert_eq!(truncate_str("héllo", 2), "h");
    }

    #[test]
    fn plugin_error_display_mirrors_go_wrapping() {
        assert_eq!(
            plugin_errf(PluginErrorKind::Conflict, "boom").to_string(),
            "boom"
        );
        let wrapped = PluginError::with_source(
            PluginErrorKind::Invalid,
            "decode failed",
            Box::new(std::io::Error::other("eof")),
        );
        assert_eq!(wrapped.to_string(), "decode failed: eof");
        assert!(wrapped.source().is_some());
    }

    #[test]
    fn capability_message_lists_missing_capabilities() {
        let unavailable = CapabilityUnavailable {
            missing: vec!["surface modal".to_string()],
        };
        assert_eq!(
            capability_message(&unavailable),
            "This plugin declares capabilities that are not enabled yet: surface modal",
        );
    }

    fn empty_hook() -> Hook {
        Hook {
            key: String::new(),
            name: String::new(),
            description: String::new(),
            input_schema: None,
            triggers: Vec::new(),
            events: Vec::new(),
            transport: HookTransport::default(),
            timeout_ms: 0,
        }
    }
}
