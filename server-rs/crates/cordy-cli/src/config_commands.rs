use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fmt::Write;
use std::path::Path;
use url::Url;

use super::{
    lexical_normalize, require_task_local_config_root, Cli, Environment, OutputFormat, RunOutput,
};

pub(super) const CONFIG_SET_SUPPORTED_KEYS: &[&str] = &[
    "server_url",
    "app_url",
    "workspace_id",
    "device_name",
    "runtime_name",
    "workspaces_root",
    "max_concurrent_tasks",
    "poll_interval",
    "heartbeat_interval",
    "agent_timeout",
    "codex_semantic_inactivity_timeout",
    "codex_handshake_timeout",
    "disable_auto_update",
    "auto_update_check_interval",
    "disable_auto_reload",
];

pub(super) fn run_config_show(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    require_task_local_config_root(environment)?;
    let path = environment.config_path(&cli.profile)?;
    let document = environment.load_profile_document(&cli.profile)?;
    let values = config_display_values(&document)?;
    let stdout = match output {
        OutputFormat::Table => format_config_table(&path, &cli.profile, &values),
        OutputFormat::Json => {
            let mut object = serde_json::Map::new();
            object.insert(
                "config_file".into(),
                Value::String(path.display().to_string()),
            );
            if !cli.profile.is_empty() {
                object.insert("profile".into(), Value::String(cli.profile.clone()));
            }
            for (key, value) in values {
                object.insert(key.into(), value);
            }
            format!(
                "{}\n",
                serde_json::to_string_pretty(&Value::Object(object))?
            )
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) fn run_config_set(
    cli: &Cli,
    environment: &Environment,
    key: &str,
    value: &str,
) -> Result<RunOutput> {
    require_task_local_config_root(environment)?;
    let (stored, displayed) = validate_config_set(key, value, environment)?;
    environment.set_profile_value(&cli.profile, key, stored)?;
    Ok(RunOutput {
        stdout: String::new(),
        stderr: format!("Set {key} = {displayed}\n"),
    })
}

pub(super) fn config_display_values(document: &Value) -> Result<Vec<(&'static str, Value)>> {
    let object = document
        .as_object()
        .context("parse CLI config: expected a JSON object")?;
    let string = |key: &'static str| -> Result<Value> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(Value::Null),
            Some(Value::String(value)) if value.is_empty() => Ok(Value::Null),
            Some(Value::String(value)) => Ok(Value::String(value.clone())),
            Some(_) => bail!("parse CLI config: field {key:?} must be a string"),
        }
    };
    let integer = |key: &'static str| -> Result<Value> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(Value::Null),
            Some(Value::Number(value)) if value.as_i64() == Some(0) => Ok(Value::Null),
            Some(Value::Number(value)) if value.as_i64().is_some() => {
                Ok(Value::Number(value.clone()))
            }
            Some(_) => bail!("parse CLI config: field {key:?} must be an integer"),
        }
    };
    let boolean = |key: &'static str| -> Result<Value> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(Value::Bool(false)),
            Some(Value::Bool(value)) => Ok(Value::Bool(*value)),
            Some(_) => bail!("parse CLI config: field {key:?} must be a boolean"),
        }
    };
    Ok(vec![
        ("server_url", string("server_url")?),
        ("app_url", string("app_url")?),
        ("workspace_id", string("workspace_id")?),
        ("device_name", string("device_name")?),
        ("runtime_name", string("runtime_name")?),
        ("workspaces_root", string("workspaces_root")?),
        ("max_concurrent_tasks", integer("max_concurrent_tasks")?),
        ("poll_interval", string("poll_interval")?),
        ("heartbeat_interval", string("heartbeat_interval")?),
        ("agent_timeout", string("agent_timeout")?),
        (
            "codex_semantic_inactivity_timeout",
            string("codex_semantic_inactivity_timeout")?,
        ),
        (
            "codex_handshake_timeout",
            string("codex_handshake_timeout")?,
        ),
        ("disable_auto_update", boolean("disable_auto_update")?),
        (
            "auto_update_check_interval",
            string("auto_update_check_interval")?,
        ),
        ("disable_auto_reload", boolean("disable_auto_reload")?),
    ])
}

pub(super) fn format_config_table(path: &Path, profile: &str, values: &[(&str, Value)]) -> String {
    let mut output = format!("Config file: {}\n", path.display());
    if !profile.is_empty() {
        let _ = writeln!(output, "Profile:      {profile}");
    }
    for (key, value) in values {
        let rendered = match (*key, value) {
            ("agent_timeout", Value::String(value))
                if parse_go_duration(value).is_some_and(|duration| duration == 0.0) =>
            {
                format!("{value} (disabled)")
            }
            (_, Value::String(value)) => value.clone(),
            (_, Value::Bool(value)) => value.to_string(),
            (_, Value::Number(value)) => value.to_string(),
            _ => "(not set)".into(),
        };
        let label = format!("{key}:");
        let _ = writeln!(output, "{label:<34} {rendered}");
    }
    output
}

