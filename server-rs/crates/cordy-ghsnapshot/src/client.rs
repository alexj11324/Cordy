//! GitHub App-authenticated API client for the PR snapshot pipeline.
//!
//! Layers:
//! App JWT → installation access token (cached per installation, renewed
//! early, concurrent mints collapsed) → GraphQL calls.
//!
//! Credential hygiene: the App private key and every installation token are
//! treated as opaque secrets and are NEVER written to a log or embedded in
//! an error message.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Result};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Deserialize;
use thiserror::Error;

pub const DEFAULT_API_BASE: &str = "https://api.github.com";
const USER_AGENT: &str = "Cordy-GitHub-Snapshot/1.0";

/// Renew an installation token this long before it actually expires so an
/// in-flight request never races the expiry boundary. GitHub tokens live
/// one hour.
const TOKEN_RENEW_SKEW: Duration = Duration::from_secs(5 * 60);

/// Signals that GitHub asked us to back off. `retry_after` is how long to
/// wait before retrying; it is derived from the Retry-After header, or the
/// X-RateLimit-Reset header, or a conservative default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("github rate limited, retry after {retry_after:?}")]
pub struct RateLimitError {
    pub retry_after: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestMetadata {
    pub title: String,
    pub state: String,
    pub html_url: String,
    pub branch: String,
    pub head_sha: String,
    pub author_login: String,
    pub author_avatar_url: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub merged_at: Option<chrono::DateTime<chrono::Utc>>,
    pub closed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub additions: i32,
    pub deletions: i32,
    pub changed_files: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InstallationAccount {
    pub login: String,
    #[serde(rename = "type")]
    pub account_type: String,
    pub avatar_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct InstallationRepository {
    pub id: i64,
    pub full_name: String,
    pub html_url: String,
    pub clone_url: String,
    pub description: Option<String>,
    pub private: bool,
    pub archived: bool,
    pub default_branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct InstallationRepositories {
    pub repositories: Vec<InstallationRepository>,
    pub total_count: i64,
    pub next_page: Option<i32>,
}

struct CachedToken {
    token: String,
    expiry: SystemTime,
}

/// An installation-token-authenticated GitHub API client.
///
/// Go models "feature disabled" with a nil `*Client`; Rust uses
/// [`Client::enabled()`] returning false when no private key was provided,
/// and every caller tolerates a disabled client the same way.
pub struct Client {
    app_id: String,
    private_key: EncodingKey,
    /// Validation-side mirror of the encoding key; built once so JWT
    /// verification (tests) does not re-parse the PEM per call.
    api_base: String,
    http: reqwest::Client,
    now: Box<dyn Fn() -> SystemTime + Send + Sync>,

    tokens: Mutex<HashMap<i64, CachedToken>>,
    mint_locks: Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
}

impl Client {
    /// Builds a client from GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY.
    ///
    /// - Both unset → `Ok(None)`: the App API is simply not configured;
    ///   the caller degrades the whole feature off.
    /// - Key present but malformed → `Err`: operator-actionable, surface
    ///   it. The key material is deliberately not included in the error.
    pub fn new_from_env() -> Result<Option<Self>> {
        let app_id = std::env::var("GITHUB_APP_ID")
            .unwrap_or_default()
            .trim()
            .to_string();
        let pem_key = std::env::var("GITHUB_APP_PRIVATE_KEY")
            .unwrap_or_default()
            .trim()
            .to_string();
        if app_id.is_empty() || pem_key.is_empty() {
            return Ok(None);
        }
        let private_key = EncodingKey::from_rsa_pem(pem_key.as_bytes())
            .map_err(|e| anyhow!("parse GITHUB_APP_PRIVATE_KEY: {e}"))?;
        Ok(Some(Self {
            app_id,
            private_key,
            api_base: DEFAULT_API_BASE.to_string(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent(USER_AGENT)
                .build()
                .map_err(|e| anyhow!("build github http client: {e}"))?,
            now: Box::new(SystemTime::now),
            tokens: Mutex::new(HashMap::new()),
            mint_locks: Mutex::new(HashMap::new()),
        }))
    }

    /// Test/ops constructor with an explicit PEM key and clock.
    pub fn new(
        app_id: String,
        pem_key: &[u8],
        api_base: String,
        now: Box<dyn Fn() -> SystemTime + Send + Sync>,
    ) -> Result<Self> {
        let private_key =
            EncodingKey::from_rsa_pem(pem_key).map_err(|e| anyhow!("parse private key: {e}"))?;
        Self::with_encoding_key(app_id, private_key, api_base, now)
    }

    /// Test/ops constructor taking an already-parsed key.
    pub fn with_encoding_key(
        app_id: String,
        private_key: EncodingKey,
        api_base: String,
        now: Box<dyn Fn() -> SystemTime + Send + Sync>,
    ) -> Result<Self> {
        Ok(Self {
            app_id,
            private_key,
            api_base,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent(USER_AGENT)
                .build()
                .map_err(|e| anyhow!("build github http client: {e}"))?,
            now,
            tokens: Mutex::new(HashMap::new()),
            mint_locks: Mutex::new(HashMap::new()),
        })
    }

    /// Reports whether the App API is configured. A disabled client makes
    /// every method no-ops / errors at the call site's discretion.
    pub fn enabled(&self) -> bool {
        // private_key presence is implied by construction; the Option-style
        // nil-client semantics live with the Manager.
        true
    }

    /// Mints the short-lived RS256 JWT GitHub requires for
    /// App-authenticated calls. iat is back-dated 60s to absorb clock skew
    /// and exp is capped at 9 minutes (GitHub's ceiling is 10).
    fn sign_app_jwt(&self, now: SystemTime) -> Result<String> {
        let unix = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| anyhow!("clock before epoch"))?
            .as_secs() as i64;
        let claims = serde_json::json!({
            "iat": unix - 60,
            "exp": unix + 9 * 60,
            "iss": self.app_id,
        });
        jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &self.private_key)
            .map_err(|_| anyhow!("sign App JWT failed"))
    }

    fn cached_token(&self, installation_id: i64) -> Option<String> {
        let now = (self.now)();
        let tokens = self.tokens.lock().unwrap();
        if let Some(t) = tokens.get(&installation_id) {
            if now + TOKEN_RENEW_SKEW < t.expiry {
                return Some(t.token.clone());
            }
        }
        None
    }

    /// Returns a cached installation access token, minting a new one via
    /// POST /app/installations/{id}/access_tokens when the cache is empty
    /// or within the renew skew of expiry.
    async fn installation_token(&self, installation_id: i64) -> Result<String> {
        if let Some(tok) = self.cached_token(installation_id) {
            return Ok(tok);
        }
        let mint_lock = {
            let mut locks = self.mint_locks.lock().unwrap();
            locks
                .entry(installation_id)
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _guard = mint_lock.lock().await;
        if let Some(tok) = self.cached_token(installation_id) {
            return Ok(tok);
        }
        self.mint_installation_token(installation_id).await
    }

    /// Reads the display identity for setup callbacks. Failures never include
    /// response bodies or credentials.
    pub async fn installation_account(&self, installation_id: i64) -> Result<InstallationAccount> {
        #[derive(Deserialize)]
        struct Envelope {
            account: InstallationAccount,
        }
        let jwt = self.sign_app_jwt((self.now)())?;
        let response = self
            .http
            .get(format!(
                "{}/app/installations/{installation_id}",
                self.api_base.trim_end_matches('/')
            ))
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {jwt}"))
            .send()
            .await?;
        if response.status() != reqwest::StatusCode::OK {
            anyhow::bail!(
                "github installation account: unexpected status {}",
                response.status().as_u16()
            );
        }
        Ok(response.json::<Envelope>().await?.account)
    }

    pub async fn installation_repositories(
        &self,
        installation_id: i64,
        page: i32,
        per_page: i32,
    ) -> Result<InstallationRepositories> {
        #[derive(Deserialize)]
        struct Envelope {
            total_count: i64,
            repositories: Vec<InstallationRepository>,
        }
        let token = self.installation_token(installation_id).await?;
        let response = self
            .http
            .get(format!(
                "{}/installation/repositories",
                self.api_base.trim_end_matches('/')
            ))
            .query(&[("page", page), ("per_page", per_page)])
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        if response.status() != reqwest::StatusCode::OK {
            anyhow::bail!(
                "github repositories: unexpected status {}",
                response.status().as_u16()
            );
        }
        let body = response.json::<Envelope>().await?;
        Ok(InstallationRepositories {
            next_page: (i64::from(page) * i64::from(per_page) < body.total_count)
                .then_some(page + 1),
            repositories: body.repositories,
            total_count: body.total_count,
        })
    }

    /// Lists installation repositories with a least-privilege, single-use
    /// metadata token. Unlike snapshot refresh tokens this credential is not
    /// cached and is revoked before returning, matching the settings browse
    /// endpoint's stricter credential lifecycle.
    pub async fn installation_repositories_once(
        &self,
        installation_id: i64,
        page: i32,
        per_page: i32,
    ) -> Result<InstallationRepositories> {
        #[derive(Deserialize)]
        struct Envelope {
            total_count: i64,
            repositories: Vec<InstallationRepository>,
        }
        let jwt = self.sign_app_jwt((self.now)())?;
        let token_response = self
            .http
            .post(format!(
                "{}/app/installations/{installation_id}/access_tokens",
                self.api_base.trim_end_matches('/')
            ))
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {jwt}"))
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&serde_json::json!({"permissions": {"metadata": "read"}}))
            .send()
            .await?;
        if token_response.status() != reqwest::StatusCode::CREATED {
            anyhow::bail!(
                "github installation token: unexpected status {}",
                token_response.status().as_u16()
            );
        }
        let token = token_response
            .json::<InstallationTokenResponse>()
            .await
            .map_err(|_| anyhow!("github installation token: malformed response"))?
            .token;
        if token.is_empty() {
            anyhow::bail!("github installation token: empty token");
        }
        let result = async {
            let response = self
                .http
                .get(format!(
                    "{}/installation/repositories",
                    self.api_base.trim_end_matches('/')
                ))
                .query(&[("page", page), ("per_page", per_page)])
                .header("Accept", "application/vnd.github+json")
                .header("Authorization", format!("Bearer {token}"))
                .header("X-GitHub-Api-Version", "2022-11-28")
                .send()
                .await?;
            if response.status() != reqwest::StatusCode::OK {
                anyhow::bail!(
                    "github repositories: unexpected status {}",
                    response.status().as_u16()
                );
            }
            let body = response.json::<Envelope>().await?;
            Ok(InstallationRepositories {
                next_page: (i64::from(page) * i64::from(per_page) < body.total_count)
                    .then_some(page + 1),
                repositories: body.repositories,
                total_count: body.total_count,
            })
        }
        .await;
        let _ = self
            .http
            .delete(format!(
                "{}/installation/token",
                self.api_base.trim_end_matches('/')
            ))
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {token}"))
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await;
        result
    }

    /// Exchanges the App JWT for a fresh installation token and caches it.
    async fn mint_installation_token(&self, installation_id: i64) -> Result<String> {
        let now = (self.now)();
        let app_jwt = self.sign_app_jwt(now)?;
        let endpoint = format!(
            "{}/app/installations/{installation_id}/access_tokens",
            self.api_base.trim_end_matches('/')
        );
        let response = self
            .http
            .post(&endpoint)
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {app_jwt}"))
            .send()
            .await
            .map_err(|e| anyhow!("{e}"))?;
        let status = response.status();
        if status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        {
            return Err(anyhow!(rate_limit_from_response(&response, now)?));
        }
        if status != reqwest::StatusCode::CREATED && status != reqwest::StatusCode::OK {
            // Never echo the body — a token-mint failure body can contain
            // sensitive hints; the status code is enough to diagnose.
            return Err(anyhow!(
                "github installation token: unexpected status {}",
                status.as_u16()
            ));
        }
        let parsed: InstallationTokenResponse = response
            .json()
            .await
            .map_err(|_| anyhow!("github installation token: malformed response"))?;
        if parsed.token.is_empty() {
            return Err(anyhow!("github installation token: empty token"));
        }
        let mut expiry = now + Duration::from_secs(3600);
        if !parsed.expires_at.is_empty() {
            if let Ok(t) = chrono::DateTime::parse_from_rfc3339(&parsed.expires_at) {
                let unix = t.to_utc().timestamp();
                if unix > 0 {
                    expiry = SystemTime::UNIX_EPOCH + Duration::from_secs(unix as u64);
                }
            }
        }
        self.tokens.lock().unwrap().insert(
            installation_id,
            CachedToken {
                token: parsed.token.clone(),
                expiry,
            },
        );
        Ok(parsed.token)
    }

    /// Fetches the mirrored PR metadata used by explicit issue attachment.
    /// The caller intentionally treats failures as a metadata-less attach.
    pub async fn pull_request_metadata(
        &self,
        installation_id: i64,
        owner: &str,
        repo: &str,
        number: i32,
    ) -> Result<PullRequestMetadata> {
        let token = self.installation_token(installation_id).await?;
        let endpoint = format!(
            "{}/repos/{owner}/{repo}/pulls/{number}",
            self.api_base.trim_end_matches('/')
        );
        let response = self
            .http
            .get(endpoint)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|error| anyhow!("fetch pull request: {error}"))?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(anyhow!(
                "fetch pull request: github status {}",
                response.status().as_u16()
            ));
        }
        let body: PullRequestBody = response
            .json()
            .await
            .map_err(|_| anyhow!("decode pull request: malformed response"))?;
        metadata_from_body(body)
    }

    /// Runs a single GraphQL query as the given installation and returns
    /// the raw `data` object. GitHub returns HTTP 200 even for query-level
    /// errors, so we inspect the `errors` array too, mapping a RATE_LIMITED
    /// error type to a [`RateLimitError`].
    pub async fn graph_ql(
        &self,
        installation_id: i64,
        query: &str,
        variables: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let token = self.installation_token(installation_id).await?;
        let payload = serde_json::json!({"query": query, "variables": variables});
        let endpoint = format!("{}/graphql", self.api_base.trim_end_matches('/'));
        let response = self
            .http
            .post(&endpoint)
            .header("Accept", "application/vnd.github+json")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {token}"))
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow!("{e}"))?;
        let status = response.status();
        if status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        {
            return Err(anyhow!(rate_limit_from_response(&response, (self.now)())?));
        }
        if status != reqwest::StatusCode::OK {
            return Err(anyhow!(
                "github graphql: unexpected status {}",
                status.as_u16()
            ));
        }
        let body: GraphQLResponse = response
            .json()
            .await
            .map_err(|_| anyhow!("github graphql: malformed response"))?;
        if !body.errors.is_empty() {
            for e in &body.errors {
                if e.r#type == "RATE_LIMITED" {
                    return Err(anyhow!(RateLimitError {
                        retry_after: Duration::from_secs(60),
                    }));
                }
            }
            // Surface the message but nothing else; GraphQL error messages
            // do not contain credentials.
            return Err(anyhow!("github graphql error: {}", body.errors[0].message));
        }
        let Some(data) = body.data else {
            return Err(anyhow!("github graphql: empty data"));
        };
        Ok(data)
    }
}

