//! Wire types for the plugin manifest. Serde field names match the Go json
//! tags byte-for-byte, and struct field order matches Go declaration order so
//! canonical re-marshaling is stable.

use serde::{Deserialize, Serialize};

/// The only manifest version this host understands.
pub const MANIFEST_VERSION_1: i64 = 1;

/// The conventional file name inside a plugin package.
pub const MANIFEST_FILENAME: &str = "cordy.plugin.json";

/// Bounds a fetched manifest before it is parsed.
pub const MAX_MANIFEST_SIZE: usize = 1 << 20;

/// Mirrors the plugin_installation.version column bound.
pub const MAX_VERSION_LENGTH: usize = 64;

/// Surface types. A surface is an iframe the host mounts at a fixed location.
pub const SURFACE_ISSUE_PANEL: &str = "issue_panel";
pub const SURFACE_SIDEBAR_PANEL: &str = "sidebar_panel";
pub const SURFACE_MODAL: &str = "modal";

/// Hook triggers. Declaring a trigger says who may invoke the hook, never what
/// the hook does. Only `event` is asynchronous, and it never blocks the host.
pub const TRIGGER_UI: &str = "ui";
pub const TRIGGER_MANUAL: &str = "manual";
pub const TRIGGER_AGENT: &str = "agent";
pub const TRIGGER_EVENT: &str = "event";

/// Hook transports.
pub const TRANSPORT_HTTP: &str = "http";
pub const TRANSPORT_MCP: &str = "mcp";

/// Resource types. Resources involve no call in either direction.
pub const RESOURCE_SKILL: &str = "skill";

/// Config field types. This is a deliberately small subset of JSON Schema: the
/// host renders the configuration form from it, so plugins never ship form UI.
pub const CONFIG_STRING: &str = "string";
pub const CONFIG_NUMBER: &str = "number";
pub const CONFIG_BOOL: &str = "bool";
pub const CONFIG_ENUM: &str = "enum";
pub const CONFIG_SECRET: &str = "secret";

/// Scopes. This list is complete and closed; anything else is rejected at parse
/// time. `net:<domain>` is the only parameterized form.
pub const SCOPE_ISSUES_READ: &str = "issues:read";
pub const SCOPE_ISSUES_WRITE: &str = "issues:write";
pub const SCOPE_COMMENTS_READ: &str = "comments:read";
pub const SCOPE_COMMENTS_WRITE: &str = "comments:write";
pub const SCOPE_TASKS_READ: &str = "tasks:read";
pub const SCOPE_TASKS_WRITE: &str = "tasks:write";
pub const SCOPE_AGENTS_READ: &str = "agents:read";
pub const SCOPE_MEMBERS_READ: &str = "members:read";
pub const SCOPE_STORAGE_USER: &str = "storage:user";
pub const SCOPE_STORAGE_WORKSPACE: &str = "storage:workspace";

/// Guards outbound network access: both the iframe CSP connect-src allowlist
/// and the hook transport host check derive from it.
pub const SCOPE_NET_PREFIX: &str = "net:";

/// Product events an `event`-triggered hook may subscribe to.
pub const EVENT_ISSUE_CREATED: &str = "issue.created";
pub const EVENT_ISSUE_UPDATED: &str = "issue.updated";
pub const EVENT_ISSUE_STATUS_CHANGED: &str = "issue.status_changed";
pub const EVENT_COMMENT_CREATED: &str = "comment.created";
pub const EVENT_TASK_STARTED: &str = "task.started";
pub const EVENT_TASK_COMPLETED: &str = "task.completed";
pub const EVENT_TASK_FAILED: &str = "task.failed";

/// The closed set of scopes that are not `net:`-parameterized.
pub fn is_fixed_scope(scope: &str) -> bool {
    matches!(
        scope,
        SCOPE_ISSUES_READ
            | SCOPE_ISSUES_WRITE
            | SCOPE_COMMENTS_READ
            | SCOPE_COMMENTS_WRITE
            | SCOPE_TASKS_READ
            | SCOPE_TASKS_WRITE
            | SCOPE_AGENTS_READ
            | SCOPE_MEMBERS_READ
            | SCOPE_STORAGE_USER
            | SCOPE_STORAGE_WORKSPACE
    )
}

/// Reports whether an event may be subscribed to by a manifest. Exported so the
/// dispatcher cannot publish an event no plugin could ever receive — the two
/// lists have to agree, and only one of them is authoritative.
pub fn is_known_event(event: &str) -> bool {
    matches!(
        event,
        EVENT_ISSUE_CREATED
            | EVENT_ISSUE_UPDATED
            | EVENT_ISSUE_STATUS_CHANGED
            | EVENT_COMMENT_CREATED
            | EVENT_TASK_STARTED
            | EVENT_TASK_COMPLETED
            | EVENT_TASK_FAILED
    )
}

