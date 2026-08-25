//! Issue-column queries used by reorder operations.
//!
//! Pagination and cross-column diagnostics stay separate from reorder
//! validation and position arithmetic.

use anyhow::{Context, Result};
use serde_json::Value;
use url::form_urlencoded;

use super::{value_string, ApiClient};

pub(super) async fn fetch_issue_column(
    client: &ApiClient,
    workspace_id: &str,
    project_id: &str,
    status: &str,
) -> Result<Vec<Value>> {
    let mut issues = Vec::new();
    let mut offset = 0_i64;
    loop {
        let mut serializer = form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("workspace_id", workspace_id);
        serializer.append_pair("status", status);
        if !project_id.is_empty() {
            serializer.append_pair("project_id", project_id);
        }
        serializer.append_pair("sort", "position");
        serializer.append_pair("limit", "100");
        serializer.append_pair("offset", &offset.to_string());
        let result: Value = client
            .get_json(&format!("/api/issues?{}", serializer.finish()))
            .await
            .with_context(|| format!("list {status} column"))?;
        let page = result
            .get("issues")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let page_len = page.len() as i64;
        issues.extend(page);
        offset += page_len;
        let total = result.get("total").and_then(Value::as_i64).unwrap_or(0);
        if page_len == 0 || offset >= total {
            break;
        }
    }
    Ok(issues)
}

pub(super) async fn reorder_target_not_in_column(
    client: &ApiClient,
    other_id: &str,
    other_display: &str,
    issue_display: &str,
    status: &str,
) -> anyhow::Error {
    if let Ok(other) = client
        .get_json::<Value>(&format!("/api/issues/{other_id}"))
        .await
    {
        let other_status = value_string(&other, "status");
        if !other_status.is_empty() && other_status != status {
            return anyhow::anyhow!(
                "issue {other_display} is in the {other_status:?} column but {issue_display} is in {status:?}; move one with `cordy issue status` first, or pick a target in the same column"
            );
        }
    }
    anyhow::anyhow!("issue {other_display} was not found in the {status:?} column")
}
