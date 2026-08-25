use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PropertyOption {
    pub(super) id: String,
    pub(super) name: String,
    #[serde(default)]
    pub(super) color: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct PropertyConfig {
    #[serde(default)]
    pub(super) options: Vec<PropertyOption>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PropertyDefinition {
    pub(super) id: String,
    pub(super) name: String,
    #[serde(rename = "type")]
    pub(super) property_type: String,
    #[serde(default)]
    pub(super) description: String,
    #[serde(default)]
    pub(super) icon: String,
    #[serde(default)]
    pub(super) config: PropertyConfig,
    #[serde(default)]
    pub(super) position: f64,
    #[serde(default)]
    pub(super) archived: bool,
    #[serde(default)]
    pub(super) usage_count: i64,
    #[serde(default)]
    pub(super) created_at: String,
    #[serde(default)]
    pub(super) updated_at: String,
}
