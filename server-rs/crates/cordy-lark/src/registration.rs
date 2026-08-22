//! Lark PersonalAgent registration — port of
//! `server/internal/integrations/lark/registration.go`.
//!
//! A 1:1 implementation of RFC 8628 (OAuth 2.0 Device Authorization Grant)
//! against accounts.feishu.cn (mainland) / accounts.larksuite.com
//! (international). The protocol has only two phases:
//!
//! 1. begin — POST <domain>/oauth/v1/app/registration with action=begin and
//!    (archetype=PersonalAgent / auth_method=client_secret /
//!    request_user_info=open_id). Lark returns a device_code, a
//!    verification_uri_complete (the QR target), a polling interval, and an
//!    expiry. Cordy renders the QR, the user scans it in the Lark app, walks
//!    the "create a PersonalAgent for this account" flow, and authorizes.
//!
//! 2. poll — POST the same URL with action=poll and the device_code. The
//!    server replies with one of:
//!    - {error: "authorization_pending"} — keep polling.
//!    - {error: "slow_down"}             — bump the interval +5s, then poll.
//!    - {user_info: {tenant_brand: "lark"}} — user authorized via the
//!      international tenant; we switch the polling host to
//!      accounts.larksuite.com and keep going.
//!    - {client_id, client_secret, user_info: {open_id}} — terminal success.
//!    - {error: "expired_token"|"access_denied"} — terminal failure.
//!
//! We deliberately inline this client (rather than depend on an external SDK)
//! so the registration surface ships with the same dependency footprint as
//! the rest of the crate — a single-file client is the right size to own when
//! the alternative is dragging a full SDK for one endpoint.

use std::time::Duration;

use serde::Deserialize;

use crate::http_client::{truncate, urlencode_pairs};
use crate::types::{OpenId, Region};

pub const REGISTRATION_DEFAULT_FEISHU_DOMAIN: &str = "https://accounts.feishu.cn";
pub const REGISTRATION_DEFAULT_LARK_DOMAIN: &str = "https://accounts.larksuite.com";

const REGISTRATION_ENDPOINT: &str = "/oauth/v1/app/registration";

/// Default polling cadence Lark uses when the server omits `interval`. 5s
/// matches the Lark SDK; smaller would risk slow_down responses without
/// buying any latency improvement.
const REGISTRATION_DEFAULT_POLL_SECONDS: u64 = 5;

/// Default registration window (10 minutes) — long enough for a user to scan,
/// switch apps, walk the create-bot flow, and authorize on their phone, short
/// enough that an abandoned session does not pin resources for hours.
const REGISTRATION_DEFAULT_EXPIRE_SECONDS: u64 = 600;

/// Internal-tenant brand label Lark uses to flag "you scanned with a Lark
/// (international) account, not a Feishu (mainland) one". When we see this we
/// re-aim polling at accounts.larksuite.com and re-issue the very next poll
/// WITHOUT first waiting for the polling interval — the upstream SDK shows
/// that Lark's server emits the tenant_brand hint exactly once during the
/// polling stream and the subsequent poll must reach the new domain to learn
/// the credentials.
const REGISTRATION_TENANT_BRAND_LARK: &str = "lark";

/// Mirror brand label for the reverse direction: a user who picked the
/// "Bind to Lark" CTA but actually authorized with a mainland Feishu account.
/// The split-CTA UX (MUL-3083) rendered a QR against
/// accounts.larksuite.com, but Lark's poll stream surfaces
/// tenant_brand="feishu" once authorization completes on the wrong cloud, and
/// we honor that signal symmetrically — re-aim polling at accounts.feishu.cn
/// and let the next poll fetch the credentials from the right host. Without
/// this, "wrong entry" was a hard install failure for the lark→feishu
/// direction even though the feishu→lark direction recovered automatically.
const REGISTRATION_TENANT_BRAND_FEISHU: &str = "feishu";

