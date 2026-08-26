//! Autopilot and trigger reference resolution.
//!
//! This module owns only UUID/reference lookup and pagination. Member and
//! agent resolution lives in `autopilot_member_resolver` so the two lookup
//! domains do not accumulate unrelated matching rules.

use anyhow::{bail, Error, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use url::form_urlencoded;

use super::{compact_uuid, is_canonical_uuid, normalize_uuid_prefix, value_string, ApiClient};

pub(super) fn context_autopilot_resolution(error: Error) -> Error {
    if error
        .to_string()
        .starts_with("ambiguous autopilot id prefix ")
    {
        error
    } else {
        anyhow::anyhow!("resolve autopilot: {error:#}")
    }
}

#[derive(Debug, Deserialize)]
struct AutopilotResolverEnvelope {
    autopilots: Vec<Value>,
    #[serde(default)]
    total: i64,
    #[serde(default)]
    has_more: bool,
}

pub(super) async fn resolve_autopilot_trigger_id(
    client: &ApiClient,
    autopilot_id: &str,
    input: &str,
) -> Result<String> {
    let trimmed = input.trim();
    if is_canonical_uuid(trimmed) {
        return Ok(trimmed.into());
    }
    let Some(prefix) = normalize_uuid_prefix(trimmed) else {
        if trimmed.is_empty() {
            bail!("autopilot trigger id is required");
        }
        let compact = trimmed.replace('-', "");
        if compact.len() < 4 {
            bail!(
                "resolve autopilot trigger: expected a full UUID or at least 4 hex characters, got {input:?}"
            );
        }
        bail!(
            "resolve autopilot trigger: expected a UUID prefix containing only hex characters, got {input:?}"
        );
    };
    let response: Value = client
        .get_json(&format!("/api/autopilots/{autopilot_id}"))
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot trigger: {error:#}"))?;
    let mut matches = response
        .get("triggers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|trigger| value_string(trigger, "id"))
        .filter(|id| !id.is_empty() && compact_uuid(id).starts_with(&prefix))
        .collect::<Vec<_>>();
    matches.sort();
    match matches.as_slice() {
        [id] => Ok(id.clone()),
        [] => bail!(
            "no autopilot trigger found matching id prefix {input:?}; run the list command with --full-id to copy the full UUID"
        ),
        _ => bail!(
            "ambiguous autopilot trigger id prefix {input:?}; matches:\n  {}\nUse more characters or run the list command with --full-id",
            matches.join("\n  ")
        ),
    }
}

pub(super) async fn resolve_autopilot_id(
    client: &ApiClient,
    workspace_id: &str,
    input: &str,
) -> Result<(String, String)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("autopilot id is required");
    }
    if is_canonical_uuid(trimmed) {
        return Ok((trimmed.into(), trimmed.into()));
    }
    let Some(prefix) = normalize_uuid_prefix(trimmed) else {
        let compact = trimmed.replace('-', "");
        if compact.len() < 4 {
            bail!(
                "resolve autopilot: expected a full UUID or at least 4 hex characters, got {input:?}"
            );
        }
        bail!(
            "resolve autopilot: expected a UUID prefix containing only hex characters, got {input:?}"
        );
    };
    if workspace_id.is_empty() {
        bail!("workspace_id is required to resolve autopilot id prefixes");
    }

    const LIMIT: usize = 50;
    let mut offset = 0;
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    loop {
        let mut query = form_urlencoded::Serializer::new(String::new());
        query.append_pair("limit", &LIMIT.to_string());
        if offset > 0 {
            query.append_pair("offset", &offset.to_string());
        }
        query.append_pair("workspace_id", workspace_id);
        let page: AutopilotResolverEnvelope = client
            .get_json(&format!("/api/autopilots?{}", query.finish()))
            .await
            .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
        let page_len = page.autopilots.len();
        let mut added = 0;
        for autopilot in page.autopilots {
            let id = value_string(&autopilot, "id");
            if !id.is_empty() && seen.insert(id.clone()) {
                added += 1;
                let title = value_string(&autopilot, "title");
                candidates.push((id.clone(), if title.is_empty() { id } else { title }));
            }
        }
        offset += page_len;
        if page_len == 0 || added == 0 || page_len < LIMIT {
            break;
        }
        if page.has_more {
            continue;
        }
        if page.total > 0 {
            if offset as i64 >= page.total {
                break;
            }
            continue;
        }
        break;
    }

    let mut matches = candidates
        .into_iter()
        .filter(|(id, _)| compact_uuid(id).starts_with(&prefix))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.0.cmp(&right.0));
    match matches.as_slice() {
        [resolved] => Ok(resolved.clone()),
        [] => bail!(
            "no autopilot found matching id prefix {input:?}; run the list command with --full-id to copy the full UUID"
        ),
        _ => bail!(
            "ambiguous autopilot id prefix {input:?}; matches:\n  {}\nUse more characters or run the list command with --full-id",
            matches
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
    }
}