fn metadata_from_body(body: PullRequestBody) -> Result<PullRequestMetadata> {
    let created_at = parse_time(&body.created_at, "created_at")?;
    let updated_at = parse_time(&body.updated_at, "updated_at")?;
    let state = if body.merged {
        "merged"
    } else if body.draft {
        "draft"
    } else if body.state.eq_ignore_ascii_case("closed") {
        "closed"
    } else {
        "open"
    };
    Ok(PullRequestMetadata {
        title: body.title,
        state: state.into(),
        html_url: body.html_url,
        branch: body.head.ref_name,
        head_sha: body.head.sha,
        author_login: body.user.login,
        author_avatar_url: body.user.avatar_url,
        created_at,
        updated_at,
        merged_at: parse_optional_time(&body.merged_at),
        closed_at: parse_optional_time(&body.closed_at),
        additions: body.additions,
        deletions: body.deletions,
        changed_files: body.changed_files,
    })
}

fn parse_time(raw: &str, name: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&chrono::Utc))
        .map_err(|_| anyhow!("decode pull request: invalid {name}"))
}

fn parse_optional_time(raw: &Option<String>) -> Option<chrono::DateTime<chrono::Utc>> {
    raw.as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&chrono::Utc))
}

#[derive(Debug, Deserialize)]
struct PullRequestBody {
    #[serde(default)]
    title: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    merged: bool,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    closed_at: Option<String>,
    #[serde(default)]
    head: PullRequestHead,
    #[serde(default)]
    user: PullRequestUser,
    #[serde(default)]
    additions: i32,
    #[serde(default)]
    deletions: i32,
    #[serde(default)]
    changed_files: i32,
}