/// Per-call HTTP timeout. The device-flow endpoint is normally a sub-second
/// call but we add headroom for cross-region paths.
const REGISTRATION_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Configures the device-flow client. All fields are optional; the zero value
/// targets accounts.feishu.cn over a standard reqwest client.
#[derive(Clone)]
pub struct RegistrationConfig {
    /// The initial polling host. Default "https://accounts.feishu.cn";
    /// staging deployments can point this at a mock or at the Lark beta
    /// endpoint.
    pub domain: String,

    /// The international-tenant polling host the client switches to when
    /// Lark's poll response surfaces user_info.tenant_brand="lark". Default
    /// "https://accounts.larksuite.com".
    pub lark_domain: String,

    /// The transport for every request the client makes. Empty defaults to a
    /// fresh reqwest client with a 30s timeout.
    pub http_client: Option<reqwest::Client>,

    /// Labels the QR-code URL's `source` query param so Lark's telemetry can
    /// attribute installs back to Cordy. Empty defaults to "cordy".
    pub source: String,
}

impl Default for RegistrationConfig {
    fn default() -> Self {
        Self {
            domain: String::new(),
            lark_domain: String::new(),
            http_client: None,
            source: String::new(),
        }
    }
}

impl RegistrationConfig {
    fn with_defaults(self) -> Self {
        Self {
            domain: if self.domain.is_empty() {
                REGISTRATION_DEFAULT_FEISHU_DOMAIN.to_string()
            } else {
                self.domain
            },
            lark_domain: if self.lark_domain.is_empty() {
                REGISTRATION_DEFAULT_LARK_DOMAIN.to_string()
            } else {
                self.lark_domain
            },
            http_client: Some(self.http_client.unwrap_or_else(|| {
                reqwest::Client::builder()
                    .timeout(REGISTRATION_HTTP_TIMEOUT)
                    .build()
                    .expect("reqwest client")
            })),
            source: if self.source.is_empty() {
                "cordy".to_string()
            } else {
                self.source
            },
        }
    }
}

/// The typed Lark protocol error. The handler pipeline maps `code` to a
/// stable user-facing reason so the UI can render the right copy without
/// parsing prose.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("registration: {}{}", .code, .description.as_ref().map(|d| format!(": {d}")).unwrap_or_default())]
pub struct RegistrationError {
    pub code: String,
    pub description: String,
}

impl RegistrationError {
    fn new(code: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            description: description.into(),
        }
    }
}

/// Returned by RegistrationService when the user explicitly denied the install
/// in the Lark UI. Distinct from other terminal failures so the UI can render
/// "you cancelled the install" instead of a generic error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("lark registration: access denied by user")]
pub struct ErrRegistrationAccessDenied;

/// Returned by RegistrationService when the device_code's expiry window
/// elapsed without the user authorizing. Distinct so the UI can prompt "scan
/// again — the previous QR expired".
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("lark registration: expired")]
pub struct ErrRegistrationExpired;

/// BeginResult is what begin returns to RegistrationService.
#[derive(Debug, Clone)]
pub struct BeginResult {
    pub device_code: String,
    /// The verification_uri_complete with Cordy's `source` telemetry params
    /// appended; render this as a QR image client-side.
    pub qr_code_url: String,
    /// The polling host this session opened against. The service must pass it
    /// back to poll so the international-tenant switch (the switched_domain
    /// branch) can re-aim subsequent polls.
    pub domain: String,
    /// Lark's suggested polling cadence. Slow_down responses add 5s; the
    /// service is responsible for honoring the updated cadence.
    pub interval: Duration,
    /// The absolute lifetime of the device_code. A poll after this window
    /// returns expired_token; the session task uses this to size its timeout.
    pub expires_in: Duration,
}

/// PollResult is the discriminated union of every terminal and non-terminal
/// poll outcome. The caller branches on the populated fields:
/// - success (client_id + client_secret + open_id) → install
/// - switched_domain (new domain string)           → swap host, re-poll immediately
/// - status ("authorization_pending" / "slow_down") → wait, poll again
/// - err (terminal error)                          → abort the session
#[derive(Debug, Clone, Default)]
pub struct PollResult {
    pub client_id: String,
    pub client_secret: String,
    pub open_id: OpenId,

