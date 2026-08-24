//! Provider-neutral model catalog vocabulary and cache policy.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub service_tiers: Vec<ModelServiceTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ModelThinking>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelServiceTier {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelThinking {
    pub supported_levels: Vec<ThinkingLevel>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_level: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingLevel {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    pub models: Vec<Model>,
    /// True when models are a static stand-in after discovery failed. Such a
    /// catalog must not be cached or used to qualify a persisted selector.
    pub fallback: bool,
}

/// Parses the standard ACP `session/new.models` catalog without inventing a
/// fallback. Callers supply the runtime family only when an entry carries no
/// provider-qualified id; authoritative model ids and advertised order remain
/// unchanged.
pub fn parse_acp_session_models(result: &Value, fallback_provider: &str) -> Vec<Model> {
    let from_models = result
        .get("models")
        .and_then(Value::as_object)
        .map(|models| {
            let current = first_non_empty_string(
                models.get("currentModelId"),
                models.get("current_model_id"),
            );
            let available = first_non_empty_array(
                models.get("availableModels"),
                models.get("available_models"),
            );
            parse_model_entries(available, &current, fallback_provider, |entry| {
                first_non_empty_string(entry.get("modelId"), entry.get("model_id"))
            })
        })
        .unwrap_or_default();
    if !from_models.is_empty() {
        return from_models;
    }

    for options in [result.get("configOptions"), result.get("config_options")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
    {
        for option in options {
            let is_model = ["id", "category"].iter().any(|key| {
                option
                    .get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case("model"))
            });
            if !is_model {
                continue;
            }
            let current =
                first_non_empty_string(option.get("currentValue"), option.get("current_value"));
            let available = option
                .get("options")
                .and_then(Value::as_array)
                .filter(|choices| !choices.is_empty());
            let parsed = parse_model_entries(available, &current, fallback_provider, |entry| {
                entry
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            });
            if !parsed.is_empty() {
                return parsed;
            }
        }
    }
    Vec::new()
}

fn first_non_empty_string(camel: Option<&Value>, snake: Option<&Value>) -> String {
    [camel, snake]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn first_non_empty_array<'a>(
    camel: Option<&'a Value>,
    snake: Option<&'a Value>,
) -> Option<&'a Vec<Value>> {
    [camel, snake]
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .find(|values| !values.is_empty())
}

fn parse_model_entries(
    available: Option<&Vec<Value>>,
    current: &str,
    fallback_provider: &str,
    model_id: impl Fn(&Value) -> String,
) -> Vec<Model> {
    let Some(available) = available else {
        return Vec::new();
    };
    let mut seen = std::collections::BTreeSet::new();
    available
        .iter()
        .filter_map(|entry| {
            let id = model_id(entry);
            let id = id.trim();
            if id.is_empty() || !seen.insert(id.to_string()) {
                return None;
            }
            let label = entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .unwrap_or(id);
            let provider = id
                .split_once(':')
                .or_else(|| id.split_once('/'))
                .map(|(provider, _)| provider)
                .filter(|provider| !provider.is_empty())
                .unwrap_or(fallback_provider);
            Some(Model {
                id: id.to_string(),
                label: label.to_string(),
                provider: provider.to_string(),
                default: id == current,
                ..Model::default()
            })
        })
        .collect()
}

/// Qualifies a bare selector only when one authoritative catalog entry owns it.
pub fn qualify_model_id(catalog: &Catalog, model: &str) -> (String, bool) {
    let model = model.trim();
    if model.is_empty() || catalog.fallback {
        return (model.to_string(), false);
    }
    if catalog.models.iter().any(|entry| entry.id == model) {
        return (model.to_string(), false);
    }
    let mut qualified: Option<&str> = None;
    for entry in &catalog.models {
        if entry.provider.is_empty() || entry.id != format!("{}/{}", entry.provider, model) {
            continue;
        }
        if qualified.is_some_and(|current| current != entry.id) {
            return (model.to_string(), false);
        }
        qualified = Some(&entry.id);
    }
    qualified.map_or_else(
        || (model.to_string(), false),
        |qualified| (qualified.to_string(), true),
    )
}

#[derive(Debug, Clone)]
struct CacheEntry {
    catalog: Catalog,
    expires_at: Instant,
}

