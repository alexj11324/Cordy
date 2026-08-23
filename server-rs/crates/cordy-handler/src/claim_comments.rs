//! Coalesced comment loading + claim delivery budget — port of the comment
//! half of `server/internal/handler/daemon.go` (`buildCoalescedCommentData`,
//! `selectCommentDelivery`, `formatLegacyCommentBundle` and friends).

use std::collections::HashSet;

use serde_json::json;

use crate::timefmt::rfc3339;

/// One folded comment's full detail (Go `CoalescedCommentData`). Serialized
/// with Go's omitempty semantics: empty thread/author/created_at fields drop
/// off the wire.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CoalescedCommentData {
    pub id: String,
    pub thread_id: String,
    pub author_type: String,
    pub author_name: String,
    pub content: String,
    pub created_at: String,
}

impl CoalescedCommentData {
    pub fn to_json(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        m.insert("id".to_string(), json!(self.id));
        if !self.thread_id.is_empty() {
            m.insert("thread_id".to_string(), json!(self.thread_id));
        }
        if !self.author_type.is_empty() {
            m.insert("author_type".to_string(), json!(self.author_type));
        }
        if !self.author_name.is_empty() {
            m.insert("author_name".to_string(), json!(self.author_name));
        }
        m.insert("content".to_string(), json!(self.content));
        if !self.created_at.is_empty() {
            m.insert("created_at".to_string(), json!(self.created_at));
        }
        serde_json::Value::Object(m)
    }

    /// The JSON encoding size of this entry as one array element, plus the
    /// separating comma (Go `commentDeliveryEntrySize`, non-legacy branch).
    fn encoded_entry_size(&self) -> usize {
        match serde_json::to_vec(&self.to_json()) {
            Ok(bytes) => bytes.len() + 1,
            Err(_) => MAX_CLAIM_COMMENT_PAYLOAD_BYTES + 1,
        }
    }

    /// Escaped string content cost when nested inside the legacy bundle
    /// (Go `escapedJSONStringContentSize`).
    fn escaped_content_size(s: &str) -> usize {
        match serde_json::to_string(s) {
            Ok(encoded) => encoded.len().saturating_sub(2),
            Err(_) => MAX_CLAIM_COMMENT_PAYLOAD_BYTES + 1,
        }
    }
}

/// 512 KiB of comment input per claim (Go `maxClaimCommentPayloadBytes`).
pub const MAX_CLAIM_COMMENT_PAYLOAD_BYTES: usize = 512 << 10;

const LEGACY_COMMENT_BUNDLE_HEADER: &str = "This run covers multiple distinct issue comments. Address every comment below in chronological order; do not treat this bundle as one rewritten comment.\n";

fn legacy_comment_entry(comment: &CoalescedCommentData) -> String {
    let mut b = String::new();
    b.push_str(&format!("\n--- comment {}", comment.id));
    if !comment.thread_id.is_empty() {
        b.push_str(&format!(" [thread {}]", comment.thread_id));
    }
    if !comment.author_type.is_empty() || !comment.author_name.is_empty() {
        b.push_str(&format!(" [author {}", comment.author_type));
        if !comment.author_name.is_empty() {
            b.push_str(&format!(": {}", comment.author_name));
        }
        b.push(']');
    }
    if !comment.created_at.is_empty() {
        b.push_str(&format!(" [created {}]", comment.created_at));
    }
    b.push_str(" ---\n");
    b.push_str(&comment.content);
    b.push_str(&format!("\n--- end comment {} ---\n", comment.id));
    b
}

/// Carries every planned comment through the one field understood by daemons
/// that predate coalesced-comments-v1. Delimiters, ids and thread ids keep
/// distinct instructions attributable and fetchable.
pub fn format_legacy_comment_bundle(comments: &[CoalescedCommentData]) -> String {
    if comments.is_empty() {
        return String::new();
    }
    let mut b = String::from(LEGACY_COMMENT_BUNDLE_HEADER);
    for c in comments {
        b.push_str(&legacy_comment_entry(c));
    }
    b.trim().to_string()
}

