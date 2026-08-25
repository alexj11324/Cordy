//! Bounded comment-list projections for issue readers.

use std::collections::{HashMap, HashSet};

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use cordy_db::models::Comment;
use cordy_db::queries::{attachment, comment, reaction};
use cordy_middleware::workspace::WorkspaceContext;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const COMMENT_HARD_CAP: usize = 2_000;
const COMMENT_PROBE_LIMIT: i32 = 2_001;
const COMMENT_THREAD_CONTEXT_BUDGET: usize = 2_000;
const COMMENT_THREAD_MAX_DEPTH: usize = 64;
const SUMMARY_CONTENT_RUNES: usize = 200;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ListQuery {
    since: Option<String>,
    thread: Option<String>,
    recent: Option<String>,
    tail: Option<String>,
    before: Option<String>,
    before_id: Option<String>,
    #[serde(rename = "before-id")]
    before_id_alias: Option<String>,
    roots_only: Option<String>,
    #[serde(rename = "roots-only")]
    roots_only_alias: Option<String>,
    summary: Option<String>,
    fold: Option<String>,
}

#[derive(Debug)]
struct ListParams {
    since: Option<DateTime<Utc>>,
    thread: Option<Uuid>,
    recent: Option<usize>,
    tail: Option<usize>,
    before: Option<(DateTime<Utc>, Uuid)>,
    roots_only: bool,
    summary: bool,
    fold: bool,
}

fn parse_bool(raw: Option<&str>, name: &str) -> Result<bool, String> {
    match raw {
        None | Some("") | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(format!("invalid {name} parameter; expected boolean")),
    }
}

fn parse_time(raw: &str, name: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| format!("invalid {name} parameter; expected RFC3339 format"))
}

fn parse_query(query: ListQuery) -> Result<ListParams, String> {
    let roots_raw = query
        .roots_only
        .as_deref()
        .or(query.roots_only_alias.as_deref());
    let before_id_raw = query
        .before_id
        .as_deref()
        .or(query.before_id_alias.as_deref());
    let roots_only = parse_bool(roots_raw, "roots_only")?;
    let summary = parse_bool(query.summary.as_deref(), "summary")?;
    let fold = parse_bool(query.fold.as_deref(), "fold")?;
    let since = query
        .since
        .as_deref()
        .map(|value| parse_time(value, "since"))
        .transpose()?;

    let thread_raw = query.thread.filter(|value| !value.is_empty());
    let recent_raw = query.recent.filter(|value| !value.is_empty());
    let tail_raw = query.tail.filter(|value| !value.is_empty());
    let before_raw = query.before.filter(|value| !value.is_empty());
    let before_id_raw = before_id_raw.filter(|value| !value.is_empty());

    if fold && since.is_some() {
        return Err("fold and since are mutually exclusive: since returns a partial thread, and a fold over a partial thread could hide a resolution that was not fetched".into());
    }
    if fold && tail_raw.is_some() {
        return Err("fold and tail are mutually exclusive: tail returns a partial thread, which cannot be folded safely".into());
    }
    if fold && roots_only {
        return Err(
            "fold and roots_only are mutually exclusive: roots_only returns no replies to fold"
                .into(),
        );
    }
    if roots_only && thread_raw.is_some() {
        return Err("roots_only and thread are mutually exclusive".into());
    }
    if roots_only && recent_raw.is_some() {
        return Err("roots_only and recent are mutually exclusive".into());
    }
    if roots_only && tail_raw.is_some() {
        return Err("roots_only and tail are mutually exclusive".into());
    }
    if roots_only && (before_raw.is_some() || before_id_raw.is_some()) {
        return Err("roots_only does not support before / before_id".into());
    }
    if thread_raw.is_some() && recent_raw.is_some() {
        return Err("thread and recent are mutually exclusive".into());
    }
    if tail_raw.is_some() && thread_raw.is_none() {
        return Err("tail requires thread (it is a thread-scoped limit)".into());
    }
    if before_raw.is_some() != before_id_raw.is_some() {
        return Err("before and before_id must be set together (composite cursor)".into());
    }
    if before_raw.is_some() && recent_raw.is_none() && (thread_raw.is_none() || tail_raw.is_none())
    {
        return Err(
            "before / before_id require recent (thread cursor) or thread + tail (reply cursor)"
                .into(),
        );
    }

    let thread = thread_raw
        .as_deref()
        .map(|raw| {
            Uuid::parse_str(raw).map_err(|_| "invalid thread parameter; expected UUID".to_string())
        })
        .transpose()?;
    let recent = recent_raw
        .as_deref()
        .map(|raw| {
            raw.parse::<usize>()
                .ok()
                .filter(|value| *value > 0)
                .map(|value| value.min(COMMENT_HARD_CAP))
                .ok_or_else(|| "invalid recent parameter; expected positive integer".to_string())
        })
        .transpose()?;
    let tail = tail_raw
        .as_deref()
        .map(|raw| {
            raw.parse::<usize>()
                .map(|value| value.min(COMMENT_HARD_CAP))
                .map_err(|_| "invalid tail parameter; expected non-negative integer".to_string())
        })
        .transpose()?;
    let before = match (before_raw.as_deref(), before_id_raw) {
        (Some(at), Some(id)) => Some((
            parse_time(at, "before")?,
            Uuid::parse_str(id)
                .map_err(|_| "invalid before_id parameter; expected UUID".to_string())?,
        )),
        _ => None,
    };

    Ok(ListParams {
        since,
        thread,
        recent,
        tail,
        before,
        roots_only,
        summary,
        fold,
    })
}

