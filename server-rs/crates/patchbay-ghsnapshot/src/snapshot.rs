//! The single GraphQL query, contexts pagination, and normalization into a
//! flat per-check snapshot.
//!

use anyhow::{anyhow, Result};
use serde::Deserialize;

use crate::client::Client;

/// prSnapshotQuery is the single GraphQL query behind the whole feature. It
/// returns, in one round trip (Elon measured cost=1):
///   - headRefOid: the head the snapshot describes (pins the anti-stale
///     write);
///   - mergeable: MERGEABLE / CONFLICTING / UNKNOWN — answers "is there a
///     conflict" only;
///   - mergeStateStatus: CLEAN / DIRTY / BLOCKED / BEHIND / UNSTABLE /
///     ... — "Ready to merge" is derived ONLY from CLEAN;
///   - statusCheckRollup: the overall CI verdict plus every check/status
///     context.
///
/// $cursor paginates statusCheckRollup.contexts; the caller loops until
/// hasNextPage is false (never assume <100 contexts).
pub const PR_SNAPSHOT_QUERY: &str = r#"query($owner:String!,$repo:String!,$number:Int!,$cursor:String){
  repository(owner:$owner,name:$repo){
    pullRequest(number:$number){
      headRefOid
      mergeable
      mergeStateStatus
      commits(last:1){nodes{commit{
        statusCheckRollup{
          state
          contexts(first:100,after:$cursor){
            pageInfo{hasNextPage endCursor}
            nodes{
              __typename
              ... on CheckRun{name status conclusion detailsUrl}
              ... on StatusContext{context state targetUrl}
            }
          }
        }
      }}}
    }
  }
}"#;

/// One normalized check for a PR head. Both GraphQL CheckRun and
/// StatusContext contexts are flattened into this shape (see
/// [`normalize_node`]), so downstream storage and aggregation are uniform.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckContext {
    pub name: String,
    /// Normalized lifecycle: "queued", "in_progress", or "completed".
    pub status: String,
    /// Normalized result: "success", "failure", "neutral", "cancelled",
    /// "skipped", "timed_out", "action_required", "startup_failure",
    /// "stale", or "error"; empty while the check is still running.
    pub conclusion: String,
    pub details_url: String,
    pub is_status_context: bool,
}

/// The atomic unit written per fetch. It mirrors exactly what the API
/// returned — no incremental inference.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrSnapshot {
    pub head_sha: String,
    /// MERGEABLE / CONFLICTING / UNKNOWN (raw enum)
    pub mergeable: String,
    /// CLEAN / DIRTY / BLOCKED / BEHIND / UNSTABLE / ... (raw enum)
    pub merge_state_status: String,
    /// statusCheckRollup.state (SUCCESS/FAILURE/PENDING/ERROR/EXPECTED).
    /// Empty ONLY when has_checks is false.
    pub rollup_state: String,
    /// False when statusCheckRollup was null — GitHub reports "no checks
    /// have been created for this commit yet". This must NEVER be rendered
    /// as passed.
    pub has_checks: bool,
    pub contexts: Vec<CheckContext>,
}