fn comment_by_id<'a>(
    comments: &'a [CoalescedCommentData],
    id: &str,
) -> Option<&'a CoalescedCommentData> {
    comments.iter().find(|c| c.id == id)
}

fn legacy_entry_size(comment: &CoalescedCommentData) -> usize {
    CoalescedCommentData::escaped_content_size(&legacy_comment_bundle_entry_cost(comment))
}

fn legacy_comment_bundle_entry_cost(comment: &CoalescedCommentData) -> String {
    // The bundle nests each entry inside a JSON string; the budget counts the
    // escaped form of the whole entry text.
    legacy_comment_entry(comment)
}

/// Deterministic claim budget (Go `selectCommentDelivery`). The primary trigger
/// is mandatory when it still exists — even when that single comment exceeds
/// the budget. Extra comments are admitted as an oldest-first prefix so
/// overflow remains a stable suffix for completion reconciliation.
pub fn select_comment_delivery(
    comments: &[CoalescedCommentData],
    trigger_id: &str,
    legacy: bool,
    limit: usize,
) -> Vec<CoalescedCommentData> {
    if comments.is_empty() {
        return Vec::new();
    }
    let mut mandatory_id = trigger_id.to_string();
    if !comments.iter().any(|c| c.id == mandatory_id) {
        // The planned trigger may have been deleted. Keep the newest available
        // comment so the claim still makes progress and reconcile picks up the
        // remainder.
        mandatory_id = comments[comments.len() - 1].id.clone();
    }

    let mut selected: HashSet<String> = HashSet::new();
    selected.insert(mandatory_id.clone());

    let base = if legacy {
        CoalescedCommentData::escaped_content_size(LEGACY_COMMENT_BUNDLE_HEADER)
    } else {
        2 // JSON array brackets
    };
    let entry_cost = |c: &CoalescedCommentData| -> usize {
        if legacy {
            legacy_entry_size(c)
        } else {
            c.encoded_entry_size()
        }
    };
    let mut used = base;
    if let Some(mandatory) = comment_by_id(comments, &mandatory_id) {
        used += entry_cost(mandatory);
    }
    for comment in comments {
        if comment.id == mandatory_id {
            continue;
        }
        let cost = entry_cost(comment);
        if limit > 0 && used + cost > limit {
            break;
        }
        selected.insert(comment.id.clone());
        used += cost;
    }

    comments
        .iter()
        .filter(|c| selected.contains(&c.id))
        .cloned()
        .collect()
}

pub fn comment_data_ids(comments: &[CoalescedCommentData]) -> Vec<uuid::Uuid> {
    comments
        .iter()
        .filter_map(|c| uuid::Uuid::parse_str(&c.id).ok())
        .collect()
}

/// Row loader shared by the handler: workspace-scoped so a foreign comment UUID
/// resolves to "missing" (skipped) instead of leaking another tenant's text
/// into the prompt (MUL-4252). Chronologically sorted, de-duplicated.
///
/// Port of Go `buildCoalescedCommentData`.
pub async fn build_coalesced_comment_data(
    pool: &sqlx::PgPool,
    workspace_id: uuid::Uuid,
    ids: &[uuid::Uuid],
) -> Vec<CoalescedCommentData> {
    if ids.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<CoalescedCommentData> = Vec::with_capacity(ids.len());
    let mut seen: HashSet<String> = HashSet::with_capacity(ids.len());
    for id in ids {
        let id_string = id.to_string();
        if !seen.insert(id_string.clone()) {
            continue;
        }
        let comment =
            match cordy_db::queries::comment::get_comment_in_workspace(pool, *id, workspace_id)
                .await
            {
                Ok(Some(c)) => c,
                Ok(None) => continue,
                Err(e) => {
                    tracing::debug!(error = %e, comment_id = %id, "claim: load comment failed");
                    continue;
                }
            };
        let mut data = CoalescedCommentData {
            id: comment.id.to_string(),
            thread_id: comment.id.to_string(),
            author_type: comment.author_type.clone(),
            content: comment.content.clone(),
            created_at: rfc3339(comment.created_at),
            ..Default::default()
        };
        if let Some(parent) = comment.parent_id {
            data.thread_id = parent.to_string();
        }
        match comment.author_type.as_str() {
            "agent" => {
                if let Ok(Some(a)) =
                    cordy_db::queries::agent::get_agent(pool, comment.author_id).await
                {
                    data.author_name = a.name;
                }
            }
            "member" => {
                if let Ok(Some(u)) =
                    cordy_db::queries::user::get_user(pool, comment.author_id).await
                {
                    data.author_name = u.name;
                }
            }
            _ => {}
        }
        out.push(data);
    }
    sort_chronological(&mut out);
    out
}