#[derive(Default)]
struct FetchResult {
    comments: Vec<Comment>,
    root_stats: HashMap<Uuid, (i32, DateTime<Utc>)>,
    next_before: Option<(DateTime<Utc>, Uuid)>,
    truncated: bool,
    fold_unsafe: bool,
}

fn recent_row_comment(row: &comment::ListRecentThreadCommentsForIssueRow) -> Option<Comment> {
    Some(Comment {
        id: row.id?,
        issue_id: row.issue_id?,
        author_type: row.author_type.clone(),
        author_id: row.author_id?,
        content: row.content.clone(),
        type_: row.type_.clone(),
        created_at: row.created_at?,
        updated_at: row.updated_at?,
        parent_id: row.parent_id,
        workspace_id: row.workspace_id?,
        resolved_at: row.resolved_at,
        resolved_by_type: row.resolved_by_type.clone(),
        resolved_by_id: row.resolved_by_id,
        source_task_id: row.source_task_id,
        quick_action_id: row.quick_action_id,
        via_plugin_id: None,
        revision: row.revision,
    })
}

fn paged_row_comment(row: &comment::ListThreadCommentsForIssuePagedRow) -> Option<Comment> {
    Some(Comment {
        id: row.id?,
        issue_id: row.issue_id?,
        author_type: row.author_type.clone(),
        author_id: row.author_id?,
        content: row.content.clone(),
        type_: row.type_.clone(),
        created_at: row.created_at?,
        updated_at: row.updated_at?,
        parent_id: row.parent_id,
        workspace_id: row.workspace_id?,
        resolved_at: row.resolved_at,
        resolved_by_type: row.resolved_by_type.clone(),
        resolved_by_id: row.resolved_by_id,
        source_task_id: row.source_task_id,
        quick_action_id: row.quick_action_id,
        via_plugin_id: None,
        revision: row.revision,
    })
}

