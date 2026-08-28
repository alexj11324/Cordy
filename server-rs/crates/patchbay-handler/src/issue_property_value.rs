//! Typed issue-property value validation shared by the HTTP write boundary.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use patchbay_db::models::IssueProperty;
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;
use uuid::Uuid;

const MAX_TEXT_CHARS: usize = 2_000;
const MAX_URL_BYTES: usize = 2_048;
const MAX_ACTORS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActorRef {
    pub(crate) value: String,
    pub(crate) user_id: Uuid,
}

#[derive(Debug, Default, Deserialize)]
struct PropertyConfig {
    #[serde(default)]
    options: Vec<PropertyOption>,
}

#[derive(Debug, Default, Deserialize)]
struct PropertyOption {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
}

fn config(definition: &IssueProperty) -> PropertyConfig {
    serde_json::from_value(definition.config.clone()).unwrap_or_default()
}

fn option_order(config: &PropertyConfig) -> HashMap<&str, usize> {
    config
        .options
        .iter()
        .enumerate()
        .map(|(index, option)| (option.id.as_str(), index))
        .collect()
}

fn options_hint(config: &PropertyConfig) -> String {
    config
        .options
        .iter()
        .map(|option| format!("{} ({})", option.id, option.name))
        .collect::<Vec<_>>()
        .join(", ")
}

fn actor_ref(raw: &str) -> Result<ActorRef, String> {
    let Some((kind, id)) = raw.split_once(':') else {
        return Err("value must look like \"<kind>:<uuid>\" where kind is one of: member".into());
    };
    if kind != "member" {
        return Err(format!("unknown actor kind {kind:?}; valid kinds: member"));
    }
    let user_id = Uuid::parse_str(id).map_err(|_| format!("actor id in {raw:?} must be a UUID"))?;
    Ok(ActorRef {
        value: format!("member:{user_id}"),
        user_id,
    })
}

fn actor_refs(value: &Value, multiple: bool) -> Result<Vec<ActorRef>, String> {
    if !multiple {
        let raw = value.as_str().ok_or_else(|| {
            "value must be an actor reference string like \"member:<uuid>\" (kinds: member)"
                .to_string()
        })?;
        return actor_ref(raw).map(|reference| vec![reference]);
    }
    let items = value.as_array().ok_or_else(|| {
        "value must be an array of actor reference strings like \"member:<uuid>\" (kinds: member)"
            .to_string()
    })?;
    if items.is_empty() {
        return Err("value must be a non-empty array of actor references".into());
    }
    if items.len() > MAX_ACTORS {
        return Err(format!("value cannot list more than {MAX_ACTORS} actors"));
    }
    let mut seen = HashSet::new();
    let mut references = Vec::with_capacity(items.len());
    for item in items {
        let raw = item
            .as_str()
            .ok_or_else(|| "value must be an array of actor reference strings".to_string())?;
        let reference = actor_ref(raw)?;
        if seen.insert(reference.value.clone()) {
            references.push(reference);
        }
    }
    Ok(references)
}

