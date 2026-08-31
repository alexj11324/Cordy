use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{
    format_table, load_issue_actor_names, new_api_client, resolve_current_workspace_id,
    resolve_issue_ref, resolve_subscriber_id, resolve_subscriber_name, value_string, Cli,
    Environment, IssueActorNames, IssueSubscriberMutationArgs, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_subscriber_list(
    cli: &Cli,
    environment: &Environment,
    issue_ref: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, issue_ref)
        .await
        .context("resolve issue")?;
    let subscribers: Vec<Value> = client
        .get_json(&format!("/api/issues/{issue_id}/subscribers"))
        .await
        .context("list subscribers")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&subscribers)?),
        OutputFormat::Table => {
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let synthetic = subscribers
                .iter()
                .map(|subscriber| {
                    serde_json::json!({
                        "executor_type": subscriber.get("user_type").cloned().unwrap_or(Value::Null),
                        "executor_id": subscriber.get("user_id").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect::<Vec<_>>();
            let actors = load_issue_actor_names(&client, &workspace_id, &synthetic).await;
            format_issue_subscribers_table(&subscribers, &actors)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) fn format_issue_subscribers_table(
    subscribers: &[Value],
    actors: &IssueActorNames,
) -> String {
    let mut rows = vec![vec!["USER".into(), "REASON".into(), "CREATED".into()]];
    for subscriber in subscribers {
        let actor_type = value_string(subscriber, "user_type");
        let actor_id = value_string(subscriber, "user_id");
        let actor_key = format!("{actor_type}:{actor_id}");
        let actor = actors
            .0
            .get(&actor_key)
            .map_or(actor_key, |name| format!("{actor_type}:{name}"));
        rows.push(vec![
            actor,
            value_string(subscriber, "reason"),
            value_string(subscriber, "created_at")
                .chars()
                .take(16)
                .collect(),
        ]);
    }
    format_table(&rows)
}

pub(super) async fn run_issue_subscriber_mutation(
    cli: &Cli,
    environment: &Environment,
    args: &IssueSubscriberMutationArgs,
    subscribe: bool,
) -> Result<RunOutput> {
    if args.user.is_some() && args.user_id.is_some() {
        bail!("--user and --user-id are mutually exclusive");
    }
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let resolved = if let Some(user_id) = &args.user_id {
        Some(
            resolve_subscriber_id(&client, &workspace_id, user_id)
                .await
                .context("resolve user")?,
        )
    } else if let Some(user) = &args.user {
        Some(
            resolve_subscriber_name(&client, &workspace_id, user)
                .await
                .context("resolve user")?,
        )
    } else {
        None
    };
    let mut body = serde_json::Map::new();
    if let Some(actor) = &resolved {
        body.insert("user_type".into(), Value::String(actor.actor_type.clone()));
        body.insert("user_id".into(), Value::String(actor.id.clone()));
    }
    let action = if subscribe {
        "subscribe"
    } else {
        "unsubscribe"
    };
    let result: Value = client
        .post_json(&format!("/api/issues/{issue_id}/{action}"), &body)
        .await
        .with_context(|| format!("{action} issue"))?;
    let target = if let Some(user) = args.user.as_deref() {
        user.into()
    } else if let Some(actor) = resolved {
        if actor.name.is_empty() {
            format!("{}:{}", actor.actor_type, actor.id)
        } else {
            format!("{}:{}", actor.actor_type, actor.name)
        }
    } else {
        "caller".into()
    };
    let verb = if subscribe {
        "Subscribed"
    } else {
        "Unsubscribed"
    };
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => String::new(),
        },
        stderr: format!("{verb} {target} to issue {}.\n", args.issue_id),
    })
}