    /// Non-empty when Lark told us "this is the wrong cloud, re-poll over
    /// there." It is paired with switched_region so the caller can update both
    /// the polling host AND the per-install region in one step. Originally
    /// this only fired in the Feishu→Lark direction (Lark international users
    /// authorizing on a Feishu-first begin); after MUL-3083 follow-up it is
    /// symmetric, so a user who picked the "wrong" Bind CTA also recovers —
    /// the service must update the session's stored domain AND region and
    /// re-poll WITHOUT honoring the interval (the SDK does the same — the
    /// upstream behaviour is that the very next poll lands on the new domain
    /// and returns the actual credentials).
    pub switched_domain: String,
    /// The region the new domain belongs to. Set in lockstep with
    /// switched_domain; ignored when switched_domain is empty. Carrying the
    /// region here keeps the caller from having to re-derive it from the
    /// domain string at session-update time.
    pub switched_region: Option<Region>,

    /// Non-terminal protocol signals — typically "authorization_pending" or
    /// "slow_down". The service uses these to decide whether to bump the
    /// polling interval.
    pub status: String,

    /// The terminal error code (e.g. "access_denied", "expired_token", or a
    /// free-form Lark code we did not anticipate). None on a non-terminal
    /// result.
    pub err: Option<RegistrationError>,
}

/// Runs the device-flow protocol but does NOT own session state or
/// installation provisioning — RegistrationService composes the client with
/// the session store and the DB write path. Splitting these lets the protocol
/// client be deterministic and easy to test against a mock server without
/// involving the database.
pub struct RegistrationClient {
    cfg: RegistrationConfig,
}

impl RegistrationClient {
    /// Constructs the device-flow client.
    pub fn new(cfg: RegistrationConfig) -> Self {
        Self {
            cfg: cfg.with_defaults(),
        }
    }

    fn begin_domain(&self, region: Region) -> String {
        // Pick the begin domain off the requested region. Unknown regions
        // degrade to Feishu (mainland) — same back-compat invariant as
        // region_or_default, so callers that pre-date this signature keep
        // working.
        match region {
            Region::Lark => self.cfg.lark_domain.clone(),
            Region::Feishu => self.cfg.domain.clone(),
        }
    }

    /// Opens a new device-flow session against the open-platform host for the
    /// requested region. Region is normally chosen explicitly by the caller
    /// (the user picked "Feishu" or "Lark" in the UI) so the QR renders
    /// against the same cloud the user expects to scan from; the default
    /// falls back to Feishu (mainland) for back-compat with callers that
    /// pre-date region-aware install. Lark may STILL surface a
    /// Lark-international tenant on a subsequent poll even when the begin host
    /// was Feishu — the switched_domain branch in RegistrationService keeps
    /// that auto-detect path alive as a fallback for users who pick the wrong
    /// entry, so explicit region selection is a routing optimization (saves
    /// one round-trip and renders the right cloud's QR up front), not a
    /// constraint on what the device flow can recover from.
    ///
    /// name_preset pre-fills the bot/app name on Lark's "create a
    /// PersonalAgent" form so the installed bot defaults to e.g.
    /// "<agent> - Cordy" instead of Lark's auto-generated
    /// "{用户姓名}的智能助手". It is a user-editable default (the user can
    /// still change it on the form), and it rides on the QR URL — not the
    /// begin POST body, which has no name field. Empty omits the pre-fill.
    pub async fn begin(
        &self,
        name_preset: &str,
        region: Region,
    ) -> anyhow::Result<BeginResult> {
        let domain = self.begin_domain(region);
        let resp: BeginResponse = self
            .do_form(
                &domain,
                &[
                    ("action".to_string(), "begin".to_string()),
                    ("archetype".to_string(), "PersonalAgent".to_string()),
                    ("auth_method".to_string(), "client_secret".to_string()),
                    ("request_user_info".to_string(), "open_id".to_string()),
                ],
            )
            .await?;
        self.process_begin_response(resp, name_preset, &domain)
    }

