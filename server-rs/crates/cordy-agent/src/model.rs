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
    let models = result.get("models");
    let current = models
        .and_then(|models| {
            models
                .get("currentModelId")
                .or_else(|| models.get("current_model_id"))
        })
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let available = models
        .and_then(|models| {
            models
                .get("availableModels")
                .or_else(|| models.get("available_models"))
        })
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut seen = std::collections::BTreeSet::new();
    let parsed: Vec<_> = available
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
        .collect();
    if parsed.is_empty() {
        parse_acp_config_models(result)
    } else {
        parsed
    }
}

fn parse_acp_config_models(result: &Value) -> Vec<Model> {
    let options = result
        .get("configOptions")
        .or_else(|| result.get("config_options"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let Some(model_option) = options.iter().find(|option| {
        ["id", "category"].iter().any(|key| {
            option
                .get(*key)
                .and_then(Value::as_str)
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("model"))
        })
    }) else {
        return Vec::new();
    };
    let current = model_option
        .get("currentValue")
        .or_else(|| model_option.get("current_value"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let choices = model_option
        .get("options")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut seen = std::collections::BTreeSet::new();
    choices
        .iter()
        .filter_map(|choice| {
            let id = choice.get("value").and_then(Value::as_str)?.trim();
            if id.is_empty() || !seen.insert(id.to_string()) {
                return None;
            }
            let label = choice
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|label| !label.is_empty() && !label.eq_ignore_ascii_case("unknown"))
                .unwrap_or(id);
            let provider = id
                .split_once(':')
                .map(|(provider, _)| provider)
                .filter(|provider| !provider.is_empty())
                .unwrap_or_default();
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

/// Reports whether a runtime requires the canonical `<provider>/<model>`
/// selector before it can launch a pinned model.
///
/// This is deliberately an execution contract, not a guess based on whether
/// a catalog happens to contain slashes. Built-in runtime identities inherit
/// the protocol family's rule; custom profiles keep the provider family they
/// registered under.
pub fn model_selector_must_be_provider_qualified(provider: &str) -> bool {
    matches!(
        crate::registry::protocol_family(provider).unwrap_or(""),
        "opencode" | "deveco"
    )
}

/// Validates one thinking override against the catalog that the runtime just
/// advertised. An empty model means the runtime's own default, except for
/// Codex where the effective model comes from config and therefore cannot be
/// inferred safely from the catalog.
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

    let mut target = model_id_for_capability_lookup(provider, model);
    if target.is_empty() {
        target = catalog
            .models
            .iter()
            .find(|entry| entry.default)
            .map(|entry| entry.id.as_str())
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

    catalog.models.iter().any(|entry| {
        entry.id == target
            && entry.thinking.as_ref().is_some_and(|thinking| {
                thinking
                    .supported_levels
                    .iter()
                    .any(|level| level.value == value)
            })
    })
}

/// Validates the Codex service tier advertised for one explicit model.
/// Other runtimes do not expose a service-tier execution control.
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
        .any(|entry| entry.id == model && entry.service_tiers.iter().any(|tier| tier.id == value))
}

fn model_id_for_capability_lookup<'a>(provider: &str, model: &'a str) -> &'a str {
    if provider != "claude" {
        return model;
    }
    let Some(open) = model.rfind('[') else {
        return model;
    };
    let Some(body) = model.get(open + 1..model.len().saturating_sub(1)) else {
        return model;
    };
    if !model.ends_with(']')
        || body.len() < 2
        || !matches!(body.as_bytes().last(), Some(b'k' | b'm'))
        || body.starts_with('0')
        || !body[..body.len() - 1]
            .bytes()
            .all(|byte| byte.is_ascii_digit())
    {
        return model;
    }
    &model[..open]
}

#[derive(Debug, Clone)]
struct CacheEntry {
    catalog: Catalog,
    expires_at: Instant,
}

/// Provider/runtime-scoped identity for one model-discovery memo entry.
///
/// The scope is mandatory even when two adapters launch the same executable
/// and fixed prefix: compatible runtimes can share a wrapper while exposing
/// different catalogs or discovery protocols. The inner representation is
/// private so cache users cannot substitute `RuntimeCommand::cache_key()` and
/// accidentally collapse those runtime identities.
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

    fn model(id: &str, provider: &str) -> Model {
        Model {
            id: id.to_string(),
            label: id.to_string(),
            provider: provider.to_string(),
            ..Model::default()
        }
    }

    fn cache_key(scope: &str) -> ModelDiscoveryCacheKey {
        let Some(key) = ModelDiscoveryCacheKey::new(scope, &RuntimeCommand::default()) else {
            panic!("test cache scope must be non-empty");
        };
        key
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
    fn selector_contract_follows_builtin_protocol_family() {
        assert!(model_selector_must_be_provider_qualified("opencode"));
        assert!(model_selector_must_be_provider_qualified("deveco"));
        assert!(!model_selector_must_be_provider_qualified("pi"));
        assert!(!model_selector_must_be_provider_qualified("omp"));
        assert!(!model_selector_must_be_provider_qualified("unknown"));
    }

    #[test]
    fn capability_validation_uses_the_canonical_model_and_default() {
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
                    id: "openai/gpt-5".to_string(),
                    service_tiers: vec![ModelServiceTier {
                        id: "priority".to_string(),
                        ..ModelServiceTier::default()
                    }],
                    ..Model::default()
                },
            ],
            fallback: false,
        };
        assert!(validate_thinking_level(
            &catalog,
            "claude",
            "claude-opus-5[1m]",
            "high"
        ));
        assert!(validate_thinking_level(&catalog, "claude", "", "high"));
        assert!(!validate_thinking_level(
            &catalog,
            "claude",
            "claude-opus-5",
            "low"
        ));
        assert!(validate_service_tier(
            &catalog,
            "codex",
            "openai/gpt-5",
            "priority"
        ));
        assert!(!validate_service_tier(&catalog, "codex", "", "priority"));
    }

    #[test]
    fn opencode_empty_model_checks_any_advertised_thinking_level() {
        let catalog = Catalog {
            models: vec![Model {
                thinking: Some(ModelThinking {
                    supported_levels: vec![ThinkingLevel {
                        value: "xhigh".to_string(),
                        ..ThinkingLevel::default()
                    }],
                    ..ModelThinking::default()
                }),
                ..Model::default()
            }],
            fallback: false,
        };
        assert!(validate_thinking_level(&catalog, "opencode", "", "xhigh"));
        assert!(!validate_thinking_level(&catalog, "opencode", "", "low"));
    }

    #[test]
    fn acp_config_model_catalog_is_used_without_mixing_thinking_options() {
        let models = parse_acp_session_models(
            &serde_json::json!({
                "configOptions":[
                    {
                        "id":"thinking",
                        "category":"thought_level",
                        "currentValue":"high",
                        "options":[{"value":"high","name":"High"}]
                    },
                    {
                        "id":"model",
                        "category":"model",
                        "currentValue":"kimi-code/k3",
                        "options":[
                            {"value":"kimi-code/k3","name":"K3"},
                            {"value":"openai:gpt-5","name":"unknown"},
                            {"value":"kimi-code/k3","name":"duplicate"},
                            {"value":"","name":"empty"}
                        ]
                    }
                ]
            }),
            "kimi",
        );
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "kimi-code/k3");
        assert_eq!(models[0].label, "K3");
        assert!(models[0].default);
        assert!(models[0].provider.is_empty());
        assert_eq!(models[1].label, "openai:gpt-5");
        assert_eq!(models[1].provider, "openai");
    }

    #[test]
    fn structured_models_catalog_wins_over_config_option_fallback() {
        let models = parse_acp_session_models(
            &serde_json::json!({
                "models": {
                    "currentModelId":"direct",
                    "availableModels":[{"modelId":"direct","name":"Direct"}]
                },
                "configOptions":[{
                    "id":"model",
                    "options":[{"value":"fallback","name":"Fallback"}]
                }]
            }),
            "provider",
        );
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "direct");
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
    fn cache_key_isolates_runtime_identity_for_a_shared_wrapper() {
        let command = RuntimeCommand::new(
            "/usr/local/bin/agent-wrapper",
            vec!["start".to_string(), "shared".to_string()],
        );
        let Some(pi_key) = ModelDiscoveryCacheKey::new("pi", &command) else {
            panic!("pi scope must be valid");
        };
        let Some(omp_key) = ModelDiscoveryCacheKey::new("omp", &command) else {
            panic!("omp scope must be valid");
        };
        assert_ne!(pi_key, omp_key);
        assert!(ModelDiscoveryCacheKey::new("", &command).is_none());

        let cache = CatalogCache::default();
        assert!(cache.insert(
            pi_key.clone(),
            Catalog {
                models: vec![model("pi/model", "pi")],
                fallback: false,
            }
        ));
        assert_eq!(
            cache.get(&pi_key).map(|catalog| catalog.models.len()),
            Some(1)
        );
        assert!(cache.get(&omp_key).is_none());
    }
}
