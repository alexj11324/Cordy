//! GitLab provider — differs from Forgejo/Gitea on every axis: /api/v4
//! with a PRIVATE-TOKEN header, webhooks authenticated by a plaintext
//! X-Gitlab-Token compare (no HMAC), an X-Gitlab-Event header,
//! "merge request" terminology, and pipeline events for CI.
//!
//! Port of `server/internal/integrations/vcs/gitlab.go`. The normalized
//! [`PullRequestEvent`](super::PullRequestEvent) /
//! [`CiStatusEvent`](super::CiStatusEvent) hide all of that from the
//! handler.

use anyhow::{anyhow, Result};
use serde::Deserialize;

use crate::vcs::{
    header_get, http_client, normalize_instance_url, Account, CiStatusEvent, EventKind, Kind,
    Provider, ProviderError, PullRequestEvent, UnauthorizedError,
};

/// Implements [`Provider`] for GitLab.
#[derive(Debug, Clone, Copy, Default)]
pub struct GitlabProvider;

#[async_trait::async_trait]
impl Provider for GitlabProvider {
    fn kind(&self) -> Kind {
        Kind::GITLAB
    }

    fn event_kind(&self, headers: &http::HeaderMap) -> EventKind {
        match header_get(headers, "X-Gitlab-Event") {
            "Merge Request Hook" => EventKind::PullRequest,
            "Pipeline Hook" => EventKind::CiStatus,
            _ => EventKind::Other,
        }
    }

    /// Compares the X-Gitlab-Token header to the stored secret in constant
    /// time. GitLab does not HMAC-sign webhook bodies; the shared token is
    /// the whole authentication, so an empty stored secret never validates.
    fn verify_signature(&self, secret: &str, headers: &http::HeaderMap, _body: &[u8]) -> bool {
        if secret.is_empty() {
            return false;
        }
        let got = header_get(headers, "X-Gitlab-Token");
        // subtle::ConstantTimeCompare equivalent via ct-codecs-style fixed
        // comparison on equal-length slices; Go's ConstantTimeCompare
        // returns 0 immediately for length mismatches.
        constant_time_eq(got.as_bytes(), secret.as_bytes())
    }

    fn parse_pull_request(&self, body: &[u8]) -> Result<PullRequestEvent> {
        let d: GlMergeRequestPayload =
            serde_json::from_slice(body).map_err(|e| anyhow!(e.to_string()))?;
        let (owner, name) = split_namespace(&d.project.path_with_namespace);
        let draft = d.object_attributes.draft
            || d.object_attributes.work_in_progress
            || d.object_attributes
                .title
                .to_lowercase()
                .starts_with("draft:");
        Ok(PullRequestEvent {
            action: d.object_attributes.action,
            repo_owner: owner,
            repo_name: name,
            number: d.object_attributes.iid,
            title: d.object_attributes.title,
            body: d.object_attributes.description,
            state: normalize_gitlab_mr_state(&d.object_attributes.state, draft),
            html_url: d.object_attributes.url,
            branch: d.object_attributes.source_branch,
            head_sha: d.object_attributes.last_commit.id,
            author_login: d.user.username,
            author_avatar_url: d.user.avatar_url,
            created_at: normalize_gitlab_time(&d.object_attributes.created_at),
            updated_at: normalize_gitlab_time(&d.object_attributes.updated_at),
            ..Default::default()
        })
    }

    fn parse_ci_status(&self, body: &[u8]) -> Result<CiStatusEvent> {
        let d: GlPipelinePayload =
            serde_json::from_slice(body).map_err(|e| anyhow!(e.to_string()))?;
        // Prefer the pipeline's finished_at (the state transition we're
        // recording); fall back to created_at. Normalized to RFC3339 so the
        // commit-status monotonic guard has a real, comparable timestamp
        // instead of ingestion time.
        let raw_updated = if d.object_attributes.finished_at.is_empty() {
            &d.object_attributes.created_at
        } else {
            &d.object_attributes.finished_at
        };
        Ok(CiStatusEvent {
            sha: d.object_attributes.sha,
            // GitLab pipelines are modelled as one status per commit, not
            // per named check, so a stable synthetic context keys the
            // single status row. Known limitations of this simplification
            // (acceptable for the default branch-pipeline config):
            //   - Merge-train pipelines run on a synthetic merge commit
            //     whose SHA differs from the MR head, so the head_sha join
            //     won't match and the card shows no checks.
            //   - Multiple pipelines on one commit collapse into this
            //     single context; the last one to fire wins per commit.
            context: "gitlab/pipeline".to_string(),
            state: normalize_gitlab_pipeline_state(&d.object_attributes.status),
            target_url: d.object_attributes.url,
            description: String::new(),
            updated_at: normalize_gitlab_time(raw_updated),
        })
    }