    fn process_begin_response(
        &self,
        resp: BeginResponse,
        name_preset: &str,
        domain: &str,
    ) -> anyhow::Result<BeginResult> {
        if !resp.error.is_empty() {
            return Err(RegistrationError::new(resp.error, resp.error_description).into());
        }
        if resp.device_code.is_empty() {
            return Err(RegistrationError::new(
                "invalid_response",
                "device_code is empty",
            )
            .into());
        }
        if resp.verification_uri_complete.is_empty() {
            return Err(RegistrationError::new(
                "invalid_response",
                "verification_uri_complete is empty",
            )
            .into());
        }
        let qr = decorate_qr_code_url(&resp.verification_uri_complete, &self.cfg.source, name_preset)
            .map_err(|e| {
                RegistrationError::new(
                    "invalid_response",
                    format!("verification_uri_complete is not a URL: {e}"),
                )
            })?;
        let interval_secs = if resp.interval > 0 {
            resp.interval as u64
        } else {
            REGISTRATION_DEFAULT_POLL_SECONDS
        };
        let expire_secs = if resp.expire_in > 0 {
            resp.expire_in as u64
        } else {
            REGISTRATION_DEFAULT_EXPIRE_SECONDS
        };
        Ok(BeginResult {
            device_code: resp.device_code,
            qr_code_url: qr,
            domain: domain.to_string(),
            interval: Duration::from_secs(interval_secs),
            expires_in: Duration::from_secs(expire_secs),
        })
    }

    /// Runs a single poll round-trip against the supplied domain (which the
    /// caller may have updated mid-session via switched_domain from a prior
    /// PollResult). Domain selection lives outside the client so the session
    /// state machine in RegistrationService is the single source of truth for
    /// which host the next call must hit.
    pub async fn poll(&self, domain: &str, device_code: &str) -> anyhow::Result<PollResult> {
        if device_code.is_empty() {
            return Err(
                RegistrationError::new("invalid_argument", "device_code is required").into(),
            );
        }
        let effective = if domain.is_empty() {
            self.cfg.domain.clone()
        } else {
            domain.to_string()
        };
        let resp: PollResponse = self
            .do_form(
                &effective,
                &[
                    ("action".to_string(), "poll".to_string()),
                    ("device_code".to_string(), device_code.to_string()),
                ],
            )
            .await?;
        Ok(self.process_poll_response(resp, &effective))
    }

    fn process_poll_response(&self, resp: PollResponse, domain: &str) -> PollResult {
        // Tenant-brand-driven domain swap. Lark emits this exactly once on the
        // transition poll when the authorized account does not match the cloud
        // the begin call hit; the next poll must reach the matching
        // open-platform host to learn the credentials. We surface the swap
        // (domain + region) as a typed signal so the service does not have to
        // know the brand string OR re-derive the region from the host.
        //
        // Both directions are honored: feishu→lark for users who scanned a
        // Feishu QR with a Lark-international account, AND lark→feishu for
        // users who picked the new "Bind to Lark" CTA but actually authorized
        // with a mainland Feishu account. Symmetry matters because the
        // split-CTA UI (MUL-3083) also begins on accounts.larksuite.com
        // directly — without the reverse swap, a "wrong entry" install on that
        // side would carry Region::Lark all the way through finish_success and
        // fail (or commit a wrong-region row) at get_bot_info. The check is
        // gated on the current domain so we do not loop on the same brand we
        // already match.
        if let Some(user_info) = &resp.user_info {
            match user_info.tenant_brand.as_str() {
                REGISTRATION_TENANT_BRAND_LARK => {
                    if !domain.starts_with(&self.cfg.lark_domain) {
                        return PollResult {
                            switched_domain: self.cfg.lark_domain.clone(),
                            switched_region: Some(Region::Lark),
                            ..PollResult::default()
                        };
                    }
                }
                REGISTRATION_TENANT_BRAND_FEISHU => {
                    if !domain.starts_with(&self.cfg.domain) {
                        return PollResult {
                            switched_domain: self.cfg.domain.clone(),
                            switched_region: Some(Region::Feishu),
                            ..PollResult::default()
                        };
                    }
                }
                _ => {}
            }
        }

        // Success: both client_id AND client_secret AND the installer open_id
        // must be present. Partial responses are treated as a protocol error
        // so RegistrationService never writes a half-populated installation
        // row.
        if !resp.client_id.is_empty() && !resp.client_secret.is_empty() {
            let open_id = resp.user_info.map(|u| u.open_id).unwrap_or_default();
            if open_id.is_empty() {
                return PollResult {
                    err: Some(RegistrationError::new(
                        "invalid_response",
                        "success response missing installer open_id",
                    )),
                    ..PollResult::default()
                };
            }
            return PollResult {
                client_id: resp.client_id,
                client_secret: resp.client_secret,
                open_id: OpenId(open_id),
                ..PollResult::default()
            };
        }

        match resp.error.as_str() {
            "authorization_pending" | "slow_down" => PollResult {
                status: resp.error,
                ..PollResult::default()
            },
            "access_denied" | "expired_token" => PollResult {
                err: Some(RegistrationError::new(resp.error, resp.error_description)),
                ..PollResult::default()
            },
            "" =>
            // Empty error AND empty credentials = keep polling; this matches
            // the upstream SDK's tolerant handling for the case where the
            // server briefly returns an empty body during the
            // authorize-redirect window.
            {
                PollResult {
                    status: "authorization_pending".to_string(),
                    ..PollResult::default()
                }
            }
            _ => PollResult {
                err: Some(RegistrationError::new(resp.error, resp.error_description)),
                ..PollResult::default()
            },
        }
    }

