//! Forgejo/Gitea provider — wire-identical upstream and fork: same
//! /api/v1 REST surface, same X-Gitea-Signature HMAC-SHA256 webhooks, same
//! pull_request / status event shapes.
//!
//! Port of `server/internal/integrations/vcs/forgejo.go`. One struct serves
//! both kinds; it registers under Kind::FORGEJO and Kind::GITEA so the only
//! user-visible difference is the provider label.

use anyhow::{anyhow, Result};
use hmac::Mac as _;
use serde::Deserialize;

use crate::vcs::{
    coalesce, derive_pr_state, header_get, http_client, normalize_instance_url, Account,
    CiStatusEvent, EventKind, Kind, Provider, ProviderError, PullRequestEvent, UnauthorizedError,
};

/// Implements [`Provider`] for Forgejo and Gitea.
#[derive(Debug, Clone)]
pub struct ForgejoProvider {
    kind: Kind,
}

impl ForgejoProvider {
    pub fn new(kind: Kind) -> Self {
        Self { kind }
    }
}

#[async_trait::async_trait]
impl Provider for ForgejoProvider {
    fn kind(&self) -> Kind {
        self.kind
    }

    fn event_kind(&self, headers: &http::HeaderMap) -> EventKind {
        let mut event = header_get(headers, "X-Gitea-Event");
        if event.is_empty() {
            // Gitea mirrors this header too.
            event = header_get(headers, "X-GitHub-Event");
        }
        match event {
            "pull_request" => EventKind::PullRequest,
            "status" => EventKind::CiStatus,
            _ => EventKind::Other,
        }
    }

    /// Checks X-Gitea-Signature, a bare hex HMAC-SHA256 of the body (no
    /// "sha256=" prefix — that is GitHub's convention; tolerate it anyway).
    fn verify_signature(&self, secret: &str, headers: &http::HeaderMap, body: &[u8]) -> bool {
        // HMAC with an empty key is forgeable, so reject an empty secret
        // outright (mirrors the GitLab verifier). Not reachable today — the
        // secret is always 32 random bytes — but keep the auth boundary
        // safe regardless.
        if secret.is_empty() {
            return false;
        }
        let sig = header_get(headers, "X-Gitea-Signature").trim();
        let sig = sig.strip_prefix("sha256=").unwrap_or(sig);
        let Ok(want) = hex::decode(sig) else {
            return false;
        };
        let mut mac = <hmac::Hmac<sha2::Sha256> as hmac::Mac>::new_from_slice(secret.as_bytes())
            .expect("hmac accepts any key length");
        mac.update(body);
        mac.finalize().into_bytes().as_slice() == want.as_slice()
    }

    fn parse_pull_request(&self, body: &[u8]) -> Result<PullRequestEvent> {
        let d: ForgejoPullRequestPayload =
            serde_json::from_slice(body).map_err(|e| anyhow!(e.to_string()))?;
        let mut owner = coalesce(&d.pull_request.user.username, &d.pull_request.user.login);
        // Note: Go coalesces repository owner first; mirror its exact order.
        let repo_owner = {
            let candidate = coalesce(&d.repository.owner.username, &d.repository.owner.login);
            if !candidate.is_empty() {
                candidate
            } else {
                owner.clear();
                if let Some(i) = d.repository.full_name.find('/') {
                    if i > 0 {
                        d.repository.full_name[..i].to_string()
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            }
        };
        let _ = &mut owner;
        Ok(PullRequestEvent {
            action: d.action,
            repo_owner,
            repo_name: d.repository.name,
            number: d.pull_request.number,
            title: d.pull_request.title,
            body: d.pull_request.body,
            state: derive_pr_state(
                &d.pull_request.state,
                d.pull_request.draft,
                d.pull_request.merged,
            ),
            html_url: d.pull_request.html_url,
            branch: d.pull_request.head.ref_,
            head_sha: d.pull_request.head.sha,
            author_login: coalesce(&d.pull_request.user.username, &d.pull_request.user.login),
            author_avatar_url: d.pull_request.user.avatar_url,
            additions: d.pull_request.additions,
            deletions: d.pull_request.deletions,
            changed_files: d.pull_request.changed_files,
            merged_at: d.pull_request.merged_at,
            closed_at: d.pull_request.closed_at,
            created_at: d.pull_request.created_at,
            updated_at: d.pull_request.updated_at,
        })
    }

    fn parse_ci_status(&self, body: &[u8]) -> Result<CiStatusEvent> {
        let d: ForgejoStatusPayload =
            serde_json::from_slice(body).map_err(|e| anyhow!(e.to_string()))?;
        // Prefer the status' own updated_at (RFC3339) so the monotonic
        // guard is real; fall back to created_at, then empty (handler uses
        // ingestion time).
        let updated_at = if d.updated_at.is_empty() {
            d.created_at
        } else {
            d.updated_at
        };
        Ok(CiStatusEvent {
            sha: d.sha,
            context: d.context,
            state: normalize_forgejo_state(&d.state),
            target_url: d.target_url,
            description: d.description,
            updated_at,
        })
    }

    async fn validate_token(&self, instance_url: &str, token: &str) -> Result<Account> {
        let endpoint = format!("{}/api/v1/user", normalize_instance_url(instance_url));
        let response = http_client()
            .get(&endpoint)
            .header("Authorization", format!("token {token}"))
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| anyhow!("forgejo: request: {e}"))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            // Log the upstream status + body snippet so a bad token (401)
            // is distinguishable from an insufficient-scope token (403)
            // without leaking the secret into the HTTP response.
            let body_text = response.text().await.unwrap_or_default();
            let snippet: String = body_text.chars().take(512).collect();
            tracing::warn!(
                endpoint = %endpoint,
                status = status.as_u16(),
                body = %snippet.trim(),
                "forgejo: token validation rejected"
            );
            return Err(ProviderError::from(UnauthorizedError).into());
        }
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            let snippet: String = body_text.chars().take(512).collect();
            return Err(anyhow!(
                "forgejo: GET /user: status {}: {}",
                status.as_u16(),
                snippet.trim()
            ));
        }
        let u: ForgejoUser = response
            .json()
            .await
            .map_err(|e| anyhow!("forgejo: decode user: {e}"))?;
        let login = coalesce(&u.login, &u.username);
        if login.is_empty() {
            return Err(anyhow!("forgejo: user response missing login"));
        }
        Ok(Account { login })
    }
}