macro_rules! root_row_comment {
    ($row:expr) => {{
        let row = $row;
        anyhow::Ok(Comment {
            id: row
                .id
                .ok_or_else(|| anyhow::anyhow!("invalid-comment-row"))?,
            issue_id: row
                .issue_id
                .ok_or_else(|| anyhow::anyhow!("invalid-comment-row"))?,
            author_type: row.author_type,
            author_id: row
                .author_id
                .ok_or_else(|| anyhow::anyhow!("invalid-comment-row"))?,
            content: row.content,
            type_: row.type_,
            created_at: row
                .created_at
                .ok_or_else(|| anyhow::anyhow!("invalid-comment-row"))?,
            updated_at: row
                .updated_at
                .ok_or_else(|| anyhow::anyhow!("invalid-comment-row"))?,
            parent_id: row.parent_id,
            workspace_id: row
                .workspace_id
                .ok_or_else(|| anyhow::anyhow!("invalid-comment-row"))?,
            resolved_at: row.resolved_at,
            resolved_by_type: row.resolved_by_type,
            resolved_by_id: row.resolved_by_id,
            source_task_id: row.source_task_id,
            quick_action_id: row.quick_action_id,
            via_plugin_id: None,
            revision: row.revision,
        })
    }};
}

async fn fetch_comments(
    state: &HandlerState,
    issue_id: Uuid,
    workspace_id: Uuid,
    params: &ListParams,
) -> anyhow::Result<FetchResult> {
    if let Some(anchor) = params.thread {
        let tail = params.tail.unwrap_or(COMMENT_HARD_CAP);
        let probe = if params.tail.is_some() {
            tail.saturating_add(1)
        } else {
            COMMENT_HARD_CAP
        };
        let rows = comment::list_thread_comments_for_issue_paged(
            &state.pool,
            anchor,
            issue_id,
            workspace_id,
            params.before.is_some(),
            params.before.map(|value| value.0),
            params.before.map(|value| value.1).unwrap_or(Uuid::nil()),
            i32::try_from(probe).unwrap_or(i32::MAX),
        )
        .await?;
        if rows.is_empty() {
            anyhow::bail!("thread-not-found");
        }
        let mut root = None;
        let mut replies = Vec::new();
        for row in &rows {
            let Some(value) = paged_row_comment(row) else {
                anyhow::bail!("invalid-comment-row")
            };
            if value.parent_id.is_none() {
                root = Some(value);
            } else {
                replies.push(value);
            }
        }
        let truncated = params.tail.is_none() && replies.len() >= COMMENT_HARD_CAP;
        let has_more = if params.tail.is_some() {
            replies.len() > tail
        } else {
            truncated
        };
        if has_more && !replies.is_empty() {
            replies.remove(0);
        }
        let cursor_reply = replies.first().cloned();
        let mut comments = Vec::with_capacity(replies.len() + 1);
        if let Some(root) = root {
            if params.tail.is_some() || params.since.is_none_or(|since| root.created_at > since) {
                comments.push(root);
            }
        }
        comments.extend(
            replies
                .into_iter()
                .filter(|row| params.since.is_none_or(|since| row.created_at > since)),
        );
        let next_before = cursor_reply
            .filter(|row| has_more && params.since.is_none_or(|since| row.created_at > since))
            .map(|row| (row.created_at, row.id));
        return Ok(FetchResult {
            comments,
            next_before,
            truncated,
            fold_unsafe: truncated,
            ..Default::default()
        });
    }

    if let Some(recent) = params.recent {
        let rows = comment::list_recent_thread_comments_for_issue(
            &state.pool,
            issue_id,
            workspace_id,
            params.before.is_some(),
            params.before.map(|value| value.0),
            params.before.map(|value| value.1).unwrap_or(Uuid::nil()),
            i32::try_from(recent).unwrap_or(i32::MAX),
        )
        .await?;
        let mut comments = Vec::new();
        let mut roots = HashSet::new();
        let mut head = None;
        for row in &rows {
            if head.is_none() {
                head = row.thread_last_activity_at.zip(row.thread_root_id);
            }
            if let Some(root) = row.thread_root_id {
                roots.insert(root);
            }
            let Some(value) = recent_row_comment(row) else {
                anyhow::bail!("invalid-comment-row")
            };
            if params.since.is_none_or(|since| value.created_at > since) {
                comments.push(value);
            }
        }
        let next_before = head.filter(|(at, _)| {
            roots.len() >= recent && params.since.is_none_or(|since| *at > since)
        });
        return Ok(FetchResult {
            comments,
            next_before,
            ..Default::default()
        });
    }

    if params.roots_only {
        let mut result = FetchResult::default();
        if let Some(since) = params.since {
            let mut rows = comment::list_root_comments_since_for_issue(
                &state.pool,
                issue_id,
                workspace_id,
                Some(since),
                COMMENT_PROBE_LIMIT,
            )
            .await?;
            result.truncated = rows.len() > COMMENT_HARD_CAP;
            rows.truncate(COMMENT_HARD_CAP);
            for row in rows {
                let reply_count = row.reply_count;
                let last_activity_at = row.last_activity_at;
                let value = root_row_comment!(row)?;
                if let Some(at) = last_activity_at {
                    result.root_stats.insert(value.id, (reply_count, at));
                }
                result.comments.push(value);
            }
        } else {
            let mut rows = comment::list_root_comments_for_issue(
                &state.pool,
                issue_id,
                workspace_id,
                COMMENT_PROBE_LIMIT,
            )
            .await?;
            result.truncated = rows.len() > COMMENT_HARD_CAP;
            if result.truncated {
                rows.remove(0);
            }
            for row in rows {
                let reply_count = row.reply_count;
                let last_activity_at = row.last_activity_at;
                let value = root_row_comment!(row)?;
                if let Some(at) = last_activity_at {
                    result.root_stats.insert(value.id, (reply_count, at));
                }
                result.comments.push(value);
            }
        }
        return Ok(result);
    }

    if let Some(since) = params.since {
        let mut comments = comment::list_comments_since_for_issue(
            &state.pool,
            issue_id,
            workspace_id,
            Some(since),
            COMMENT_PROBE_LIMIT,
        )
        .await?;
        let truncated = comments.len() > COMMENT_HARD_CAP;
        comments.truncate(COMMENT_HARD_CAP);
        return Ok(FetchResult {
            comments,
            truncated,
            ..Default::default()
        });
    }
    let mut comments =
        comment::list_comments_for_issue(&state.pool, issue_id, workspace_id, COMMENT_PROBE_LIMIT)
            .await?;
    let truncated = comments.len() > COMMENT_HARD_CAP;
    if truncated {
        comments.remove(0);
        comments = complete_comment_threads(state, issue_id, workspace_id, comments).await?;
    }
    Ok(FetchResult {
        comments,
        truncated,
        ..Default::default()
    })
}

