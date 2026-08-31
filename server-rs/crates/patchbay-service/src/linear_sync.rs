//! Provider-neutral Linear synchronization helpers.
//!
//! Network workers use these pure functions after loading durable inbox and
//! outbox rows.  Keeping description editing and conflict classification here
//! makes retries deterministic and preserves human-authored Linear text.

use std::collections::BTreeMap;

use serde_json::Value;

pub const MANAGED_BLOCK_START: &str = "<!-- patchbay:managed:start -->";
pub const MANAGED_BLOCK_END: &str = "<!-- patchbay:managed:end -->";

pub fn managed_description_block(
    acceptance_criteria: &Value,
    orchestration_summary: Option<&str>,
) -> String {
    let mut lines = vec![MANAGED_BLOCK_START.to_string(), "### Patchbay".to_string()];
    if !acceptance_criteria.is_null() {
        lines.push(format!(
            "- Acceptance criteria: `{}`",
            serde_json::to_string(acceptance_criteria).unwrap_or_else(|_| "null".into())
        ));
    }
    if let Some(summary) = orchestration_summary.map(str::trim).filter(|v| !v.is_empty()) {
        lines.push(format!("- Orchestration: {summary}"));
    }
    lines.push(MANAGED_BLOCK_END.to_string());
    lines.join("\n")
}

/// Replace only the Patchbay-managed block and keep all text outside it byte
/// for byte.  A missing block is appended with one separating newline.
pub fn merge_managed_description(human_description: &str, managed_block: &str) -> String {
    let Some(start) = human_description.find(MANAGED_BLOCK_START) else {
        return if human_description.trim().is_empty() {
            managed_block.to_string()
        } else {
            format!("{}\n\n{}", human_description.trim_end(), managed_block)
        };
    };
    let Some(end_offset) = human_description[start..].find(MANAGED_BLOCK_END) else {
        // A malformed block is human text.  Do not delete it; append a fresh
        // block so a future repair can be reviewed explicitly.
        return format!("{}\n\n{}", human_description.trim_end(), managed_block);
    };
    let end = start + end_offset + MANAGED_BLOCK_END.len();
    let mut output = String::with_capacity(human_description.len() + managed_block.len());
    output.push_str(&human_description[..start]);
    output.push_str(managed_block);
    output.push_str(&human_description[end..]);
    output
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldConflict {
    pub field: String,
    pub base: Option<Value>,
    pub local: Option<Value>,
    pub remote: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergeResult {
    pub merged: BTreeMap<String, Value>,
    pub conflicts: Vec<FieldConflict>,
}

/// Three-way merge fields independently.  A field changed on only one side
/// wins automatically; a field changed on both sides is surfaced for the
/// conflict center instead of silently overwriting either writer.
pub fn merge_fields(
    base: &BTreeMap<String, Value>,
    local: &BTreeMap<String, Value>,
    remote: &BTreeMap<String, Value>,
) -> MergeResult {
    let keys = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut merged = BTreeMap::new();
    let mut conflicts = Vec::new();
    for field in keys {
        let base_value = base.get(&field);
        let local_value = local.get(&field);
        let remote_value = remote.get(&field);
        let local_changed = local_value != base_value;
        let remote_changed = remote_value != base_value;
        match (local_changed, remote_changed) {
            (true, true) if local_value != remote_value => conflicts.push(FieldConflict {
                field,
                base: base_value.cloned(),
                local: local_value.cloned(),
                remote: remote_value.cloned(),
            }),
            (true, _) => {
                if let Some(value) = local_value {
                    merged.insert(field, value.clone());
                }
            }
            (_, true) => {
                if let Some(value) = remote_value {
                    merged.insert(field, value.clone());
                }
            }
            (false, false) => {
                if let Some(value) = base_value {
                    merged.insert(field, value.clone());
                }
            }
        }
    }
    MergeResult { merged, conflicts }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_block_preserves_human_text() {
        let existing = "Human heading\n\n<!-- patchbay:managed:start -->\nold\n<!-- patchbay:managed:end -->\n\nHuman footer";
        let merged = merge_managed_description(existing, "<!-- patchbay:managed:start -->\nnew\n<!-- patchbay:managed:end -->");
        assert_eq!(
            merged,
            "Human heading\n\n<!-- patchbay:managed:start -->\nnew\n<!-- patchbay:managed:end -->\n\nHuman footer"
        );
    }

    #[test]
    fn three_way_merge_only_conflicts_on_same_field() {
        let base = BTreeMap::from([
            ("title".into(), Value::String("old".into())),
            ("priority".into(), Value::String("none".into())),
        ]);
        let local = BTreeMap::from([
            ("title".into(), Value::String("local".into())),
            ("priority".into(), Value::String("none".into())),
        ]);
        let remote = BTreeMap::from([
            ("title".into(), Value::String("remote".into())),
            ("priority".into(), Value::String("high".into())),
        ]);
        let result = merge_fields(&base, &local, &remote);
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].field, "title");
        assert_eq!(result.merged["priority"], Value::String("high".into()));
    }
}