#[derive(Debug, Deserialize)]
struct ForgejoPullRequestPayload {
    #[serde(default)]
    action: String,
    #[serde(default)]
    pull_request: ForgejoPullRequest,
    #[serde(default)]
    repository: ForgejoRepository,
}

#[derive(Debug, Deserialize, Default)]
struct ForgejoPullRequest {
    #[serde(default)]
    number: i32,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    merged: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default, rename = "html_url")]
    html_url: String,
    #[serde(default)]
    additions: i32,
    #[serde(default)]
    deletions: i32,
    #[serde(default, rename = "changed_files")]
    changed_files: i32,
    #[serde(default, rename = "merged_at")]
    merged_at: String,
    #[serde(default, rename = "closed_at")]
    closed_at: String,
    #[serde(default, rename = "created_at")]
    created_at: String,
    #[serde(default, rename = "updated_at")]
    updated_at: String,
    #[serde(default)]
    user: ForgejoUserIdentity,
    #[serde(default)]
    head: ForgejoHead,
}

#[derive(Debug, Deserialize, Default)]
struct ForgejoRepository {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "full_name")]
    full_name: String,
    #[serde(default)]
    owner: ForgejoUserIdentity,
}

#[derive(Debug, Deserialize, Default)]
struct ForgejoUserIdentity {
    #[serde(default)]
    login: String,
    #[serde(default)]
    username: String,
    #[serde(default, rename = "avatar_url")]
    avatar_url: String,
}

#[derive(Debug, Deserialize, Default)]
struct ForgejoHead {
    #[serde(default, rename = "ref")]
    ref_: String,
    #[serde(default)]
    sha: String,
}

#[derive(Debug, Deserialize)]
struct ForgejoStatusPayload {
    #[serde(default)]
    sha: String,
    #[serde(default)]
    context: String,
    #[serde(default)]
    state: String,
    #[serde(default, rename = "target_url")]
    target_url: String,
    #[serde(default)]
    description: String,
    /// Forgejo/Gitea send these as RFC3339 on the commit-status object.
    #[serde(default, rename = "created_at")]
    created_at: String,
    #[serde(default, rename = "updated_at")]
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct ForgejoUser {
    #[serde(default)]
    login: String,
    #[serde(default)]
    username: String,
}

/// Maps Forgejo/Gitea commit-status states onto the shared
/// passed/failed/pending vocabulary. "warning" is treated as a pass (it
/// does not block), mirroring how GitHub's neutral/skipped count as passed.
pub(crate) fn normalize_forgejo_state(s: &str) -> String {
    match s {
        "success" | "warning" => "passed".to_string(),
        "failure" | "error" => "failed".to_string(),
        // pending, and anything unknown
        _ => "pending".to_string(),
    }
}

// Re-exported so the crate root can re-export the digest helper for tests
// without pulling sha2 into every consumer.
#[allow(unused_imports)]
use sha2::Digest as _Sha256DigestMarker;

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.insert(
                http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        map
    }