fn root_id(id: Uuid, by_id: &HashMap<Uuid, Comment>) -> Option<Uuid> {
    let mut current = id;
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(current) {
            return None;
        }
        let item = by_id.get(&current)?;
        match item.parent_id {
            Some(parent_id) => current = parent_id,
            None => return Some(current),
        }
    }
}

fn keep_root_connected(by_id: HashMap<Uuid, Comment>) -> HashMap<Uuid, Comment> {
    let connected = by_id
        .keys()
        .filter(|id| root_id(**id, &by_id).is_some())
        .copied()
        .collect::<HashSet<_>>();
    by_id
        .into_iter()
        .filter(|(id, _)| connected.contains(id))
        .collect()
}

fn drop_threads(by_id: &mut HashMap<Uuid, Comment>, roots: &HashSet<Uuid>) {
    let drop_ids = by_id
        .keys()
        .filter(|id| root_id(**id, by_id).is_none_or(|root| roots.contains(&root)))
        .copied()
        .collect::<Vec<_>>();
    for id in drop_ids {
        by_id.remove(&id);
    }
}

async fn add_missing_descendants(
    state: &HandlerState,
    issue_id: Uuid,
    workspace_id: Uuid,
    by_id: &mut HashMap<Uuid, Comment>,
    affected_roots: &HashSet<Uuid>,
    through: &Comment,
    added: &mut usize,
) -> anyhow::Result<bool> {
    let mut frontier = affected_roots
        .iter()
        .map(|id| (*id, *id))
        .collect::<HashMap<_, _>>();
    let mut scan_budget = by_id.len() + COMMENT_THREAD_CONTEXT_BUDGET - *added;
    for depth in 0..=COMMENT_THREAD_MAX_DEPTH {
        let mut parent_ids = frontier.keys().copied().collect::<Vec<_>>();
        parent_ids.sort_unstable();
        let rows = comment::list_child_comments_for_parents(
            &state.pool,
            parent_ids,
            issue_id,
            workspace_id,
            Some(through.created_at),
            through.id,
            i32::try_from(scan_budget.saturating_add(1)).unwrap_or(i32::MAX),
        )
        .await?;
        if rows.is_empty() {
            return Ok(true);
        }
        if rows.len() > scan_budget || depth == COMMENT_THREAD_MAX_DEPTH {
            return Ok(false);
        }
        scan_budget -= rows.len();
        let mut next = HashMap::with_capacity(rows.len());
        for row in rows {
            let Some(parent_id) = row.parent_id else {
                return Ok(false);
            };
            let Some(root) = frontier.get(&parent_id).copied() else {
                return Ok(false);
            };
            next.insert(row.id, root);
            if by_id.contains_key(&row.id) {
                continue;
            }
            if *added >= COMMENT_THREAD_CONTEXT_BUDGET {
                return Ok(false);
            }
            *added += 1;
            by_id.insert(row.id, row);
        }
        frontier = next;
    }
    Ok(false)
}

