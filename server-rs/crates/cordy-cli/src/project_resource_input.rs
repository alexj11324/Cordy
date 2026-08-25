use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{ProjectResourceAddArgs, ProjectResourceUpdateArgs};

fn parse_generic_resource_ref(raw: &str) -> Result<Value> {
    serde_json::from_str(raw).map_err(|error| anyhow::anyhow!("--ref is not valid JSON: {error}"))
}

pub(super) fn build_project_resource_add_ref(args: &ProjectResourceAddArgs) -> Result<Value> {
    let resource_type = args.resource_type.trim();
    if resource_type.is_empty() {
        bail!("--type is required");
    }
    if let Some(raw) = &args.resource_ref {
        let raw = raw.trim();
        if !raw.is_empty()
            && (resource_type != "github_repo" || raw.starts_with('{') || raw.starts_with('['))
        {
            return parse_generic_resource_ref(raw);
        }
        if resource_type != "github_repo" {
            bail!("--ref must be a JSON resource_ref payload for resource type {resource_type:?}");
        }
    }
    match resource_type {
        "github_repo" => {
            let url = args
                .url
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .context("github_repo requires --url (or pass a JSON payload via --ref)")?;
            let mut resource_ref = serde_json::Map::from_iter([(
                "url".into(),
                Value::String(url.into()),
            )]);
            if let Some(hint) = args
                .default_branch_hint
                .as_deref()
                .map(str::trim)
                .filter(|hint| !hint.is_empty())
            {
                resource_ref.insert("default_branch_hint".into(), Value::String(hint.into()));
            }
            if let Some(checkout_ref) = args
                .resource_ref
                .as_deref()
                .map(str::trim)
                .filter(|checkout_ref| !checkout_ref.is_empty())
            {
                resource_ref.insert("ref".into(), Value::String(checkout_ref.into()));
            }
            Ok(Value::Object(resource_ref))
        }
        "local_directory" => {
            let local_path = args
                .local_path
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty());
            let daemon_id = args
                .daemon_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty());
            let (Some(local_path), Some(daemon_id)) = (local_path, daemon_id) else {
                bail!("local_directory requires --local-path and --daemon-id (or pass a JSON payload via --ref)");
            };
            let mut resource_ref = serde_json::Map::from_iter([
                ("local_path".into(), Value::String(local_path.into())),
                ("daemon_id".into(), Value::String(daemon_id.into())),
            ]);
            for (key, value) in [
                ("label", args.ref_label.as_deref()),
                ("execution_mode", args.execution_mode.as_deref()),
            ] {
                if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
                    resource_ref.insert(key.into(), Value::String(value.into()));
                }
            }
            Ok(Value::Object(resource_ref))
        }
        _ => bail!(
            "type {resource_type:?} has no built-in CLI shortcut; pass the payload via --ref '<json>'"
        ),
    }
}

fn seed_resource_ref(
    existing: Option<&serde_json::Map<String, Value>>,
    keys: &[&str],
) -> serde_json::Map<String, Value> {
    let mut resource_ref = serde_json::Map::new();
    if let Some(existing) = existing {
        for key in keys {
            if let Some(value) = existing
                .get(*key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                resource_ref.insert((*key).into(), Value::String(value.into()));
            }
        }
    }
    resource_ref
}

pub(super) fn build_project_resource_update_ref(
    args: &ProjectResourceUpdateArgs,
    resource_type: &str,
    existing: Option<&serde_json::Map<String, Value>>,
) -> Result<Option<Value>> {
    if let Some(raw) = &args.resource_ref {
        let raw = raw.trim();
        if !raw.is_empty()
            && (resource_type != "github_repo" || raw.starts_with('{') || raw.starts_with('['))
        {
            return parse_generic_resource_ref(raw).map(Some);
        }
        if resource_type != "github_repo" {
            bail!("--ref must be a JSON resource_ref payload for resource type {resource_type:?}");
        }
    }
    match resource_type {
        "github_repo" => {
            if args.url.is_none()
                && args.default_branch_hint.is_none()
                && args.resource_ref.is_none()
            {
                return Ok(None);
            }
            let mut resource_ref =
                seed_resource_ref(existing, &["url", "default_branch_hint", "ref"]);
            if let Some(url) = &args.url {
                let url = url.trim();
                if url.is_empty() {
                    bail!("--url cannot be empty");
                }
                resource_ref.insert("url".into(), Value::String(url.into()));
            }
            for (key, value) in [
                ("default_branch_hint", args.default_branch_hint.as_deref()),
                ("ref", args.resource_ref.as_deref()),
            ] {
                if let Some(value) = value {
                    let value = value.trim();
                    if value.is_empty() {
                        resource_ref.remove(key);
                    } else {
                        resource_ref.insert(key.into(), Value::String(value.into()));
                    }
                }
            }
            if !resource_ref.contains_key("url") {
                bail!("github_repo: --url is required (no existing url to merge with)");
            }
            Ok(Some(Value::Object(resource_ref)))
        }
        "local_directory" => {
            if args.local_path.is_none()
                && args.daemon_id.is_none()
                && args.ref_label.is_none()
                && args.execution_mode.is_none()
            {
                return Ok(None);
            }
            let mut resource_ref = seed_resource_ref(
                existing,
                &["local_path", "daemon_id", "label", "execution_mode"],
            );
            for (flag, key, value) in [
                ("--local-path", "local_path", args.local_path.as_deref()),
                ("--daemon-id", "daemon_id", args.daemon_id.as_deref()),
            ] {
                if let Some(value) = value {
                    let value = value.trim();
                    if value.is_empty() {
                        bail!("{flag} cannot be empty");
                    }
                    resource_ref.insert(key.into(), Value::String(value.into()));
                }
            }
            for (key, value) in [
                ("label", args.ref_label.as_deref()),
                ("execution_mode", args.execution_mode.as_deref()),
            ] {
                if let Some(value) = value {
                    let value = value.trim();
                    if value.is_empty() {
                        resource_ref.remove(key);
                    } else {
                        resource_ref.insert(key.into(), Value::String(value.into()));
                    }
                }
            }
            if !resource_ref.contains_key("local_path") {
                bail!("local_directory: --local-path is required (no existing local_path to merge with)");
            }
            if !resource_ref.contains_key("daemon_id") {
                bail!("local_directory: --daemon-id is required (no existing daemon_id to merge with)");
            }
            Ok(Some(Value::Object(resource_ref)))
        }
        _ => {
            if args.url.is_some()
                || args.default_branch_hint.is_some()
                || args.local_path.is_some()
                || args.daemon_id.is_some()
                || args.ref_label.is_some()
                || args.execution_mode.is_some()
            {
                bail!(
                    "no built-in shortcut for resource type {resource_type:?}; pass the full payload via --ref '<json>'"
                );
            }
            Ok(None)
        }
    }
}
