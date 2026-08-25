use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fmt::Write;
use url::form_urlencoded;

use super::{
    format_table, load_issue_actor_names, new_api_client, resolve_current_workspace_id,
    resolve_issue_ref, value_string, Cli, Environment, IssueActorNames, IssueCommentListArgs,
    OutputFormat, RunOutput,
};

pub(super) async fn run_issue_comment_list(
    cli: &Cli,
    environment: &Environment,
    args: &IssueCommentListArgs,
) -> Result<RunOutput> {
    let since = args.since.as_deref().unwrap_or_default();
    let thread = args.thread.as_deref().unwrap_or_default();
    let before = args.before.as_deref().unwrap_or_default();
    let before_id = args.before_id.as_deref().unwrap_or_default();
    if args.recent.is_some_and(|value| value <= 0) {
        bail!("--recent must be a positive integer");
    }
    if args.tail.is_some_and(|value| value < 0) {
        bail!("--tail must be a non-negative integer (0 returns just the thread root)");
    }
    if !thread.is_empty() && args.recent.is_some() {
        bail!("--thread and --recent are mutually exclusive");
    }
    if args.roots_only && !thread.is_empty() {
        bail!("--roots-only and --thread are mutually exclusive");
    }
    if args.roots_only && args.recent.is_some() {
        bail!("--roots-only and --recent are mutually exclusive");
    }
    if args.roots_only && args.tail.is_some() {
        bail!("--roots-only and --tail are mutually exclusive");
    }
    if args.roots_only && !before.is_empty() {
        bail!("--roots-only does not support --before / --before-id");
    }
    if args.tail.is_some() && thread.is_empty() {
        bail!("--tail requires --thread (it is a thread-scoped limit)");
    }
    if before.is_empty() != before_id.is_empty() {
        bail!("--before and --before-id must be set together (composite cursor for stable pagination)");
    }
    if !before.is_empty() && args.recent.is_none() && !(args.tail.is_some() && !thread.is_empty()) {
        bail!("--before / --before-id require --recent (thread cursor) or --thread + --tail (reply cursor)");
    }

    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    if !since.is_empty() {
        serializer.append_pair("since", since);
    }
    if args.roots_only {
        serializer.append_pair("roots_only", "true");
    }
    if args.summary {
        serializer.append_pair("summary", "true");
    }
    let fold_eligible = !args.roots_only && since.is_empty() && args.tail.is_none();
    if fold_eligible && !args.full {
        serializer.append_pair("fold", "true");
    }
    if !thread.is_empty() {
        serializer.append_pair("thread", thread);
    }
    if let Some(tail) = args.tail {
        serializer.append_pair("tail", &tail.to_string());
    }
    if let Some(recent) = args.recent {
        serializer.append_pair("recent", &recent.to_string());
    }
    if !before.is_empty() {
        serializer.append_pair("before", before);
        serializer.append_pair("before_id", before_id);
    }
    let query = serializer.finish();
    let path = if query.is_empty() {
        format!("/api/issues/{issue_id}/comments")
    } else {
        format!("/api/issues/{issue_id}/comments?{query}")
    };
    let (mut comments, headers): (Vec<Value>, _) = client
        .get_json_with_headers(&path)
        .await
        .context("list comments")?;
    let mut stderr = String::new();
    let next_before = headers
        .get("X-Cordy-Next-Before")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let next_before_id = headers
        .get("X-Cordy-Next-Before-Id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !next_before.is_empty() && !next_before_id.is_empty() {
        let label = if !thread.is_empty() && args.tail.is_some() {
            "Next reply cursor"
        } else {
            "Next thread cursor"
        };
        let _ = writeln!(
            stderr,
            "{label}: --before {next_before} --before-id {next_before_id}"
        );
    }

    let stdout = match args.output {
        OutputFormat::Json => {
            if args.compact {
                compact_issue_comments(&mut comments);
            }
            format!("{}\n", serde_json::to_string_pretty(&comments)?)
        }
        OutputFormat::Table => {
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let actors = load_comment_actor_names(&client, &workspace_id, &comments).await;
            format_issue_comments_table(&comments, &actors)
        }
    };
    Ok(RunOutput { stdout, stderr })
}

fn compact_issue_comments(comments: &mut [Value]) {
    for comment in comments {
        let Some(object) = comment.as_object_mut() else {
            continue;
        };
        object.remove("issue_id");
        object.remove("source_task_id");
        if object.get("updated_at") == object.get("created_at") {
            object.remove("updated_at");
        }
        object.retain(|_, value| match value {
            Value::Null => false,
            Value::Array(items) => !items.is_empty(),
            _ => true,
        });
    }
}

async fn load_comment_actor_names(
    client: &super::ApiClient,
    workspace_id: &str,
    comments: &[Value],
) -> IssueActorNames {
    let synthetic_issues = comments
        .iter()
        .map(|comment| {
            serde_json::json!({
                "assignee_type": comment.get("author_type").cloned().unwrap_or(Value::Null),
                "assignee_id": comment.get("author_id").cloned().unwrap_or(Value::Null)
            })
        })
        .collect::<Vec<_>>();
    load_issue_actor_names(client, workspace_id, &synthetic_issues).await
}

pub(super) fn format_issue_comments_table(comments: &[Value], actors: &IssueActorNames) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "PARENT".into(),
        "AUTHOR".into(),
        "TYPE".into(),
        "CONTENT".into(),
        "CREATED".into(),
    ]];
    for comment in comments {
        let content = value_string(comment, "content");
        let content = if content.chars().count() > 80 {
            format!("{}...", content.chars().take(77).collect::<String>())
        } else {
            content
        };
        let created = value_string(comment, "created_at")
            .chars()
            .take(16)
            .collect::<String>();
        let parent = match value_string(comment, "parent_id") {
            value if value.is_empty() => "—".into(),
            value => value,
        };
        let actor_type = value_string(comment, "author_type");
        let actor_id = value_string(comment, "author_id");
        let author = if actor_type.is_empty() || actor_id.is_empty() {
            String::new()
        } else {
            let actor_key = format!("{actor_type}:{actor_id}");
            actors
                .0
                .get(&actor_key)
                .map_or(actor_key, |name| format!("{actor_type}:{name}"))
        };
        rows.push(vec![
            value_string(comment, "id"),
            parent,
            author,
            value_string(comment, "type"),
            content,
            created,
        ]);
    }
    format_table(&rows)
}
