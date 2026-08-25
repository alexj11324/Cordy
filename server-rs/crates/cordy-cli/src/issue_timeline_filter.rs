use anyhow::{bail, Context, Result};
use chrono::{DateTime, FixedOffset};
use serde_json::Value;
use std::collections::HashSet;

use super::{value_string, IssueTimelineArgs};

#[derive(Debug)]
pub(super) struct TimelineFilter {
    pub(super) activity_only: bool,
    pub(super) actions: HashSet<String>,
    pub(super) since: Option<DateTime<FixedOffset>>,
    pub(super) tail: usize,
}

pub(super) fn build_timeline_filter(args: &IssueTimelineArgs) -> Result<TimelineFilter> {
    if args.tail < 0 {
        bail!("--tail must be >= 0");
    }
    let actions = args
        .action
        .iter()
        .map(|action| action.trim())
        .filter(|action| !action.is_empty())
        .map(ToOwned::to_owned)
        .collect::<HashSet<_>>();
    let since = args
        .since
        .as_deref()
        .filter(|since| !since.is_empty())
        .map(|since| {
            DateTime::parse_from_rfc3339(since).with_context(|| {
                format!("invalid --since {since:?}: expected RFC3339, e.g. 2026-08-19T00:00:00Z")
            })
        })
        .transpose()?;
    Ok(TimelineFilter {
        activity_only: args.activity_only || !actions.is_empty(),
        actions,
        since,
        tail: args.tail as usize,
    })
}

pub(super) fn filter_timeline(entries: Vec<Value>, filter: &TimelineFilter) -> Vec<Value> {
    let mut entries = entries
        .into_iter()
        .filter(|entry| {
            if filter.activity_only && value_string(entry, "type") != "activity" {
                return false;
            }
            if !filter.actions.is_empty()
                && !filter.actions.contains(&value_string(entry, "action"))
            {
                return false;
            }
            let Some(since) = filter.since.as_ref() else {
                return true;
            };
            DateTime::parse_from_rfc3339(&value_string(entry, "created_at"))
                .is_ok_and(|created| created > *since)
        })
        .collect::<Vec<_>>();
    if filter.tail > 0 && entries.len() > filter.tail {
        entries.drain(..entries.len() - filter.tail);
    }
    entries
}