impl PrSnapshot {
    /// Whether the snapshot has settled: no check is still running and
    /// mergeability is known. An undecided snapshot on an open PR drives
    /// the bounded chase-window re-fetch.
    pub fn decided(&self) -> bool {
        if self.mergeable == "UNKNOWN" || self.mergeable.is_empty() {
            return false;
        }
        if self.has_checks && matches!(self.rollup_state.as_str(), "PENDING" | "EXPECTED" | "") {
            return false;
        }
        self.contexts.iter().all(|c| c.status == "completed")
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphQLRollup {
    #[serde(default)]
    state: String,
    #[serde(default)]
    contexts: RollupContexts,
}

#[derive(Debug, Deserialize, Default)]
struct RollupContexts {
    #[serde(default, rename = "pageInfo")]
    page_info: PageInfo,
    #[serde(default)]
    nodes: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize, Default)]
struct PageInfo {
    #[serde(default, rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(default, rename = "endCursor")]
    end_cursor: String,
}

#[derive(Debug, Deserialize)]
struct GraphQLPullRequest {
    #[serde(default, rename = "headRefOid")]
    head_ref_oid: String,
    #[serde(default)]
    mergeable: String,
    #[serde(default, rename = "mergeStateStatus")]
    merge_state_status: String,
    #[serde(default)]
    commits: CommitsField,
}

#[derive(Debug, Deserialize, Default)]
struct CommitsField {
    #[serde(default)]
    nodes: Vec<CommitNode>,
}

#[derive(Debug, Deserialize)]
struct CommitNode {
    #[serde(default)]
    commit: CommitBody,
}

#[derive(Debug, Deserialize, Default)]
struct CommitBody {
    #[serde(default, rename = "statusCheckRollup")]
    status_check_rollup: Option<GraphQLRollup>,
}

#[derive(Debug, Deserialize)]
struct GraphQLPrData {
    #[serde(default)]
    repository: RepositoryField,
}

#[derive(Debug, Deserialize, Default)]
struct RepositoryField {
    #[serde(default, rename = "pullRequest")]
    pull_request: Option<GraphQLPullRequest>,
}

const MAX_SNAPSHOT_CONTEXT_PAGES: usize = 100;

/// Runs PR_SNAPSHOT_QUERY, paginating statusCheckRollup.contexts to
/// completion, and returns the normalized snapshot. A null rollup yields
/// has_checks=false with no contexts.
pub async fn fetch_pr_snapshot(
    client: &Client,
    installation_id: i64,
    owner: &str,
    repo: &str,
    number: i32,
) -> Result<PrSnapshot> {
    let mut snap = PrSnapshot::default();
    let mut cursor: Option<String> = None;
    // Guard against a pathological cursor loop; 100 pages = 10k contexts,
    // far beyond any real PR.
    for page in 0..MAX_SNAPSHOT_CONTEXT_PAGES {
        let variables = serde_json::json!({
            "owner": owner,
            "repo": repo,
            "number": number,
            "cursor": cursor,
        });
        let data = client
            .graph_ql(installation_id, PR_SNAPSHOT_QUERY, &variables)
            .await?;
        let parsed: GraphQLPrData = serde_json::from_value(data)
            .map_err(|_| anyhow!("ghsnapshot: malformed pull request data"))?;
        let Some(pr) = parsed.repository.pull_request else {
            return Err(anyhow!("ghsnapshot: pull request not found"));
        };
        if page == 0 {
            snap.head_sha = pr.head_ref_oid.clone();
            snap.mergeable = pr.mergeable.clone();
            snap.merge_state_status = pr.merge_state_status.clone();
        } else if pr.head_ref_oid != snap.head_sha {
            // Every page re-reads the PR's latest commit. If a synchronize
            // event advances the head while pagination is in progress,
            // mixing those pages would label new-head contexts as the old
            // head.
            return Err(anyhow!(
                "ghsnapshot: pull request head changed during pagination"
            ));
        }
        let Some(rollup) = pr
            .commits
            .nodes
            .into_iter()
            .next()
            .and_then(|n| n.commit.status_check_rollup)
        else {
            // statusCheckRollup is null → no checks yet. Nothing to paginate.
            if page > 0 {
                return Err(anyhow!(
                    "ghsnapshot: check rollup changed during pagination"
                ));
            }
            return Ok(snap);
        };
        snap.has_checks = true;
        snap.rollup_state = rollup.state.clone();
        for raw in &rollup.contexts.nodes {
            if let Some(cc) = normalize_node(raw) {
                snap.contexts.push(cc);
            }
        }
        if !rollup.contexts.page_info.has_next_page {
            return Ok(snap);
        }
        let next_cursor = rollup.contexts.page_info.end_cursor.clone();
        if next_cursor.is_empty() || Some(&next_cursor) == cursor.as_ref() {
            return Err(anyhow!(
                "ghsnapshot: invalid check-context pagination cursor"
            ));
        }
        if page == MAX_SNAPSHOT_CONTEXT_PAGES - 1 {
            return Err(anyhow!(
                "ghsnapshot: check-context pagination exceeds page limit"
            ));
        }
        cursor = Some(next_cursor);
    }
    Err(anyhow!(
        "ghsnapshot: check-context pagination exceeds page limit"
    ))
}

/// Flattens one GraphQL union node (CheckRun or StatusContext) into a
/// CheckContext. CheckRun statuses/conclusions map to lowercase; legacy
/// StatusContext states map onto the same lifecycle so aggregation is
/// uniform:
///
/// ```text
/// SUCCESS  → completed / success
/// FAILURE  → completed / failure
/// ERROR    → completed / error
/// PENDING  → in_progress / (running)
/// EXPECTED → queued / (running)
/// ```
pub fn normalize_node(raw: &serde_json::Value) -> Option<CheckContext> {
    let typename = raw.get("__typename")?.as_str()?;
    match typename {
        "CheckRun" => {
            let name = raw.get("name")?.as_str()?.to_string();
            let status = raw
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let conclusion = raw
                .get("conclusion")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let details_url = raw
                .get("detailsUrl")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            Some(CheckContext {
                name,
                status: normalize_run_status(status),
                conclusion: conclusion.to_lowercase(),
                details_url: details_url.to_string(),
                is_status_context: false,
            })
        }
        "StatusContext" => {
            let context = raw.get("context")?.as_str()?.to_string();
            let state = raw
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let target_url = raw
                .get("targetUrl")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let (status, conclusion) = normalize_status_state(state);
            Some(CheckContext {
                name: context,
                status,
                conclusion,
                details_url: target_url.to_string(),
                is_status_context: true,
            })
        }
        _ => None,
    }
}

/// Maps a GraphQL CheckRun.status enum to our lifecycle. Only COMPLETED is
/// terminal; QUEUED/IN_PROGRESS/WAITING/PENDING/REQUESTED are all still
/// running.
pub fn normalize_run_status(s: &str) -> String {
    match s.to_uppercase().as_str() {
        "COMPLETED" => "completed".to_string(),
        "IN_PROGRESS" => "in_progress".to_string(),
        _ => "queued".to_string(),
    }
}

/// Maps a legacy StatusContext.state onto (status, conclusion). Empty
/// conclusion means still running.
pub fn normalize_status_state(s: &str) -> (String, String) {
    match s.to_uppercase().as_str() {
        "SUCCESS" => ("completed".to_string(), "success".to_string()),
        "FAILURE" => ("completed".to_string(), "failure".to_string()),
        "ERROR" => ("completed".to_string(), "error".to_string()),
        "PENDING" => ("in_progress".to_string(), String::new()),
        // EXPECTED and any unknown state
        _ => ("queued".to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_run(status: &str, conclusion: &str) -> serde_json::Value {
        serde_json::json!({
            "__typename": "CheckRun",
            "name": "ci/test",
            "status": status,
            "conclusion": conclusion,
            "detailsUrl": "https://x.test"
        })
    }

    fn status_context(state: &str) -> serde_json::Value {
        serde_json::json!({
            "__typename": "StatusContext",
            "context": "legacy/ci",
            "state": state,
            "targetUrl": "https://y.test"
        })
    }

    #[test]
    fn snapshot_decided_semantics_match_go() {
        let mut s = PrSnapshot::default();
        assert!(!s.decided(), "empty mergeable is undecided");
        s.mergeable = "UNKNOWN".into();
        assert!(!s.decided());
        s.mergeable = "MERGEABLE".into();
        assert!(s.decided(), "no checks + known mergeability is settled");
        s.has_checks = true;
        assert!(
            !s.decided(),
            "empty rollup with checks present is undecided"
        );
        s.rollup_state = "PENDING".into();
        assert!(!s.decided());
        s.rollup_state = "EXPECTED".into();
        assert!(!s.decided());
        s.rollup_state = "SUCCESS".into();
        assert!(s.decided());
        s.contexts.push(CheckContext {
            name: "a".into(),
            status: "in_progress".into(),
            conclusion: String::new(),
            details_url: String::new(),
            is_status_context: false,
        });
        assert!(!s.decided(), "any running context keeps it undecided");
        s.contexts[0].status = "completed".into();
        assert!(s.decided());
    }

    #[test]
    fn normalize_node_flattens_both_union_shapes() {
        let cr = normalize_node(&check_run("IN_PROGRESS", "")).unwrap();
        assert_eq!(cr.name, "ci/test");
        assert_eq!(cr.status, "in_progress");
        assert_eq!(cr.conclusion, "");
        assert!(!cr.is_status_context);

        let cr = normalize_node(&check_run("COMPLETED", "FAILURE")).unwrap();
        assert_eq!(cr.status, "completed");
        assert_eq!(cr.conclusion, "failure");

        let sc = normalize_node(&status_context("SUCCESS")).unwrap();
        assert_eq!(sc.status, "completed");
        assert_eq!(sc.conclusion, "success");
        assert!(sc.is_status_context);

        let sc = normalize_node(&status_context("EXPECTED")).unwrap();
        assert_eq!(sc.status, "queued");
        assert_eq!(sc.conclusion, "");

        assert!(normalize_node(&serde_json::json!({"__typename": "Other"})).is_none());
    }

    #[test]
    fn run_status_and_status_state_tables() {
        assert_eq!(normalize_run_status("completed"), "completed");
        assert_eq!(normalize_run_status("queued"), "queued");
        assert_eq!(normalize_run_status("requested"), "queued");
        assert_eq!(
            normalize_status_state("ERROR"),
            ("completed".into(), "error".into())
        );
        assert_eq!(
            normalize_status_state("pending"),
            ("in_progress".into(), "".into())
        );
        assert_eq!(
            normalize_status_state("weird"),
            ("queued".into(), "".into())
        );
    }

    #[test]
    fn rollup_contexts_decode_camel_case_page_info() {
        let contexts: RollupContexts = serde_json::from_value(serde_json::json!({
            "pageInfo": {"hasNextPage": true, "endCursor": "cursor-2"},
            "nodes": []
        }))
        .unwrap();
        assert!(contexts.page_info.has_next_page);
        assert_eq!(contexts.page_info.end_cursor, "cursor-2");
    }

    #[tokio::test]
    #[ignore = "requires network"]
    async fn fetch_roundtrip() {
        let Some(client) = crate::client::Client::new_from_env().unwrap() else {
            return;
        };
        let _ = fetch_pr_snapshot(&client, 1, "octocat", "hello-world", 1)
            .await
            .unwrap();
    }
}
