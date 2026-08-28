//! Provider-neutral model catalog vocabulary and cache policy.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::command::RuntimeCommand;

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
    let Some(models) = result.get("models") else {
        return Vec::new();
    };
    let current = models
        .get("currentModelId")
        .or_else(|| models.get("current_model_id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let Some(available) = models
        .get("availableModels")
        .or_else(|| models.get("available_models"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut seen = std::collections::BTreeSet::new();
    available
        .iter()
        .filter_map(|entry| {
            let id = entry
                .get("modelId")
                .or_else(|| entry.get("model_id"))
                .and_then(Value::as_str)?
                .trim();
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
                .split_once('/')
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

/// Reports whether a task's thinking-level override is advertised for the
/// selected model. An empty model means the runtime's default model, except
/// for Codex: its effective model comes from config.toml and cannot be known
/// from this catalog, so the safe answer is false.
pub fn validate_thinking_level(
    catalog: &Catalog,
    provider: &str,
    model: &str,
    value: &str,
) -> bool {
    if value.is_empty() {
        return true;
    }
    if model.is_empty() && provider == "codex" {
        return false;
    }

    let mut target = model_id_for_capability_lookup(provider, model).to_string();
    if target.is_empty() {
        target = catalog
            .models
            .iter()
            .find(|entry| entry.default)
            .map(|entry| entry.id.clone())
            .unwrap_or_default();
        if target.is_empty() {
            return provider == "opencode"
                && catalog.models.iter().any(|entry| {
                    entry.thinking.as_ref().is_some_and(|thinking| {
                        thinking
                            .supported_levels
                            .iter()
                            .any(|level| level.value == value)
                    })
                });
        }
    }

    catalog
        .models
        .iter()
        .find(|entry| entry.id == target)
        .is_some_and(|entry| {
            entry.thinking.as_ref().is_some_and(|thinking| {
                thinking
                    .supported_levels
                    .iter()
                    .any(|level| level.value == value)
            })
        })
}

/// Reports whether a task's service-tier override is advertised for the
/// selected Codex model. Other providers do not own this capability.
pub fn validate_service_tier(catalog: &Catalog, provider: &str, model: &str, value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    if provider != "codex" || model.is_empty() {
        return false;
    }
    catalog
        .models
        .iter()
        .find(|entry| entry.id == model)
        .is_some_and(|entry| entry.service_tiers.iter().any(|tier| tier.id == value))
}

fn model_id_for_capability_lookup<'a>(provider: &str, model: &'a str) -> &'a str {
    if provider != "claude" {
        return model;
    }
    let Some(without_bracket) = model.strip_suffix(']') else {
        return model;
    };
    let Some(bracket) = without_bracket.rfind('[') else {
        return model;
    };
    let tag = &without_bracket[bracket + 1..];
    let bytes = tag.as_bytes();
    if bytes.len() < 2
        || !matches!(bytes.last(), Some(b'k' | b'm'))
        || bytes[0] == b'0'
        || !bytes[..bytes.len() - 1].iter().all(u8::is_ascii_digit)
    {
        return model;
    }
    &without_bracket[..bracket]
}

#[derive(Debug, Clone)]
struct CacheEntry {
    catalog: Catalog,
    expires_at: Instant,
}

/// Provider/runtime-scoped identity for one model-discovery memo entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelDiscoveryCacheKey(String);

impl ModelDiscoveryCacheKey {
    pub fn new(provider_or_runtime: &str, command: &RuntimeCommand) -> Option<Self> {
        let scope = provider_or_runtime.trim();
        if scope.is_empty() {
            return None;
        }
        if command.path.is_empty() && command.prefix.is_empty() {
            return Some(Self(scope.to_string()));
        }
        Some(Self(format!("{scope}:{}", command.cache_key())))
    }
}

/// Thread-safe discovery cache. Empty and fallback catalogs deliberately do
/// not enter it, so transient login/CLI failures can recover immediately.
#[derive(Debug)]
pub struct CatalogCache {
    ttl: Duration,
    entries: Mutex<HashMap<ModelDiscoveryCacheKey, CacheEntry>>,
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

    pub fn get(&self, key: &ModelDiscoveryCacheKey) -> Option<Catalog> {
        let mut entries = self.entries.lock().ok()?;
        let entry = entries.get(key)?;
        if entry.expires_at <= Instant::now() {
            entries.remove(key);
            return None;
        }
        Some(entry.catalog.clone())
    }

    pub fn insert(&self, key: ModelDiscoveryCacheKey, catalog: Catalog) -> bool {
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

    fn cache_key(scope: &str) -> ModelDiscoveryCacheKey {
        let Some(key) = ModelDiscoveryCacheKey::new(scope, &RuntimeCommand::default()) else {
            panic!("test cache scope must be non-empty");
        };
        key
    }

    fn model(id: &str, provider: &str) -> Model {
        Model {
            id: id.to_string(),
            label: id.to_string(),
            provider: provider.to_string(),
            ..Model::default()
        }
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
    fn discovery_cache_key_scopes_runtime_identity() {
        let builtin = cache_key("hermes");
        let executable = ModelDiscoveryCacheKey::new(
            "hermes",
            &RuntimeCommand::new("/usr/local/bin/hermes", Vec::new()),
        )
        .unwrap_or_else(|| panic!("cache key"));
        let prefixed = ModelDiscoveryCacheKey::new(
            "hermes",
            &RuntimeCommand::new(
                "/usr/local/bin/ccms",
                vec!["start".to_string(), "opus".to_string()],
            ),
        )
        .unwrap_or_else(|| panic!("cache key"));
        let other_prefix = ModelDiscoveryCacheKey::new(
            "hermes",
            &RuntimeCommand::new(
                "/usr/local/bin/ccms",
                vec!["start".to_string(), "q36".to_string()],
            ),
        )
        .unwrap_or_else(|| panic!("cache key"));

        assert_ne!(builtin, executable);
        assert_ne!(prefixed, other_prefix);
    }

    #[test]
    fn discovery_cache_key_rejects_empty_scope() {
        assert!(ModelDiscoveryCacheKey::new("  ", &RuntimeCommand::default()).is_none());
    }

    #[test]
    fn cache_rejects_empty_and_fallback_catalogs() {
        let cache = CatalogCache::default();
        let empty_key = cache_key("empty");
        let fallback_key = cache_key("fallback");
        let real_key = cache_key("real");
        assert!(!cache.insert(empty_key.clone(), Catalog::default()));
        assert!(!cache.insert(
            fallback_key,
            Catalog {
                models: vec![model("o3", "")],
                fallback: true,
            }
        ));
        assert!(cache.get(&empty_key).is_none());
        assert!(cache.insert(
            real_key.clone(),
            Catalog {
                models: vec![model("o3", "")],
                fallback: false,
            }
        ));
        assert_eq!(
            cache.get(&real_key).map(|catalog| catalog.models.len()),
            Some(1)
        );
    }

    #[test]
    fn cache_expires_entries() {
        let cache = CatalogCache::new(Duration::ZERO);
        let real_key = cache_key("real");
        assert!(cache.insert(
            real_key.clone(),
            Catalog {
                models: vec![model("o3", "")],
                fallback: false,
            }
        ));
        assert!(cache.get(&real_key).is_none());
    }

    #[test]
    fn capability_validation_uses_the_canonical_model_and_runtime_guards() {
        let catalog = Catalog {
            models: vec![
                Model {
                    id: "claude-opus-5".to_string(),
                    default: true,
                    thinking: Some(ModelThinking {
                        supported_levels: vec![ThinkingLevel {
                            value: "high".to_string(),
                            ..ThinkingLevel::default()
                        }],
                        ..ModelThinking::default()
                    }),
                    ..Model::default()
                },
                Model {
                    id: "gpt-5.6-sol".to_string(),
                    service_tiers: vec![ModelServiceTier {
                        id: "priority".to_string(),
                        ..ModelServiceTier::default()
                    }],
                    ..Model::default()
                },
            ],
            ..Catalog::default()
        };

        assert!(validate_thinking_level(
            &catalog,
            "claude",
            "claude-opus-5[1m]",
            "high"
        ));
        assert!(!validate_thinking_level(
            &catalog,
            "claude",
            "claude-opus-5[0m]",
            "high"
        ));
        assert!(validate_service_tier(
            &catalog,
            "codex",
            "gpt-5.6-sol",
            "priority"
        ));
        assert!(!validate_service_tier(
            &catalog,
            "claude",
            "gpt-5.6-sol",
            "priority"
        ));
        assert!(!validate_thinking_level(&catalog, "codex", "", "high"));
    }

    #[test]
    fn thinking_validation_handles_default_and_opencode_any_model() {
        let catalog = Catalog {
            models: vec![Model {
                id: "openai/o3".to_string(),
                thinking: Some(ModelThinking {
                    supported_levels: vec![ThinkingLevel {
                        value: "high".to_string(),
                        ..ThinkingLevel::default()
                    }],
                    ..ModelThinking::default()
                }),
                ..Model::default()
            }],
            ..Catalog::default()
        };
        assert!(!validate_thinking_level(&catalog, "pi", "", "high"));
        assert!(validate_thinking_level(&catalog, "opencode", "", "high"));
    }

    #[test]
    fn capability_validation_uses_the_first_matching_model_entry() {
        let catalog = Catalog {
            models: vec![
                Model {
                    id: "gpt-5".to_string(),
                    ..Model::default()
                },
                Model {
                    id: "gpt-5".to_string(),
                    thinking: Some(ModelThinking {
                        supported_levels: vec![ThinkingLevel {
                            value: "high".to_string(),
                            ..ThinkingLevel::default()
                        }],
                        ..ModelThinking::default()
                    }),
                    service_tiers: vec![ModelServiceTier {
                        id: "priority".to_string(),
                        ..ModelServiceTier::default()
                    }],
                    ..Model::default()
                },
            ],
            ..Catalog::default()
        };

        assert!(!validate_thinking_level(&catalog, "codex", "gpt-5", "high"));
        assert!(!validate_service_tier(
            &catalog, "codex", "gpt-5", "priority"
        ));
    }
}