pub(crate) async fn complete_comment_threads(
    state: &HandlerState,
    issue_id: Uuid,
    workspace_id: Uuid,
    window: Vec<Comment>,
) -> anyhow::Result<Vec<Comment>> {
    if window.is_empty() {
        return Ok(window);
    }
    let initial_ids = window.iter().map(|item| item.id).collect::<HashSet<_>>();
    let through = window
        .iter()
        .max_by_key(|item| (item.created_at, item.id))
        .cloned()
        .expect("non-empty comment window");
    let mut by_id = window
        .into_iter()
        .map(|item| (item.id, item))
        .collect::<HashMap<_, _>>();
    let mut added = 0usize;
    for _ in 0..COMMENT_THREAD_MAX_DEPTH {
        let missing = by_id
            .values()
            .filter_map(|item| item.parent_id)
            .filter(|parent_id| !by_id.contains_key(parent_id))
            .collect::<HashSet<_>>();
        if missing.is_empty() || added + missing.len() > COMMENT_THREAD_CONTEXT_BUDGET {
            break;
        }
        let rows = comment::list_comments_by_i_ds_for_issue(
            &state.pool,
            missing.into_iter().collect(),
            issue_id,
            workspace_id,
        )
        .await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            if let std::collections::hash_map::Entry::Vacant(entry) = by_id.entry(row.id) {
                entry.insert(row);
                added += 1;
            }
        }
    }
    by_id = keep_root_connected(by_id);
    let affected_roots = by_id
        .values()
        .filter(|item| item.parent_id.is_none() && !initial_ids.contains(&item.id))
        .map(|item| item.id)
        .collect::<HashSet<_>>();
    if !affected_roots.is_empty()
        && !add_missing_descendants(
            state,
            issue_id,
            workspace_id,
            &mut by_id,
            &affected_roots,
            &through,
            &mut added,
        )
        .await?
    {
        drop_threads(&mut by_id, &affected_roots);
    }
    let mut comments = by_id.into_values().collect::<Vec<_>>();
    comments.sort_by_key(|item| (item.created_at, item.id));
    Ok(comments)
}

fn summarize(content: &str) -> (String, bool) {
    match content.char_indices().nth(SUMMARY_CONTENT_RUNES) {
        Some((offset, _)) => (format!("{}…", &content[..offset]), true),
        None => (content.to_string(), false),
    }
}