    async fn validate_token(&self, instance_url: &str, token: &str) -> Result<Account> {
        let endpoint = format!("{}/api/v4/user", normalize_instance_url(instance_url));
        let response = http_client()
            .get(&endpoint)
            .header("PRIVATE-TOKEN", token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| anyhow!("gitlab: request: {e}"))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::from(UnauthorizedError).into());
        }
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            let snippet: String = body_text.chars().take(512).collect();
            return Err(anyhow!(
                "gitlab: GET /user: status {}: {}",
                status.as_u16(),
                snippet.trim()
            ));
        }
        let u: GlUser = response
            .json()
            .await
            .map_err(|e| anyhow!("gitlab: decode user: {e}"))?;
        if u.username.is_empty() {
            return Err(anyhow!("gitlab: user response missing username"));
        }
        Ok(Account { login: u.username })
    }
}

/// Constant-time byte equality matching Go's `subtle.ConstantTimeCompare`
/// semantics for the ==1 check (both empty strings compare equal).
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

#[derive(Debug, Deserialize)]
struct GlMergeRequestPayload {
    #[serde(default)]
    user: GlUserIdentity,
    #[serde(default)]
    project: GlProject,
    #[serde(default, rename = "object_attributes")]
    object_attributes: GlObjectAttributes,
}

#[derive(Debug, Deserialize, Default)]
struct GlUserIdentity {
    #[serde(default)]
    username: String,
    #[serde(default, rename = "avatar_url")]
    avatar_url: String,
}

#[derive(Debug, Deserialize, Default)]
struct GlProject {
    #[serde(default, rename = "path_with_namespace")]
    path_with_namespace: String,
}

#[derive(Debug, Deserialize, Default)]
struct GlObjectAttributes {
    #[serde(default)]
    iid: i32,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    /// opened|closed|merged|locked
    #[serde(default)]
    state: String,
    #[serde(default)]
    action: String,
    #[serde(default, rename = "source_branch")]
    source_branch: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default, rename = "work_in_progress")]
    work_in_progress: bool,
    #[serde(default, rename = "created_at")]
    created_at: String,
    #[serde(default, rename = "updated_at")]
    updated_at: String,
    #[serde(default, rename = "last_commit")]
    last_commit: GlLastCommit,
}

#[derive(Debug, Deserialize, Default)]
struct GlLastCommit {
    #[serde(default)]
    id: String,
}

#[derive(Debug, Deserialize)]
struct GlPipelinePayload {
    #[serde(default, rename = "object_attributes")]
    object_attributes: GlPipelineAttributes,
}

#[derive(Debug, Deserialize, Default)]
struct GlPipelineAttributes {
    #[serde(default)]
    sha: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    url: String,
    #[serde(default, rename = "created_at")]
    created_at: String,
    #[serde(default, rename = "finished_at")]
    finished_at: String,
}

#[derive(Debug, Deserialize)]
struct GlUser {
    #[serde(default)]
    username: String,
}