pub(super) fn validate_config_set(
    key: &str,
    value: &str,
    environment: &Environment,
) -> Result<(Option<Value>, String)> {
    let clear = || (None, String::new());
    match key {
        "server_url" => validate_url_config(value, key, &["http", "https", "ws", "wss"]),
        "app_url" => validate_url_config(value, key, &["http", "https"]),
        "workspace_id" | "device_name" | "runtime_name" => Ok(if value.is_empty() {
            clear()
        } else {
            (Some(Value::String(value.into())), value.into())
        }),
        "workspaces_root" => {
            let value = value.trim();
            if value.is_empty() {
                return Ok(clear());
            }
            let path = Path::new(value);
            let absolute = if path.is_absolute() {
                lexical_normalize(path)
            } else {
                lexical_normalize(&environment.current_dir().join(path))
            };
            let value = absolute.display().to_string();
            Ok((Some(Value::String(value.clone())), value))
        }
        "max_concurrent_tasks" => {
            if value.is_empty() {
                return Ok(clear());
            }
            let number = value.parse::<i64>().with_context(|| {
                format!("max_concurrent_tasks must be an integer: invalid value {value:?}")
            })?;
            if number < 0 {
                bail!("max_concurrent_tasks must be >= 0 (got {number})");
            }
            Ok(if number == 0 {
                clear()
            } else {
                (Some(Value::Number(number.into())), value.into())
            })
        }
        "poll_interval" => validate_positive_duration(key, value, false),
        "heartbeat_interval"
        | "codex_semantic_inactivity_timeout"
        | "codex_handshake_timeout"
        | "auto_update_check_interval" => validate_positive_duration(key, value, true),
        "agent_timeout" => {
            if value.is_empty() {
                return Ok(clear());
            }
            let duration = parse_go_duration(value).with_context(|| {
                format!(
                    "agent_timeout must be a Go duration (e.g. 10m, 0s to disable): invalid value {value:?}"
                )
            })?;
            if duration < 0.0 {
                bail!(
                    "agent_timeout must be >= 0 (got {value}); use 0s to disable the cap or \"\" to clear the persisted value"
                );
            }
            Ok((Some(Value::String(value.into())), value.into()))
        }
        "disable_auto_update" | "disable_auto_reload" => {
            if value.is_empty() {
                return Ok(clear());
            }
            let parsed = parse_go_bool(value)
                .with_context(|| format!("{key} must be 'true' or 'false' (got {value:?})"))?;
            Ok(if parsed {
                (Some(Value::Bool(true)), value.into())
            } else {
                clear()
            })
        }
        _ => bail!(
            "unknown config key {key:?} (supported: {})",
            CONFIG_SET_SUPPORTED_KEYS.join(", ")
        ),
    }
}

fn validate_url_config(
    value: &str,
    key: &str,
    schemes: &[&str],
) -> Result<(Option<Value>, String)> {
    if value.is_empty() {
        return Ok((None, String::new()));
    }
    let url = Url::parse(value).with_context(|| format!("{key} must be a valid URL"))?;
    if url.host_str().is_none() {
        bail!("{key} must be a valid URL with a host");
    }
    if !schemes.contains(&url.scheme()) {
        bail!("{key} must use one of: {}", schemes.join(", "));
    }
    Ok((Some(Value::String(value.into())), value.into()))
}

fn validate_positive_duration(
    key: &str,
    value: &str,
    trim: bool,
) -> Result<(Option<Value>, String)> {
    if value.is_empty() {
        return Ok((None, String::new()));
    }
    let stored = if trim { value.trim() } else { value };
    let duration = parse_go_duration(stored).with_context(|| {
        format!("{key} must be a Go duration (e.g. 10s, 500ms): invalid value {value:?}")
    })?;
    if duration <= 0.0 {
        bail!("{key} must be positive (got {stored}); use `config set {key} \"\"` to clear it");
    }
    Ok((Some(Value::String(stored.into())), stored.into()))
}

fn parse_go_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

fn parse_go_duration(value: &str) -> Option<f64> {
    if value.is_empty() || value.trim() != value {
        return None;
    }
    let (sign, mut rest) = match value.as_bytes().first() {
        Some(b'-') => (-1.0, &value[1..]),
        Some(b'+') => (1.0, &value[1..]),
        _ => (1.0, value),
    };
    if rest.is_empty() {
        return None;
    }
    if rest == "0" {
        return Some(0.0 * sign);
    }
    let mut seconds = 0.0_f64;
    while !rest.is_empty() {
        let number_len = rest
            .char_indices()
            .take_while(|(_, character)| character.is_ascii_digit() || *character == '.')
            .map(|(index, character)| index + character.len_utf8())
            .last()?;
        let number = rest[..number_len].parse::<f64>().ok()?;
        rest = &rest[number_len..];
        let (unit, multiplier) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ]
        .into_iter()
        .find(|(unit, _)| rest.starts_with(unit))?;
        rest = &rest[unit.len()..];
        seconds += number * multiplier;
    }
    const MAX_GO_DURATION_SECONDS: f64 = i64::MAX as f64 / 1_000_000_000.0;
    (seconds.is_finite() && seconds <= MAX_GO_DURATION_SECONDS).then_some(sign * seconds)
}