fn fold_resolved(comments: Vec<Comment>) -> (Vec<Comment>, HashMap<Uuid, usize>) {
    let by_id = comments
        .iter()
        .map(|comment| (comment.id, comment))
        .collect::<HashMap<_, _>>();
    let root_of = |comment: &Comment| {
        let mut current = comment;
        for _ in 0..comments.len() {
            let Some(parent_id) = current.parent_id else {
                break;
            };
            let Some(parent) = by_id.get(&parent_id) else {
                break;
            };
            current = parent;
        }
        current.id
    };
    let mut threads: HashMap<Uuid, Vec<&Comment>> = HashMap::new();
    for item in &comments {
        threads.entry(root_of(item)).or_default().push(item);
    }
    let mut keep = HashSet::new();
    let mut stats = HashMap::new();
    for (root_id, items) in threads {
        let root = items
            .iter()
            .find(|item| item.id == root_id)
            .copied()
            .unwrap_or(items[0]);
        if root.resolved_at.is_some() {
            keep.insert(root.id);
            stats.insert(root.id, items.len().saturating_sub(1));
            continue;
        }
        let resolution = items
            .iter()
            .copied()
            .filter(|item| item.id != root.id && item.resolved_at.is_some())
            .max_by_key(|item| item.resolved_at);
        if let Some(resolution) = resolution {
            keep.insert(root.id);
            keep.insert(resolution.id);
            stats.insert(root.id, items.len().saturating_sub(2));
        } else {
            keep.extend(items.into_iter().map(|item| item.id));
        }
    }
    (
        comments
            .into_iter()
            .filter(|item| keep.contains(&item.id))
            .collect(),
        stats,
    )
}