#[derive(Debug, Default, Deserialize)]
struct PullRequestHead {
    #[serde(default, rename = "ref")]
    ref_name: String,
    #[serde(default)]
    sha: String,
}

#[derive(Debug, Default, Deserialize)]
struct PullRequestUser {
    #[serde(default)]
    login: String,
    #[serde(default)]
    avatar_url: String,
}

#[derive(Debug, Deserialize)]
struct InstallationTokenResponse {
    #[serde(default)]
    token: String,
    #[serde(default, rename = "expires_at")]
    expires_at: String,
}

#[derive(Debug, Deserialize)]
struct GraphQLErrorEntry {
    #[serde(default, rename = "type")]
    r#type: String,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct GraphQLResponse {
    #[serde(default)]
    data: Option<serde_json::Value>,
    #[serde(default)]
    errors: Vec<GraphQLErrorEntry>,
}

/// Builds a [`RateLimitError`] from GitHub's throttling headers.
/// Retry-After (seconds) wins; then X-RateLimit-Reset (unix seconds);
/// otherwise a conservative 60s. The wait is clamped to [1s, 5m].
fn rate_limit_from_response(
    response: &reqwest::Response,
    now: SystemTime,
) -> Result<RateLimitError> {
    let mut wait = Duration::from_secs(60);
    if let Some(v) = header(response, "Retry-After") {
        if let Ok(secs) = v.trim().parse::<u64>() {
            wait = Duration::from_secs(secs);
        }
    } else if let Some(v) = header(response, "X-RateLimit-Reset") {
        if let Ok(unix) = v.trim().parse::<i64>() {
            let reset = SystemTime::UNIX_EPOCH + Duration::from_secs(unix.max(0) as u64);
            if let Ok(d) = reset.duration_since(now) {
                wait = d;
            }
        }
    }
    if wait < Duration::from_secs(1) {
        wait = Duration::from_secs(1);
    }
    if wait > Duration::from_secs(5 * 60) {
        wait = Duration::from_secs(5 * 60);
    }
    Ok(RateLimitError { retry_after: wait })
}

