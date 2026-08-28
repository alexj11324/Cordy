use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::Path;

use super::{format_table, value_string, Environment};

pub(super) fn copied_agent_max_concurrent_tasks(value: Option<&Value>) -> Option<i32> {
    let value = value?.as_f64()?;
    if value.fract() != 0.0 || !(1.0..=50.0).contains(&value) {
        return None;
    }
    Some(value as i32)
}

pub(super) fn apply_agent_permission_args(
    permission_mode: Option<&str>,
    public_to_workspace: Option<bool>,
    public_to_member: &[String],
    body: &mut serde_json::Map<String, Value>,
) {
    if permission_mode.is_none() && public_to_workspace.is_none() && public_to_member.is_empty() {
        return;
    }
    body.insert(
        "permission_mode".into(),
        Value::String(
            permission_mode
                .map(str::to_owned)
                .unwrap_or_else(|| "public_to".into()),
        ),
    );
    let mut targets = Vec::new();
    if public_to_workspace == Some(true) {
        targets.push(serde_json::json!({"target_type":"workspace"}));
    }
    targets.extend(
        public_to_member
            .iter()
            .map(|member| serde_json::json!({"target_type":"member","target_id":member})),
    );
    body.insert("invocation_targets".into(), Value::Array(targets));
}

pub(super) fn validate_agent_custom_env(value: &Value) -> Result<()> {
    let Some(object) = value.as_object() else {
        bail!("--custom-env must be a valid JSON object of string keys and string values");
    };
    if object.values().any(|value| !value.is_string()) {
        bail!("--custom-env must be a valid JSON object of string keys and string values");
    }
    Ok(())
}

pub(super) fn resolve_agent_secret_json<R: Read>(
    inline: Option<&str>,
    from_stdin: bool,
    file: Option<&Path>,
    flag: &str,
    allow_null: bool,
    environment: &Environment,
    input: &mut R,
) -> Result<Option<Value>> {
    let count =
        usize::from(inline.is_some()) + usize::from(from_stdin) + usize::from(file.is_some());
    if count == 0 {
        return Ok(None);
    }
    if count > 1 {
        bail!("--{flag}, --{flag}-stdin, and --{flag}-file are mutually exclusive; pick one");
    }
    let raw = if let Some(raw) = inline {
        raw.to_string()
    } else if from_stdin {
        let mut raw = String::new();
        input
            .read_to_string(&mut raw)
            .with_context(|| format!("read --{flag}-stdin"))?;
        if raw.trim().is_empty() {
            if allow_null {
                bail!("--{flag}-stdin: empty input; pass 'null' to clear");
            }
            bail!("--{flag}-stdin: empty input; pass '{{}}' to clear");
        }
        raw
    } else {
        let path = file.context("secret file path")?;
        if path.as_os_str().is_empty() {
            bail!("--{flag}-file: path must not be empty");
        }
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            environment.current_dir().join(path)
        };
        let raw = fs::read_to_string(&path).with_context(|| format!("read --{flag}-file"))?;
        if raw.trim().is_empty() {
            if allow_null {
                bail!(
                    "--{flag}-file {:?}: empty contents; pass 'null' to clear",
                    path
                );
            }
            bail!(
                "--{flag}-file {:?}: empty contents; pass '{{}}' to clear",
                path
            );
        }
        raw
    };
    if raw.trim().is_empty() {
        if allow_null {
            bail!("--{flag}: empty input; pass 'null' to clear or a JSON object to set");
        }
        bail!("--{flag}: empty input; pass '{{}}' to clear");
    }
    let value: Value = serde_json::from_str(raw.trim()).map_err(|_| {
        if allow_null {
            anyhow::anyhow!("--{flag} must be a valid JSON object, or 'null' to clear")
        } else {
            anyhow::anyhow!("--{flag} must be a valid JSON object of string keys and string values")
        }
    })?;
    if value.is_null() && allow_null {
        return Ok(Some(value));
    }
    if value.is_null() {
        return Ok(Some(Value::Object(serde_json::Map::new())));
    }
    if !value.is_object() {
        if allow_null {
            bail!("--{flag} must be a valid JSON object, or 'null' to clear");
        }
        bail!("--{flag} must be a valid JSON object of string keys and string values");
    }
    Ok(Some(value))
}

pub(super) fn format_agent_list_table(agents: &[Value]) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "NAME".into(),
        "STATUS".into(),
        "RUNTIME".into(),
        "ARCHIVED".into(),
    ]];
    rows.extend(agents.iter().map(|agent| {
        vec![
            value_string(agent, "id"),
            value_string(agent, "name"),
            value_string(agent, "status"),
            value_string(agent, "runtime_mode"),
            if value_string(agent, "archived_at").is_empty() {
                String::new()
            } else {
                "yes".into()
            },
        ]
    }));
    format_table(&rows)
}

pub(super) fn format_agent_details_table(agent: &Value) -> String {
    format_table(&[
        vec![
            "ID".into(),
            "NAME".into(),
            "STATUS".into(),
            "RUNTIME".into(),
            "VISIBILITY".into(),
            "AVATAR_URL".into(),
            "DESCRIPTION".into(),
        ],
        vec![
            value_string(agent, "id"),
            value_string(agent, "name"),
            value_string(agent, "status"),
            value_string(agent, "runtime_mode"),
            value_string(agent, "visibility"),
            value_string(agent, "avatar_url"),
            value_string(agent, "description"),
        ],
    ])
}