    #[test]
    fn event_kind_classifies_gitea_and_github_headers() {
        let p = ForgejoProvider::new(Kind::FORGEJO);
        assert_eq!(
            p.event_kind(&headers(&[("X-Gitea-Event", "pull_request")])),
            EventKind::PullRequest
        );
        assert_eq!(
            p.event_kind(&headers(&[("X-Gitea-Event", "status")])),
            EventKind::CiStatus
        );
        assert_eq!(
            p.event_kind(&headers(&[("X-Gitea-Event", "push")])),
            EventKind::Other
        );
        // Falls back to the mirrored GitHub header.
        assert_eq!(
            p.event_kind(&headers(&[("X-GitHub-Event", "pull_request")])),
            EventKind::PullRequest
        );
        assert_eq!(p.event_kind(&headers(&[])), EventKind::Other);
    }

    #[test]
    fn verify_signature_accepts_bare_and_prefixed_hex() {
        let p = ForgejoProvider::new(Kind::FORGEJO);
        let body = b"hello";
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(body);
        let sig = hex::encode(mac.finalize().into_bytes());
        assert!(p.verify_signature(
            "secret",
            &headers(&[("X-Gitea-Signature", sig.as_str())]),
            body
        ));
        assert!(p.verify_signature(
            "secret",
            &headers(&[("X-Gitea-Signature", format!("sha256={sig}").as_str())]),
            body
        ));
        assert!(!p.verify_signature(
            "secret",
            &headers(&[("X-Gitea-Signature", "deadbeef")]),
            body
        ));
        assert!(!p.verify_signature("secret", &headers(&[]), body));
        // Empty secret never validates.
        assert!(!p.verify_signature("", &headers(&[("X-Gitea-Signature", sig.as_str())]), body));
    }

    #[test]
    fn parse_pull_request_normalizes_state_and_owner() {
        let p = ForgejoProvider::new(Kind::GITEA);
        let body = br#"{
            "action": "opened",
            "pull_request": {
                "number": 7, "title": "T", "body": "B", "state": "open",
                "merged": false, "draft": true, "html_url": "https://x.test/1",
                "additions": 3, "deletions": 1, "changed_files": 2,
                "head": {"ref": "feature", "sha": "abc"},
                "user": {"username": "alice", "avatar_url": "https://a.png"}
            },
            "repository": {"name": "repo", "full_name": "org/repo", "owner": {"username": "org"}}
        }"#;
        let e = p.parse_pull_request(body).unwrap();
        assert_eq!(e.repo_owner, "org");
        assert_eq!(e.repo_name, "repo");
        assert_eq!(e.number, 7);
        assert_eq!(e.state, "draft");
        assert_eq!(e.branch, "feature");
        assert_eq!(e.head_sha, "abc");
        assert_eq!(e.author_login, "alice");
        assert_eq!(e.additions, 3);
        assert_eq!(e.changed_files, 2);
        assert!(!e.terminal());
    }

    #[test]
    fn parse_pull_request_owner_falls_back_to_full_name() {
        let p = ForgejoProvider::new(Kind::FORGEJO);
        let body = br#"{
            "action": "closed",
            "pull_request": {"number": 1, "state": "closed", "merged": true},
            "repository": {"name": "repo", "full_name": "group/sub/repo"}
        }"#;
        let e = p.parse_pull_request(body).unwrap();
        assert_eq!(e.repo_owner, "group");
        assert_eq!(e.state, "merged");
        assert!(e.terminal());
    }

    #[test]
    fn parse_ci_status_prefers_updated_at_and_normalizes() {
        let p = ForgejoProvider::new(Kind::FORGEJO);
        let e = p
            .parse_ci_status(
                br#"{"sha":"s","context":"ci","state":"success","target_url":"https://t","description":"ok"}"#,
            )
            .unwrap();
        assert_eq!(e.state, "passed");
        assert_eq!(e.updated_at, "");

        let e = p
            .parse_ci_status(
                br#"{"sha":"s","state":"warning","created_at":"2026-01-01T00:00:00Z"}"#,
            )
            .unwrap();
        assert_eq!(e.state, "passed");
        assert_eq!(e.updated_at, "2026-01-01T00:00:00Z");

        let e = p.parse_ci_status(br#"{"state":"failure"}"#).unwrap();
        assert_eq!(e.state, "failed");

        let e = p.parse_ci_status(br#"{"state":"whatever"}"#).unwrap();
        assert_eq!(e.state, "pending");
    }

    #[tokio::test]
    #[ignore = "requires network"]
    async fn validate_token_roundtrip() {
        let p = ForgejoProvider::new(Kind::FORGEJO);
        let _ = p
            .validate_token("https://codeberg.org", "token")
            .await
            .unwrap();
    }
}