pub(crate) async fn list_comments(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_issue): Path<String>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Response {
    let issue = match crate::issue::resolve_issue(&state, &context, &raw_issue).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let params = match parse_query(query) {
        Ok(params) => params,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let mut result = match fetch_comments(&state, issue.id, issue.workspace_id, &params).await {
        Ok(result) => result,
        Err(error) if error.to_string() == "thread-not-found" => {
            return error_response(
                StatusCode::NOT_FOUND,
                "thread anchor not found in this issue",
            )
        }
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to list comments");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list comments");
        }
    };
    let fold_stats = if params.fold && !result.fold_unsafe {
        let (comments, stats) = fold_resolved(result.comments);
        result.comments = comments;
        stats
    } else {
        HashMap::new()
    };
    let ids = result
        .comments
        .iter()
        .map(|item| item.id)
        .collect::<Vec<_>>();
    let reactions = reaction::list_reactions_by_comment_i_ds(&state.pool, ids.clone())
        .await
        .unwrap_or_default();
    let attachments =
        attachment::list_attachments_by_comment_i_ds(&state.pool, ids, issue.workspace_id)
            .await
            .unwrap_or_default();
    let mut reactions_by_id: HashMap<Uuid, Vec<Value>> = HashMap::new();
    for item in &reactions {
        reactions_by_id
            .entry(item.comment_id)
            .or_default()
            .push(crate::comment::reaction_json(item));
    }
    let mut attachments_by_id: HashMap<Uuid, Vec<Value>> = HashMap::new();
    for item in &attachments {
        if let Some(comment_id) = item.comment_id {
            attachments_by_id.entry(comment_id).or_default().push(
                serde_json::to_value(crate::issue::AttachmentResponse::for_request(
                    &state, item, &headers,
                ))
                .unwrap_or(Value::Null),
            );
        }
    }
    let mut body = Vec::with_capacity(result.comments.len());
    for item in &result.comments {
        let mut value = crate::comment::comment_json_with_related(
            item,
            Value::Array(reactions_by_id.remove(&item.id).unwrap_or_default()),
            Value::Array(attachments_by_id.remove(&item.id).unwrap_or_default()),
        );
        if let Value::Object(object) = &mut value {
            if let Some((reply_count, last_activity_at)) = result.root_stats.get(&item.id) {
                object.insert("reply_count".into(), json!(reply_count));
                object.insert(
                    "last_activity_at".into(),
                    json!(crate::timefmt::rfc3339(*last_activity_at)),
                );
            }
            if let Some(folded_count) = fold_stats.get(&item.id) {
                object.insert("thread_resolved".into(), json!(true));
                object.insert("folded_count".into(), json!(folded_count));
            }
            if params.summary {
                let (content, truncated) = summarize(&item.content);
                object.insert("content".into(), json!(content));
                object.insert("content_truncated".into(), json!(truncated));
            }
        }
        body.push(value);
    }
    let mut headers = HeaderMap::new();
    if let Some((at, id)) = result.next_before {
        headers.insert(
            HeaderName::from_static("x-cordy-next-before"),
            HeaderValue::from_str(&crate::timefmt::rfc3339_nano(at)).unwrap(),
        );
        headers.insert(
            HeaderName::from_static("x-cordy-next-before-id"),
            HeaderValue::from_str(&id.to_string()).unwrap(),
        );
    }
    if result.truncated {
        headers.insert(
            HeaderName::from_static("x-comments-truncated"),
            HeaderValue::from_static("true"),
        );
    }
    (headers, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fixture_comment(id: u128, parent_id: Option<u128>, resolved: bool) -> Comment {
        Comment {
            id: Uuid::from_u128(id),
            issue_id: Uuid::from_u128(100),
            author_type: "member".into(),
            author_id: Uuid::from_u128(101),
            content: format!("comment-{id}"),
            type_: "comment".into(),
            created_at: Utc.timestamp_opt(id as i64, 0).unwrap(),
            updated_at: Utc.timestamp_opt(id as i64, 0).unwrap(),
            parent_id: parent_id.map(Uuid::from_u128),
            workspace_id: Uuid::from_u128(102),
            resolved_at: resolved.then(|| Utc.timestamp_opt(id as i64 + 1, 0).unwrap()),
            resolved_by_type: resolved.then(|| "member".into()),
            resolved_by_id: resolved.then(|| Uuid::from_u128(101)),
            source_task_id: None,
            quick_action_id: None,
            via_plugin_id: None,
            revision: 1,
        }
    }

    #[test]
    fn rejects_partial_fold_combinations() {
        for query in [
            ListQuery {
                fold: Some("true".into()),
                since: Some("2026-08-23T00:00:00Z".into()),
                ..Default::default()
            },
            ListQuery {
                fold: Some("true".into()),
                tail: Some("1".into()),
                thread: Some(Uuid::nil().to_string()),
                ..Default::default()
            },
            ListQuery {
                fold: Some("true".into()),
                roots_only: Some("true".into()),
                ..Default::default()
            },
        ] {
            assert!(parse_query(query).is_err());
        }
    }

    #[test]
    fn summary_clips_on_unicode_scalar_boundary() {
        let content = "你".repeat(201);
        let (got, truncated) = summarize(&content);
        assert!(truncated);
        assert_eq!(got, format!("{}…", "你".repeat(200)));
    }

    #[test]
    fn aliases_and_zero_tail_are_preserved() {
        let params = parse_query(ListQuery {
            thread: Some(Uuid::nil().to_string()),
            tail: Some("0".into()),
            roots_only_alias: Some("false".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(params.tail, Some(0));
        assert!(!params.roots_only);
    }

    #[test]
    fn fold_keeps_root_and_latest_resolution_conclusion() {
        let comments = vec![
            fixture_comment(1, None, false),
            fixture_comment(2, Some(1), true),
            fixture_comment(3, Some(1), true),
            fixture_comment(4, Some(1), false),
        ];
        let (folded, stats) = fold_resolved(comments);
        assert_eq!(
            folded.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![Uuid::from_u128(1), Uuid::from_u128(3)]
        );
        assert_eq!(stats.get(&Uuid::from_u128(1)), Some(&2));
    }

    #[test]
    fn root_connectivity_drops_orphans_and_cycles() {
        let rows = vec![
            fixture_comment(1, None, false),
            fixture_comment(2, Some(1), false),
            fixture_comment(3, Some(99), false),
            fixture_comment(4, Some(5), false),
            fixture_comment(5, Some(4), false),
        ];
        let kept = keep_root_connected(rows.into_iter().map(|item| (item.id, item)).collect());
        assert_eq!(
            kept.keys().copied().collect::<HashSet<_>>(),
            HashSet::from([Uuid::from_u128(1), Uuid::from_u128(2)])
        );
    }
}