/// Thread-safe discovery cache. Empty and fallback catalogs deliberately do
/// not enter it, so transient login/CLI failures can recover immediately.
#[derive(Debug)]
pub struct CatalogCache {
    ttl: Duration,
    entries: Mutex<HashMap<String, CacheEntry>>,
}

impl Default for CatalogCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(60))
    }
}

impl CatalogCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, key: &str) -> Option<Catalog> {
        let mut entries = self.entries.lock().ok()?;
        let entry = entries.get(key)?;
        if entry.expires_at <= Instant::now() {
            entries.remove(key);
            return None;
        }
        Some(entry.catalog.clone())
    }

    pub fn insert(&self, key: String, catalog: Catalog) -> bool {
        if catalog.fallback || catalog.models.is_empty() {
            return false;
        }
        let Ok(mut entries) = self.entries.lock() else {
            return false;
        };
        entries.insert(
            key,
            CacheEntry {
                catalog,
                expires_at: Instant::now() + self.ttl,
            },
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, provider: &str) -> Model {
        Model {
            id: id.to_string(),
            label: id.to_string(),
            provider: provider.to_string(),
            ..Model::default()
        }
    }

    #[test]
    fn acp_models_fall_back_to_config_options_catalog() {
        let result = serde_json::json!({
            "configOptions": [{"id": "thinking", "options": [{"value": "high"}]}],
            "config_options": [{
                "category": "MODEL",
                "current_value": "qoder:auto",
                "options": [
                    {"value": "qoder:auto", "name": "Auto"},
                    {"value": "custom:fast", "name": "Fast"},
                    {"value": "custom:fast", "name": "duplicate"}
                ]
            }]
        });
        let models = parse_acp_session_models(&result, "traecli");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "qoder:auto");
        assert_eq!(models[0].provider, "qoder");
        assert!(models[0].default);
        assert_eq!(models[1].provider, "custom");
    }

    #[test]
    fn acp_models_validate_camel_case_before_snake_case_fallback() {
        let result = serde_json::json!({
            "models": {
                "currentModelId": " ",
                "current_model_id": "custom:snake",
                "availableModels": {"unexpected": true},
                "available_models": [
                    {"modelId": null, "model_id": "custom:snake", "name": "Snake"},
                    {"modelId": "", "model_id": "plain", "name": ""}
                ]
            }
        });
        let models = parse_acp_session_models(&result, "traecli");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "custom:snake");
        assert_eq!(models[0].provider, "custom");
        assert!(models[0].default);
        assert_eq!(models[1].id, "plain");
        assert_eq!(models[1].label, "plain");
        assert_eq!(models[1].provider, "traecli");
    }

    #[test]
    fn qualification_requires_one_authoritative_owner() {
        let catalog = Catalog {
            models: vec![model("openai/o3", "openai")],
            fallback: false,
        };
        assert_eq!(
            qualify_model_id(&catalog, "o3"),
            ("openai/o3".to_string(), true)
        );
        assert_eq!(
            qualify_model_id(&catalog, "openai/o3"),
            ("openai/o3".to_string(), false)
        );
        let fallback = Catalog {
            fallback: true,
            ..catalog
        };
        assert_eq!(qualify_model_id(&fallback, "o3"), ("o3".to_string(), false));
    }

    #[test]
    fn cache_rejects_empty_and_fallback_catalogs() {
        let cache = CatalogCache::default();
        assert!(!cache.insert("empty".to_string(), Catalog::default()));
        assert!(!cache.insert(
            "fallback".to_string(),
            Catalog {
                models: vec![model("o3", "")],
                fallback: true,
            }
        ));
        assert!(cache.get("empty").is_none());
        assert!(cache.insert(
            "real".to_string(),
            Catalog {
                models: vec![model("o3", "")],
                fallback: false,
            }
        ));
        assert_eq!(
            cache.get("real").map(|catalog| catalog.models.len()),
            Some(1)
        );
    }

    #[test]
    fn cache_expires_entries() {
        let cache = CatalogCache::new(Duration::ZERO);
        assert!(cache.insert(
            "real".to_string(),
            Catalog {
                models: vec![model("o3", "")],
                fallback: false,
            }
        ));
        assert!(cache.get("real").is_none());
    }
}