pub(crate) fn validate(
    definition: &IssueProperty,
    value: &Value,
) -> Result<(Value, Vec<ActorRef>), String> {
    if value.is_null() {
        return Err("value cannot be null (use DELETE to unset a property)".into());
    }
    let empty_refs = Vec::new();
    match definition.type_.as_str() {
        "text" => {
            let text = value
                .as_str()
                .ok_or_else(|| "value must be a string".to_string())?;
            if text.trim().is_empty() {
                return Err("value cannot be empty (use DELETE to unset a property)".into());
            }
            if text.chars().count() > MAX_TEXT_CHARS {
                return Err(format!(
                    "value must be {MAX_TEXT_CHARS} characters or fewer"
                ));
            }
            Ok((json!(text.replace('\0', "")), empty_refs))
        }
        "url" => {
            let raw = value
                .as_str()
                .ok_or_else(|| "value must be a URL string".to_string())?;
            let raw = raw.trim();
            if raw.len() > MAX_URL_BYTES {
                return Err(format!("value must be {MAX_URL_BYTES} characters or fewer"));
            }
            let parsed = Url::parse(raw).ok();
            if !parsed.as_ref().is_some_and(|url| {
                matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
            }) {
                return Err("value must be an http(s) URL".into());
            }
            Ok((json!(raw), empty_refs))
        }
        "number" => {
            let number = value
                .as_f64()
                .filter(|number| number.is_finite())
                .ok_or_else(|| "value must be a number".to_string())?;
            let stored =
                if number.fract() == 0.0 && number >= i64::MIN as f64 && number < i64::MAX as f64 {
                    json!(number as i64)
                } else {
                    json!(number)
                };
            Ok((stored, empty_refs))
        }
        "checkbox" if value.is_boolean() => Ok((value.clone(), empty_refs)),
        "checkbox" => Err("value must be true or false".into()),
        "date" => {
            let raw = value
                .as_str()
                .ok_or_else(|| "value must be a date string in YYYY-MM-DD format".to_string())?;
            if NaiveDate::parse_from_str(raw, "%Y-%m-%d").is_err() {
                return Err("value must be a date string in YYYY-MM-DD format".into());
            }
            Ok((json!(raw), empty_refs))
        }
        "select" => {
            let config = config(definition);
            let hint = options_hint(&config);
            let selected = value
                .as_str()
                .ok_or_else(|| format!("value must be one of the option ids: {hint}"))?;
            if !option_order(&config).contains_key(selected) {
                return Err(format!("value must be one of the option ids: {hint}"));
            }
            Ok((json!(selected), empty_refs))
        }
        "multi_select" => {
            let config = config(definition);
            let hint = options_hint(&config);
            let items = value
                .as_array()
                .filter(|items| !items.is_empty())
                .ok_or_else(|| format!("value must be a non-empty array of option ids: {hint}"))?;
            let order = option_order(&config);
            let mut seen = HashSet::new();
            let mut selected = Vec::with_capacity(items.len());
            for item in items {
                let id = item.as_str().ok_or_else(|| {
                    format!("value must be a non-empty array of option ids: {hint}")
                })?;
                if !order.contains_key(id) {
                    return Err(format!(
                        "unknown option id {id:?}; valid option ids: {hint}"
                    ));
                }
                if seen.insert(id) {
                    selected.push(id);
                }
            }
            selected.sort_by_key(|id| order[id]);
            Ok((json!(selected), empty_refs))
        }
        "actor" | "multi_actor" => {
            let references = actor_refs(value, definition.type_ == "multi_actor")?;
            let stored = if definition.type_ == "actor" {
                json!(references[0].value)
            } else {
                json!(references
                    .iter()
                    .map(|reference| reference.value.as_str())
                    .collect::<Vec<_>>())
            };
            Ok((stored, references))
        }
        other => Err(format!("unsupported property type {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn definition(type_: &str, config: Value) -> IssueProperty {
        IssueProperty {
            archived_at: None,
            config,
            created_at: Utc::now(),
            description: String::new(),
            icon: String::new(),
            id: Uuid::nil(),
            name: "Property".into(),
            position: 1.0,
            type_: type_.into(),
            updated_at: Utc::now(),
            workspace_id: Uuid::nil(),
        }
    }

    #[test]
    fn primitive_values_match_go_validation_and_canonicalization() {
        assert_eq!(
            validate(&definition("text", json!({})), &json!("a\0b"))
                .unwrap()
                .0,
            json!("ab")
        );
        assert!(validate(&definition("text", json!({})), &json!("  ")).is_err());
        assert_eq!(
            validate(
                &definition("url", json!({})),
                &json!(" https://example.com/a ")
            )
            .unwrap()
            .0,
            json!("https://example.com/a")
        );
        assert!(validate(&definition("url", json!({})), &json!("file:///tmp/a")).is_err());
        assert!(validate(&definition("date", json!({})), &json!("2026-02-29")).is_err());
        assert!(validate(&definition("checkbox", json!({})), &json!(1)).is_err());
        assert!(validate(&definition("number", json!({})), &json!("1")).is_err());
        assert_eq!(
            validate(&definition("number", json!({})), &json!(1))
                .unwrap()
                .0,
            json!(1)
        );
    }

    #[test]
    fn select_values_are_validated_deduplicated_and_config_ordered() {
        let definition = definition(
            "multi_select",
            json!({"options": [
                {"id": "a", "name": "Alpha", "color": "#fff"},
                {"id": "b", "name": "Beta", "color": "#000"}
            ]}),
        );
        assert_eq!(
            validate(&definition, &json!(["b", "a", "b"])).unwrap().0,
            json!(["a", "b"])
        );
        assert!(validate(&definition, &json!(["unknown"])).is_err());
    }

    #[test]
    fn actor_values_canonicalize_uuid_and_preserve_first_seen_order() {
        let first = Uuid::parse_str("018F03A0-C4D2-7A37-AE4D-5AA45DE12F11").unwrap();
        let second = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f12").unwrap();
        let value = json!([
            format!("member:{}", first.to_string().to_ascii_uppercase()),
            format!("member:{second}"),
            format!("member:{first}")
        ]);
        let (stored, references) = validate(&definition("multi_actor", json!({})), &value).unwrap();
        assert_eq!(
            stored,
            json!([format!("member:{first}"), format!("member:{second}")])
        );
        assert_eq!(references.len(), 2);
        assert!(validate(
            &definition("actor", json!({})),
            &json!(format!("agent:{first}"))
        )
        .is_err());
    }
}
