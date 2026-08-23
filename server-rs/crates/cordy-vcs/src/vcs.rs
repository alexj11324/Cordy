//! The provider abstraction for token-based Git providers that Cordy
//! mirrors pull requests and CI status from: Forgejo, Gitea (Forgejo's
//! upstream, wire-identical), and GitLab. GitHub is intentionally NOT a
//! vcs provider — its App/installation model and check_suite CI differ
//! enough that it keeps its own handler.
//!
//! Port of `server/internal/integrations/vcs/vcs.go`.
//!
//! Each provider only contributes the parts that actually differ between
//! providers: how a webhook is authenticated, how its event/payload shapes
//! map to the normalized PR/CI structs, and how a token is validated. The
//! shared storage, issue auto-link / auto-close, and broadcast logic live
//! in the handler layer and consume the normalized types.

use anyhow::Result;
use thiserror::Error;

/// Identifies a provider. The string values are persisted on
/// vcs_connection.provider and used as the registry key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Kind(pub &'static str);

impl Kind {
    pub const FORGEJO: Kind = Kind("forgejo");
    pub const GITEA: Kind = Kind("gitea");
    pub const GITLAB: Kind = Kind("gitlab");

    /// Whether this is a known provider kind.
    pub fn valid(self) -> bool {
        matches!(self, Kind::FORGEJO | Kind::GITEA | Kind::GITLAB)
    }
}

/// Returned by [`Provider::validate_token`] when the instance rejects the
/// token (HTTP 401/403). Callers surface it as a connect-time validation
/// failure distinct from transport/instance errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("vcs: token unauthorized")]
pub struct UnauthorizedError;

/// The normalized webhook event category. Anything a provider does not
/// model maps to [`EventKind::Other`] and is acknowledged but ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Other,
    PullRequest,
    CiStatus,
}

/// The provider-agnostic shape of a pull/merge request webhook. State is
/// already normalized to one of open/closed/merged/draft, so the handler
/// never re-derives it. GitLab "merge requests" map onto the same struct.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PullRequestEvent {
    /// The raw provider action (e.g. "opened", "closed", "merge"). The
    /// handler only needs to know whether it is terminal; see
    /// [`PullRequestEvent::terminal`].
    pub action: String,
    pub repo_owner: String,
    pub repo_name: String,
    pub number: i32,
    pub title: String,
    pub body: String,
    /// open | closed | merged | draft
    pub state: String,
    pub html_url: String,
    pub branch: String,
    pub head_sha: String,
    pub author_login: String,
    pub author_avatar_url: String,
    pub additions: i32,
    pub deletions: i32,
    pub changed_files: i32,
    /// RFC3339 or empty
    pub merged_at: String,
    pub closed_at: String,
    pub created_at: String,
    pub updated_at: String,
}

impl PullRequestEvent {
    /// Whether this event is the PR's merge/close event, after which the
    /// close-intent decision must be frozen. Providers spell the terminal
    /// action differently (Forgejo "closed"/"merged", GitLab
    /// "merge"/"close"), so the set is matched here rather than in the
    /// handler.
    pub fn terminal(&self) -> bool {
        matches!(
            self.action.as_str(),
            "closed" | "merged" | "merge" | "close"
        )
    }
}

/// The provider-agnostic shape of a commit-status / pipeline webhook.
/// State is normalized to passed/failed/pending so the aggregation query is
/// provider-independent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CiStatusEvent {
    pub sha: String,
    /// Status check / pipeline name; "" is allowed.
    pub context: String,
    /// passed | failed | pending
    pub state: String,
    pub target_url: String,
    pub description: String,
    /// The provider's own event timestamp (RFC3339 or empty). It feeds the
    /// commit-status monotonic guard so an out-of-order redelivery can't
    /// regress a status; empty means "unknown", and the handler falls back
    /// to ingestion time.
    pub updated_at: String,
}

/// The minimal identity returned by [`Provider::validate_token`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub login: String,
}

/// Errors surfaced by the provider adapters.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error(transparent)]
    Unauthorized(#[from] UnauthorizedError),
    #[error("{0}")]
    Other(String),
}

