use anyhow::{Context, Result};
use serde_json::Value;
use url::form_urlencoded;

use super::{
    format_table, new_api_client, value_string, ChatReadArgs, Cli, Environment, OutputFormat,
    RunOutput,
};

fn chat_reply_count(message: &Value) -> String {
    message
        .get("reply_count")
        .and_then(Value::as_f64)
        .filter(|count| *count != 0.0)
        .map(|count| (count as i64).to_string())
        .unwrap_or_default()
}

fn format_chat_read(response: &Value, output: OutputFormat, overview: bool) -> Result<String> {
    if output == OutputFormat::Json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(response)?));
    }
    if let Some(note) = response
        .get("note")
        .and_then(Value::as_str)
        .filter(|note| !note.is_empty())
    {
        return Ok(format!("{note}\n"));
    }
    let messages = response
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut rows = vec![if overview {
        vec![
            "TS".into(),
            "ROLE".into(),
            "AUTHOR".into(),
            "THREAD_ID".into(),
            "REPLIES".into(),
            "TEXT".into(),
        ]
    } else {
        vec!["TS".into(), "ROLE".into(), "AUTHOR".into(), "TEXT".into()]
    }];
    rows.extend(messages.iter().map(|message| {
        let mut row = vec![
            value_string(message, "ts"),
            value_string(message, "role"),
            value_string(message, "author"),
        ];
        if overview {
            row.push(value_string(message, "thread_id"));
            row.push(chat_reply_count(message));
        }
        row.push(value_string(message, "text"));
        row
    }));
    Ok(format_table(&rows))
}

pub(super) async fn run_chat_read(
    cli: &Cli,
    environment: &Environment,
    base_path: &str,
    thread_id: Option<&str>,
    args: &ChatReadArgs,
    overview: bool,
) -> Result<RunOutput> {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    if let Some(before) = args.before.as_deref().filter(|before| !before.is_empty()) {
        serializer.append_pair("before", before);
    }
    if let Some(thread_id) = thread_id.filter(|thread_id| !thread_id.is_empty()) {
        serializer.append_pair("id", thread_id);
    }
    if args.limit > 0 {
        serializer.append_pair("limit", &args.limit.to_string());
    }
    let query = serializer.finish();
    let path = if query.is_empty() {
        base_path.into()
    } else {
        format!("{base_path}?{query}")
    };
    let client = new_api_client(cli, environment)?;
    let response: Value = client.get_json(&path).await.context("read chat")?;
    Ok(RunOutput {
        stdout: format_chat_read(&response, args.output, overview)?,
        stderr: String::new(),
    })
}