/// Converts GitLab's webhook timestamp format ("2017-09-20 08:31:45 UTC")
/// into the RFC3339 the rest of the pipeline expects. Without this every
/// GitLab event's timestamp failed to parse and was silently replaced with
/// ingestion time, defeating the PR-upsert and commit-status monotonic
/// guards. Output uses RFC3339Nano to preserve sub-second precision so two
/// events within the same wall-clock second still order correctly.
/// Unrecognized input returns "" so the handler falls back to ingestion
/// time.
pub(crate) fn normalize_gitlab_time(s: &str) -> String {
    use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
    if s.is_empty() {
        return String::new();
    }
    const FORMATS: &[&str] = &[
        "%Y-%m-%dT%H:%M:%S%.f%:z",  // RFC3339 incl. fractional
        "%Y-%m-%d %H:%M:%S %Z",     // 2017-09-20 08:31:45 UTC
        "%Y-%m-%d %H:%M:%S %:z",    // ... +00:00 form
        "%Y-%m-%d %H:%M:%S%.f %Z",  // fractional + named zone
        "%Y-%m-%d %H:%M:%S%.f %:z", // fractional + offset
    ];
    // RFC3339 paths first via DateTime parse (handles Z / offsets).
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return t
            .to_utc()
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    }
    for layout in [
        "%Y-%m-%d %H:%M:%S %z",
        "%Y-%m-%d %H:%M:%S %:z",
        "%Y-%m-%d %H:%M:%S%.f %z",
        "%Y-%m-%d %H:%M:%S%.f %:z",
    ] {
        if let Ok(t) = DateTime::parse_from_str(s, layout) {
            return t
                .to_utc()
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
        }
    }
    for layout in FORMATS.iter().skip(1) {
        // Named zones like "UTC" are not accepted by chrono's %Z parsing;
        // special-case the trailing-UTC shape before falling back.
        if let Some(stripped) = s.strip_suffix(" UTC") {
            if let Ok(naive) = NaiveDateTime::parse_from_str(
                stripped,
                layout.trim_end_matches(" %Z").trim_end_matches(" %:z"),
            ) {
                return Utc
                    .from_utc_datetime(&naive)
                    .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
            }
            if let Ok(naive) = NaiveDateTime::parse_from_str(stripped, "%Y-%m-%d %H:%M:%S") {
                return Utc
                    .from_utc_datetime(&naive)
                    .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
            }
        }
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, layout) {
            return Utc
                .from_utc_datetime(&naive)
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
        }
    }
    String::new()
}

/// Maps GitLab MR states onto open/closed/merged/draft. "locked" is a
/// transient open sub-state, so it reads as open.
pub(crate) fn normalize_gitlab_mr_state(state: &str, draft: bool) -> String {
    match state {
        "merged" => "merged".to_string(),
        "closed" => "closed".to_string(),
        // opened, locked
        _ => {
            if draft {
                "draft".to_string()
            } else {
                "open".to_string()
            }
        }
    }
}

/// Maps pipeline statuses onto passed/failed/pending. skipped is a pass
/// (nothing failed); canceled is a failure-class terminal, matching how
/// GitHub treats cancelled.
pub(crate) fn normalize_gitlab_pipeline_state(s: &str) -> String {
    match s {
        "success" | "skipped" => "passed".to_string(),
        "failed" | "canceled" => "failed".to_string(),
        // created, waiting_for_resource, preparing, pending, running,
        // manual, scheduled
        _ => "pending".to_string(),
    }
}

