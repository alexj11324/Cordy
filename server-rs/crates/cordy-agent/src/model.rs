//! Provider-neutral model catalog vocabulary and cache policy.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

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