    async fn do_form<T: serde::de::DeserializeOwned>(
        &self,
        domain: &str,
        form: &[(String, String)],
    ) -> anyhow::Result<T> {
        let endpoint = format!("{}{REGISTRATION_ENDPOINT}", domain.trim_end_matches('/'));
        let resp = self
            .cfg
            .http_client
            .as_ref()
            .expect("with_defaults fills http_client")
            .post(&endpoint)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(urlencode_pairs(form))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("registration: http do: {e}"))?;
        let status = resp.status();
        let body = resp.bytes().await.map_err(|e| anyhow::anyhow!("registration: read body: {e}"))?;
        if body.is_empty() {
            return Err(RegistrationError::new(format!("http_{}", status.as_u16()), "empty body").into());
        }
        // RFC 8628 device-flow servers return non-2xx with a JSON body whose
        // `error` field is the actual signal — `authorization_pending` and
        // `slow_down` arrive as HTTP 400, NOT 2xx. Decoding the body first and
        // letting the caller route on resp.error is what the upstream Go SDK
        // does; treating any non-2xx as a hard protocol error (the previous
        // behaviour) killed every session on the first poll because the user
        // hasn't scanned the QR yet at that point.
        match serde_json::from_slice::<T>(&body) {
            Ok(parsed) => Ok(parsed),
            Err(_) => {
                // Body didn't parse — surface the raw status + payload tail so
                // ops can tell a Lark outage / proxy interception apart from a
                // schema drift. Caller treats this as a terminal protocol
                // error.
                Err(RegistrationError::new(
                    format!("http_{}", status.as_u16()),
                    truncate(&String::from_utf8_lossy(&body), 256),
                )
                .into())
            }
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BeginResponse {
    device_code: String,
    verification_uri_complete: String,
    verification_uri: String,
    user_code: String,
    interval: i64,
    expire_in: i64,
    error: String,
    error_description: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PollResponse {
    client_id: String,
    client_secret: String,
    user_info: Option<PollUserInfo>,
    error: String,
    error_description: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PollUserInfo {
    open_id: String,
    tenant_brand: String,
}

/// Appends the SDK-style telemetry params Lark expects on the QR-image URL.
/// Without `from=sdk&tp=sdk&source=<src>` the scanner UI on the user's phone
/// shows a less polished prompt and Lark cannot attribute installs back to
/// Cordy in their analytics.
///
/// name_preset, when non-empty, is appended as `name=<...>` to pre-fill the
/// bot/app name on Lark's "create a PersonalAgent" form. This mirrors the
/// upstream SDK's AppPreset.Name: Lark reads it from the verification/QR URL
/// (the begin POST body carries no name field) and treats it as a
/// user-editable default, not a locked final name.
fn decorate_qr_code_url(raw: &str, source: &str, name_preset: &str) -> anyhow::Result<String> {
    let mut u = url::Url::parse(raw)?;
    // Set semantics (replace existing keys), mirroring Go's q.Set.
    let existing: Vec<(String, String)> = u
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    u.set_query(None);
    {
        let mut q = u.query_pairs_mut();
        for (k, v) in existing {
            if matches!(k.as_str(), "from" | "tp" | "source")
                || (!name_preset.is_empty() && k == "name")
            {
                continue;
            }
            q.append_pair(&k, &v);
        }
        q.append_pair("from", "sdk");
        q.append_pair("tp", "sdk");
        q.append_pair("source", &format!("go-sdk/{source}"));
        if !name_preset.is_empty() {
            q.append_pair("name", name_preset);
        }
    }
    Ok(u.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> RegistrationClient {
        RegistrationClient::new(RegistrationConfig::default())
    }

    #[test]
    fn config_defaults_match_go_values() {
        let c = RegistrationClient::new(RegistrationConfig::default());
        assert_eq!(c.cfg.domain, "https://accounts.feishu.cn");
        assert_eq!(c.cfg.lark_domain, "https://accounts.larksuite.com");
        assert_eq!(c.cfg.source, "cordy");
        assert!(c.cfg.http_client.is_some());
    }

    #[test]
    fn begin_domain_follows_region() {
        let c = client();
        assert_eq!(c.begin_domain(Region::Feishu), "https://accounts.feishu.cn");
        assert_eq!(
            c.begin_domain(Region::Lark),
            "https://accounts.larksuite.com"
        );
    }

    #[test]
    fn begin_response_happy_path_decorates_qr_and_defaults() {
        let c = client();
        let res = c
            .process_begin_response(
                BeginResponse {
                    device_code: "dev123".into(),
                    verification_uri_complete: "https://accounts.feishu.cn/qr?token=x".into(),
                    ..Default::default()
                },
                "My Agent - Cordy",
                "https://accounts.feishu.cn",
            )
            .unwrap();
        assert_eq!(res.device_code, "dev123");
        assert_eq!(res.domain, "https://accounts.feishu.cn");
        assert_eq!(res.interval, Duration::from_secs(5));
        assert_eq!(res.expires_in, Duration::from_secs(600));
        assert!(res.qr_code_url.contains("from=sdk"));
        assert!(res.qr_code_url.contains("tp=sdk"));
        assert!(res.qr_code_url.contains("source=go-sdk%2Fcordy"));
        assert!(res.qr_code_url.contains("name="));
    }

    #[test]
    fn begin_response_protocol_errors() {
        let c = client();
        let err = c
            .process_begin_response(
                BeginResponse {
                    error: "unsupported_archetype".into(),
                    error_description: "nope".into(),
                    ..Default::default()
                },
                "",
                "",
            )
            .unwrap_err();
        assert!(err.to_string().contains("unsupported_archetype"));

        let err = c
            .process_begin_response(BeginResponse::default(), "", "")
            .unwrap_err();
        assert!(err.to_string().contains("device_code is empty"));

        let err = c
            .process_begin_response(
                BeginResponse {
                    device_code: "d".into(),
                    ..Default::default()
                },
                "",
                "",
            )
            .unwrap_err();
        assert!(err.to_string().contains("verification_uri_complete is empty"));

        let err = c
            .process_begin_response(
                BeginResponse {
                    device_code: "d".into(),
                    verification_uri_complete: "not a url \u{0}".into(),
                    ..Default::default()
                },
                "",
                "",
            )
            .unwrap_err();
        assert!(err.to_string().contains("is not a URL"));
    }

    #[test]
    fn poll_success_requires_all_three_credentials() {
        let c = client();
        let full = c.process_poll_response(
            PollResponse {
                client_id: "cli_x".into(),
                client_secret: "sec".into(),
                user_info: Some(PollUserInfo {
                    open_id: "ou_installer".into(),
                    tenant_brand: String::new(),
                }),
                ..Default::default()
            },
            "https://accounts.feishu.cn",
        );
        assert_eq!(full.client_id, "cli_x");
        assert_eq!(full.open_id.0, "ou_installer");

        let partial = c.process_poll_response(
            PollResponse {
                client_id: "cli_x".into(),
                client_secret: "sec".into(),
                ..Default::default()
            },
            "https://accounts.feishu.cn",
        );
        assert_eq!(partial.err.unwrap().code, "invalid_response");
    }

    #[test]
    fn poll_tenant_brand_switches_both_directions_once() {
        let c = client();
        // feishu → lark: brand hint while still polling the feishu domain.
        let switched = c.process_poll_response(
            PollResponse {
                user_info: Some(PollUserInfo {
                    tenant_brand: "lark".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            "https://accounts.feishu.cn",
        );
        assert_eq!(switched.switched_domain, "https://accounts.larksuite.com");
        assert_eq!(switched.switched_region, Some(Region::Lark));

        // Same hint again AFTER the swap: gated on the current domain, no loop.
        let settled = c.process_poll_response(
            PollResponse {
                user_info: Some(PollUserInfo {
                    tenant_brand: "lark".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            "https://accounts.larksuite.com",
        );
        assert!(settled.switched_domain.is_empty());

        // Reverse direction: lark CTA but feishu account authorized.
        let reverse = c.process_poll_response(
            PollResponse {
                user_info: Some(PollUserInfo {
                    tenant_brand: "feishu".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            "https://accounts.larksuite.com",
        );
        assert_eq!(reverse.switched_domain, "https://accounts.feishu.cn");
        assert_eq!(reverse.switched_region, Some(Region::Feishu));
    }

    #[test]
    fn poll_status_and_terminal_errors() {
        let c = client();
        let pending = c.process_poll_response(
            PollResponse {
                error: "authorization_pending".into(),
                ..Default::default()
            },
            "",
        );
        assert_eq!(pending.status, "authorization_pending");

        let slow = c.process_poll_response(
            PollResponse {
                error: "slow_down".into(),
                ..Default::default()
            },
            "",
        );
        assert_eq!(slow.status, "slow_down");

        let denied = c.process_poll_response(
            PollResponse {
                error: "access_denied".into(),
                error_description: "user said no".into(),
                ..Default::default()
            },
            "",
        );
        assert_eq!(denied.err.unwrap().code, "access_denied");

        let empty = c.process_poll_response(PollResponse::default(), "");
        assert_eq!(empty.status, "authorization_pending");

        let weird = c.process_poll_response(
            PollResponse {
                error: "something_else".into(),
                ..Default::default()
            },
            "",
        );
        assert_eq!(weird.err.unwrap().code, "something_else");
    }

    #[test]
    fn poll_rejects_empty_device_code() {
        let err = futures_executor_block(client().poll("", ""));
        assert!(err.is_err());
    }

    /// Minimal block-on helper so the argument-validation path (which never
    /// touches the network) can be asserted in a sync test.
    fn futures_executor_block(fut: impl std::future::Future<Output = anyhow::Result<PollResult>>) -> anyhow::Result<PollResult> {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(fut)
    }

    #[test]
    fn qr_decoration_replaces_existing_telemetry_keys() {
        let out =
            decorate_qr_code_url("https://h/qr?token=t&from=web&name=Old", "cordy", "").unwrap();
        assert!(out.contains("token=t"));
        assert!(!out.contains("from=web"));
        assert!(out.contains("from=sdk"));
        // No preset → existing name survives untouched.
        assert!(out.contains("name=Old"));

        let out = decorate_qr_code_url("https://h/qr?name=Old", "cordy", "New").unwrap();
        assert!(out.contains("name=New"));
        assert!(!out.contains("name=Old"));
    }

    #[test]
    fn registration_error_display_matches_go_format() {
        assert_eq!(
            RegistrationError::new("expired_token", "").to_string(),
            "registration: expired_token"
        );
        assert_eq!(
            RegistrationError::new("expired_token", "too late").to_string(),
            "registration: expired_token: too late"
        );
    }
}