/// Splits a GitLab path_with_namespace ("group/subgroup/repo") into owner
/// ("group/subgroup") and repo name ("repo"). Subgroups are kept in the
/// owner so the identity stays unique.
pub(crate) fn split_namespace(path: &str) -> (String, String) {
    let path = path.trim_matches('/');
    match path.rfind('/') {
        Some(i) => (path[..i].to_string(), path[i + 1..].to_string()),
        None => (String::new(), path.to_string()),
    }
}

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
    fn event_kind_classifies_gitlab_hooks() {
        let p = GitlabProvider;
        assert_eq!(
            p.event_kind(&headers(&[("X-Gitlab-Event", "Merge Request Hook")])),
            EventKind::PullRequest
        );
        assert_eq!(
            p.event_kind(&headers(&[("X-Gitlab-Event", "Pipeline Hook")])),
            EventKind::CiStatus
        );
        assert_eq!(
            p.event_kind(&headers(&[("X-Gitlab-Event", "Push Hook")])),
            EventKind::Other
        );
    }

    #[test]
    fn verify_signature_is_constant_time_token_compare() {
        let p = GitlabProvider;
        assert!(p.verify_signature("tok", &headers(&[("X-Gitlab-Token", "tok")]), b"anything"));
        assert!(!p.verify_signature("tok", &headers(&[("X-Gitlab-Token", "other")]), b""));
        // Length mismatch fails closed.
        assert!(!p.verify_signature("tok", &headers(&[("X-Gitlab-Token", "")]), b""));
        // Empty stored secret never validates.
        assert!(!p.verify_signature("", &headers(&[("X-Gitlab-Token", "tok")]), b""));
    }

    #[test]
    fn parse_pull_request_normalizes_mr_shapes() {
        let p = GitlabProvider;
        let body = br#"{
            "user": {"username": "bob", "avatar_url": "https://a.png"},
            "project": {"path_with_namespace": "group/sub/repo"},
            "object_attributes": {
                "iid": 42, "title": "Fix", "description": "D",
                "state": "opened", "action": "merge",
                "source_branch": "feat", "url": "https://gl.test/42",
                "work_in_progress": true,
                "created_at": "2017-09-20 08:31:45 UTC",
                "updated_at": "2017-09-21 10:00:00 UTC",
                "last_commit": {"id": "cafe"}
            }
        }"#;
        let e = p.parse_pull_request(body).unwrap();
        assert_eq!(e.repo_owner, "group/sub");
        assert_eq!(e.repo_name, "repo");
        assert_eq!(e.number, 42);
        assert_eq!(e.state, "draft");
        assert!(e.terminal(), "merge action is terminal");
        assert_eq!(e.head_sha, "cafe");
        // GitLab's legacy timestamp format normalized to RFC3339.
        assert_eq!(e.created_at, "2017-09-20T08:31:45.000000000Z");
        assert_eq!(e.updated_at, "2017-09-21T10:00:00.000000000Z");
    }

    #[test]
    fn parse_pull_request_draft_prefix_counts() {
        let p = GitlabProvider;
        let e = p
            .parse_pull_request(
                br#"{
                "project": {"path_with_namespace": "r"},
                "object_attributes": {"iid": 1, "state": "opened", "title": "Draft: x"}
            }"#,
            )
            .unwrap();
        assert_eq!(e.state, "draft");
    }

    #[test]
    fn parse_ci_status_uses_finished_at_and_synthetic_context() {
        let p = GitlabProvider;
        let e = p
            .parse_ci_status(
                br#"{
                "object_attributes": {
                    "sha": "s", "status": "success", "url": "https://gl.test/p/1",
                    "finished_at": "2026-01-02 03:04:05 UTC"
                }
            }"#,
            )
            .unwrap();
        assert_eq!(e.context, "gitlab/pipeline");
        assert_eq!(e.state, "passed");
        assert_eq!(e.updated_at, "2026-01-02T03:04:05.000000000Z");

        // Falls back to created_at when finished_at absent.
        let e = p
            .parse_ci_status(br#"{"object_attributes": {"sha": "s", "status": "running", "created_at": "2026-01-02T03:04:05Z"}}"#)
            .unwrap();
        assert_eq!(e.state, "pending");
        assert_eq!(e.updated_at, "2026-01-02T03:04:05.000000000Z");

        // canceled is failure-class; skipped passes.
        let e = p
            .parse_ci_status(br#"{"object_attributes":{"status":"canceled"}}"#)
            .unwrap();
        assert_eq!(e.state, "failed");
        let e = p
            .parse_ci_status(br#"{"object_attributes":{"status":"skipped"}}"#)
            .unwrap();
        assert_eq!(e.state, "passed");
    }

    #[test]
    fn normalize_gitlab_time_handles_all_go_layouts() {
        assert_eq!(
            normalize_gitlab_time("2017-09-20 08:31:45 UTC"),
            "2017-09-20T08:31:45.000000000Z"
        );
        assert_eq!(
            normalize_gitlab_time("2017-09-20 08:31:45 +0000"),
            "2017-09-20T08:31:45.000000000Z"
        );
        assert_eq!(
            normalize_gitlab_time("2017-09-20 08:31:45 +0200"),
            "2017-09-20T06:31:45.000000000Z"
        );
        assert_eq!(
            normalize_gitlab_time("2017-09-20 08:31:45 +08:00"),
            "2017-09-20T00:31:45.000000000Z"
        );
        assert_eq!(
            normalize_gitlab_time("2017-09-20T08:31:45Z"),
            "2017-09-20T08:31:45.000000000Z"
        );
        assert_eq!(normalize_gitlab_time(""), "");
        assert_eq!(normalize_gitlab_time("garbage"), "");
    }

    #[test]
    fn split_namespace_keeps_subgroups_in_owner() {
        assert_eq!(
            split_namespace("group/subgroup/repo"),
            ("group/subgroup".into(), "repo".into())
        );
        assert_eq!(split_namespace("repo"), ("".into(), "repo".into()));
        assert_eq!(split_namespace("/repo/"), ("".into(), "repo".into()));
    }

    #[tokio::test]
    #[ignore = "requires network"]
    async fn validate_token_roundtrip() {
        let p = GitlabProvider;
        let _ = p
            .validate_token("https://gitlab.com", "token")
            .await
            .unwrap();
    }
}