fn sort_chronological(out: &mut [CoalescedCommentData]) {
    out.sort_by(|a, b| {
        let a_key = a.created_at.clone();
        let b_key = b.created_at.clone();
        a_key.cmp(&b_key).then_with(|| a.id.cmp(&b.id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, created: &str, content: &str) -> CoalescedCommentData {
        CoalescedCommentData {
            id: id.to_string(),
            created_at: created.to_string(),
            content: content.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn trigger_is_mandatory_even_over_budget() {
        let big = c(
            "t",
            "2026-01-01T00:00:00Z",
            &"x".repeat(MAX_CLAIM_COMMENT_PAYLOAD_BYTES * 2),
        );
        let small = c("a", "2026-01-02T00:00:00Z", "hi");
        let got =
            select_comment_delivery(&[big, small], "t", false, MAX_CLAIM_COMMENT_PAYLOAD_BYTES);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "t");
    }

    #[test]
    fn deleted_trigger_falls_back_to_newest_and_admits_prefix() {
        let older = c("a", "2026-01-01T00:00:00Z", "old");
        let newest = c("z", "2026-01-05T00:00:00Z", "new");
        // The mandatory slot takes the newest; the oldest-first prefix also
        // admits the older comment within budget (matches Go's loop).
        let got = select_comment_delivery(
            &[older, newest],
            "gone",
            false,
            MAX_CLAIM_COMMENT_PAYLOAD_BYTES,
        );
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].id, "z");

        // A tight budget keeps only the mandatory (newest) comment.
        let got2 = select_comment_delivery(
            &[
                c("a", "2026-01-01T00:00:00Z", "old"),
                c("z", "2026-01-05T00:00:00Z", "new"),
            ],
            "gone",
            false,
            1,
        );
        assert_eq!(got2.len(), 1);
        assert_eq!(got2[0].id, "z");
    }

    #[test]
    fn extra_comments_admit_oldest_first_prefix_and_stop_cleanly() {
        let comments = vec![
            c("1", "2026-01-01T00:00:00Z", "one"),
            c("2", "2026-01-02T00:00:00Z", "two"),
            c("3", "2026-01-03T00:00:00Z", "three"),
            c("4", "2026-01-04T00:00:00Z", "four"),
        ];
        // Generous budget: everything fits, order stays chronological.
        let got = select_comment_delivery(&comments, "3", false, MAX_CLAIM_COMMENT_PAYLOAD_BYTES);
        let ids: Vec<&str> = got.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["1", "2", "3", "4"]);

        // A budget that fits the trigger plus one extra stops cleanly at the
        // overflow point (stable suffix for reconcile).
        let tight: Vec<CoalescedCommentData> = vec![
            c("1", "2026-01-01T00:00:00Z", "one"),
            c("3", "2026-01-03T00:00:00Z", "three"),
            c("4", "2026-01-04T00:00:00Z", "four"),
        ];
        let got2 = select_comment_delivery(&tight, "3", false, usize::MAX);
        let ids2: Vec<&str> = got2.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids2, vec!["1", "3", "4"]);
    }

    #[test]
    fn legacy_bundle_formats_entries_with_delimiters() {
        let comments = vec![CoalescedCommentData {
            id: "aaa".into(),
            thread_id: "root".into(),
            author_type: "member".into(),
            author_name: "Alex".into(),
            content: "hello".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        }];
        let bundle = format_legacy_comment_bundle(&comments);
        assert!(bundle.contains("--- comment aaa [thread root]"));
        assert!(bundle.contains("[author member: Alex]"));
        assert!(bundle.contains("hello"));
        assert!(bundle.ends_with("--- end comment aaa ---"));
    }
}