/// The per-provider adapter. Implementations are stateless and cheap to
/// construct; the registry holds one instance per kind.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn kind(&self) -> Kind;
    /// Classifies an inbound webhook from its headers.
    fn event_kind(&self, headers: &http::HeaderMap) -> EventKind;
    /// Authenticates the raw body against the connection's stored secret.
    /// Forgejo/Gitea use HMAC-SHA256 (X-Gitea-Signature); GitLab uses a
    /// plaintext token compare (X-Gitlab-Token).
    fn verify_signature(&self, secret: &str, headers: &http::HeaderMap, body: &[u8]) -> bool;
    /// Decodes a pull/merge request webhook body.
    fn parse_pull_request(&self, body: &[u8]) -> Result<PullRequestEvent>;
    /// Decodes a commit-status / pipeline webhook body.
    fn parse_ci_status(&self, body: &[u8]) -> Result<CiStatusEvent>;
    /// Confirms the token works against `instance_url` and returns the
    /// authenticated account. Maps a 401/403 to
    /// [`UnauthorizedError`].
    async fn validate_token(&self, instance_url: &str, token: &str) -> Result<Account>;
}

// The registry mapping a Kind to its Provider constructor lives in
// crate::for_kind (lib.rs) to avoid a module cycle: the provider adapters
// consume these shared helpers while the registry names them.

/// Trims whitespace and any trailing slash so stored instance URLs and
/// derived webhook URLs are stable regardless of input.
pub fn normalize_instance_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

/// Collapses (state, draft, merged) into the normalized PR state.
pub fn derive_pr_state(state: &str, draft: bool, merged: bool) -> String {
    if merged {
        return "merged".to_string();
    }
    if state == "closed" {
        return "closed".to_string();
    }
    if draft {
        return "draft".to_string();
    }
    "open".to_string()
}

pub(crate) fn coalesce(a: &str, b: &str) -> String {
    if !a.is_empty() {
        a.to_string()
    } else {
        b.to_string()
    }
}

/// Header name lookup that is case-insensitive the way Go's
/// `http.Header.Get` is (the http crate already canonicalizes on insert,
/// but raw maps from tests may not be).
pub(crate) fn header_get<'a>(headers: &'a http::HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

/// Shared 15s-timeout client (Go `httpClient = &http.Client{Timeout: 15s}`).
pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("static client configuration")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::for_kind;

    #[test]
    fn kind_valid_matches_go_set() {
        assert!(Kind::FORGEJO.valid());
        assert!(Kind::GITEA.valid());
        assert!(Kind::GITLAB.valid());
        assert!(!Kind("github").valid());
        assert!(!Kind("").valid());
    }

    #[test]
    fn registry_returns_provider_per_kind() {
        assert!(for_kind("forgejo").is_some());
        assert!(for_kind("gitea").is_some());
        assert!(for_kind("gitlab").is_some());
        assert!(for_kind("github").is_none());
        assert_eq!(for_kind("forgejo").unwrap().kind(), Kind::FORGEJO);
        assert_eq!(for_kind("gitea").unwrap().kind(), Kind::GITEA);
    }

    #[test]
    fn pr_terminal_matches_go_action_set() {
        let mut e = PullRequestEvent::default();
        for action in ["closed", "merged", "merge", "close"] {
            e.action = action.to_string();
            assert!(e.terminal(), "{action} should be terminal");
        }
        for action in ["opened", "synchronize", "reopened", ""] {
            e.action = action.to_string();
            assert!(!e.terminal(), "{action} should not be terminal");
        }
    }

    #[test]
    fn derive_pr_state_priority() {
        assert_eq!(derive_pr_state("open", false, true), "merged");
        assert_eq!(derive_pr_state("closed", false, false), "closed");
        assert_eq!(derive_pr_state("open", true, false), "draft");
        assert_eq!(derive_pr_state("open", false, false), "open");
    }

    #[test]
    fn normalize_instance_url_trims() {
        assert_eq!(
            normalize_instance_url(" https://gitea.example.com/ "),
            "https://gitea.example.com"
        );
        assert_eq!(
            normalize_instance_url("https://gitlab.example.com"),
            "https://gitlab.example.com"
        );
        assert_eq!(
            normalize_instance_url("https://x.test///"),
            "https://x.test"
        );
    }
}