/// Maps each event onto the scope a plugin would have needed to read the same
/// content through the Action API.
pub(crate) fn event_read_scope(event: &str) -> Option<&'static str> {
    match event {
        EVENT_ISSUE_CREATED | EVENT_ISSUE_UPDATED | EVENT_ISSUE_STATUS_CHANGED => {
            Some(SCOPE_ISSUES_READ)
        }
        EVENT_COMMENT_CREATED => Some(SCOPE_COMMENTS_READ),
        EVENT_TASK_STARTED | EVENT_TASK_COMPLETED | EVENT_TASK_FAILED => Some(SCOPE_TASKS_READ),
        _ => None,
    }
}

pub(crate) fn has_scope(scopes: &[String], want: &str) -> bool {
    scopes.iter().any(|scope| scope == want)
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    // Every non-Option field carries `default`: Go's encoding/json zero-values
    // missing keys instead of erroring, and validation rejects the empty
    // values afterward. Missing must stay legal at decode time.
    #[serde(rename = "manifest_version", default)]
    pub manifest_version: i64,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: Author,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub icon: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Go's omitempty never omits a struct, and ConfigSchema marshals at least
    /// `{}` — so this field is always present on the wire.
    #[serde(default)]
    pub config: ConfigSchema,
    #[serde(default)]
    pub contributes: Contributes,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Author {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contributes {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surfaces: Vec<Surface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<Hook>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<Resource>,
}

/// A host-mounted iframe. The host owns where it appears; the plugin owns what
/// renders inside it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Surface {
    #[serde(default)]
    pub key: String,
    #[serde(rename = "type", default)]
    pub surface_type: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub entry: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<String>,
}

/// One plugin-side capability. Triggers say who may invoke it; the host never
/// invents a call site that is not listed here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Kept as raw JSON (like Go's json.RawMessage) so an explicit `null`
    /// stays distinguishable from an absent field: Go rejects `null` because
    /// it fails the object/type check, and so does this port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Box<serde_json::value::RawValue>>,
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
    #[serde(default)]
    pub transport: HookTransport,
    #[serde(rename = "timeout_ms", default, skip_serializing_if = "is_zero_i64")]
    pub timeout_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookTransport {
    #[serde(rename = "type", default)]
    pub transport_type: String,
    #[serde(default)]
    pub url: String,
}

/// A static contribution. No call happens in either direction, so a resource is
/// not a hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    #[serde(rename = "type", default)]
    pub resource_type: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub entry: String,
}

/// One host-rendered configuration input. Secrets are stored write-only and
/// never echoed back by any read endpoint.
///
/// `key` carries no wire presence (Go's `json:"-"`): on the wire it is the
/// object key of the enclosing config map, assigned during [`ConfigSchema`]
/// deserialization.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct ConfigField {
    #[serde(skip)]
    pub key: String,
    #[serde(rename = "type")]
    pub field_type: String,
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub placeholder: String,
    /// Asks the host to render a textarea. Only meaningful for string fields;
    /// without it a value that is a list of lines is unreadable in the
    /// generated form, which is the one thing the host owns rendering for.
    #[serde(skip_serializing_if = "is_false")]
    pub multiline: bool,
}

/// Deserialization shape of one config field body: everything except `key`,
/// which arrives as the enclosing map's object key.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFieldBody {
    #[serde(rename = "type", default)]
    field_type: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    placeholder: String,
    #[serde(default)]
    multiline: bool,
}

impl From<ConfigFieldBody> for ConfigField {
    fn from(body: ConfigFieldBody) -> Self {
        ConfigField {
            key: String::new(),
            field_type: body.field_type,
            label: body.label,
            description: body.description,
            required: body.required,
            options: body.options,
            placeholder: body.placeholder,
            multiline: body.multiline,
        }
    }
}

/// Keeps declaration order so the generated form is stable across installs.
/// The wire format stays a JSON object keyed by field name.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConfigSchema {
    pub fields: Vec<ConfigField>,
}

impl ConfigSchema {
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn field(&self, key: &str) -> Option<&ConfigField> {
        self.fields.iter().find(|field| field.key == key)
    }
}

impl Serialize for ConfigSchema {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.fields.len()))?;
        for field in &self.fields {
            map.serialize_entry(&field.key, field)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ConfigSchema {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = ConfigSchema;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object of config fields")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Self::Value, A::Error> {
                let mut fields = Vec::new();
                let mut seen = std::collections::HashSet::new();
                while let Some(key) = access.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(serde::de::Error::custom(format!(
                            "config contains duplicate field {key:?}"
                        )));
                    }
                    let mut field: ConfigField = access.next_value::<ConfigFieldBody>()?.into();
                    field.key = key;
                    fields.push(field);
                }
                Ok(ConfigSchema { fields })
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}