fn header<'a>(response: &'a reqwest::Response, name: &str) -> Option<&'a str> {
    response.headers().get(name)?.to_str().ok()
}

#[cfg(test)]
pub(crate) fn test_encoding_key() -> EncodingKey {
    // Public test-only fixture copied from ring's RSA test suite. It is not a
    // credential and must never be used outside tests.
    let pem = format!(
        "-----BEGIN PRIVATE KEY-----\n{}-----END PRIVATE KEY-----\n",
        include_str!("../../../testdata/rsa_test_private_key_2048.pk8.b64")
    );
    EncodingKey::from_rsa_pem(pem.as_bytes()).expect("parse test RSA key")
}

// JWT claim-shape test: verifies the signed token carries iat/exp/iss the
// way Go's jwt.MapClaims did.
#[cfg(test)]
mod jwt_tests {
    use super::*;

    fn test_client(now: SystemTime) -> Client {
        Client::with_encoding_key(
            "42".to_string(),
            test_encoding_key(),
            DEFAULT_API_BASE.to_string(),
            Box::new(move || now),
        )
        .expect("client construction")
    }

    #[test]
    fn sign_app_jwt_carries_backdated_iat_and_iss() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let client = test_client(now);
        let token = client.sign_app_jwt(now).unwrap();
        let header = jsonwebtoken::decode_header(&token).unwrap();
        assert_eq!(header.alg, Algorithm::RS256);
        // Split-and-decode the payload segment: claim shape is what matters
        // here, not signature verification.
        use base64::Engine as _;
        let segments: Vec<&str> = token.split('.').collect();
        assert_eq!(segments.len(), 3);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(segments[1])
            .unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(claims["iss"], "42");
        let iat = claims["iat"].as_i64().unwrap();
        let exp = claims["exp"].as_i64().unwrap();
        let unix = 1_800_000_000i64;
        assert_eq!(iat, unix - 60, "iat back-dated 60s for clock skew");
        assert_eq!(exp, unix + 9 * 60, "exp capped at 9 minutes");
    }

    #[test]
    fn cached_token_respects_renew_skew() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let client = test_client(now);
        // Seed a token that expires in exactly the skew window — it must be
        // treated as stale so a renew happens before the real expiry.
        client.tokens.lock().unwrap().insert(
            7,
            CachedToken {
                token: "t".into(),
                expiry: now + TOKEN_RENEW_SKEW,
            },
        );
        assert!(client.cached_token(7).is_none(), "at-skew token is stale");
        client.tokens.lock().unwrap().insert(
            8,
            CachedToken {
                token: "t".into(),
                expiry: now + TOKEN_RENEW_SKEW + Duration::from_secs(60),
            },
        );
        assert_eq!(
            client.cached_token(8).as_deref(),
            Some("t"),
            "outside the skew is fresh"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_request_metadata_uses_github_state_priority_and_timestamps() {
        let body: PullRequestBody = serde_json::from_value(serde_json::json!({
            "title": "Port attach route",
            "state": "closed",
            "draft": true,
            "merged": true,
            "html_url": "https://github.com/o/r/pull/24",
            "created_at": "2026-08-22T10:20:30Z",
            "updated_at": "2026-08-23T11:21:31Z",
            "merged_at": "2026-08-23T11:21:30Z",
            "closed_at": "not-a-time",
            "head": {"ref": "codex/attach", "sha": "abc123"},
            "user": {"login": "alex", "avatar_url": "https://avatars.example/alex"},
            "additions": 10,
            "deletions": 2,
            "changed_files": 3
        }))
        .unwrap();
        let metadata = metadata_from_body(body).unwrap();
        assert_eq!(metadata.state, "merged");
        assert_eq!(metadata.branch, "codex/attach");
        assert_eq!(metadata.head_sha, "abc123");
        assert_eq!(
            metadata.created_at.to_rfc3339(),
            "2026-08-22T10:20:30+00:00"
        );
        assert!(metadata.merged_at.is_some());
        assert!(metadata.closed_at.is_none());
    }

    #[test]
    fn disabled_when_env_missing() {
        // Not setting the env vars in the test process yields None rather
        // than an error (feature-off degradation).
        let result = std::env::var("GITHUB_APP_ID").is_err()
            && std::env::var("GITHUB_APP_PRIVATE_KEY").is_err();
        // Only assert when truly unset to avoid clobbering a real deployment.
        if result {
            assert!(Client::new_from_env().unwrap().is_none());
        }
    }

    #[tokio::test]
    #[ignore = "requires network + real App credentials"]
    async fn graphql_roundtrip() {
        let Some(client) = Client::new_from_env().unwrap() else {
            return;
        };
        let _ = client
            .graph_ql(1, "query{viewer{login}}", &serde_json::json!({}))
            .await
            .unwrap();
    }
}
