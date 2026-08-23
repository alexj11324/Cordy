//! Plugin contract: manifest parsing, config field types, and host
//! capabilities. Rust port of `server/pkg/plugincontract/`.
//!
//! A plugin relates to Cordy in exactly three ways:
//!
//!   - Action   (plugin -> Cordy): host capabilities the plugin calls.
//!   - Hook     (Cordy -> plugin): plugin capabilities the host calls.
//!   - Resource (no call at all):  static contributions such as skill text.
//!
//! "Who triggers" and "what capability is called" are orthogonal: a hook is
//! declared once and may be invoked by any of the triggers it opts into.

pub mod capabilities;
pub mod parse;
pub mod types;
pub mod validate;

pub use capabilities::{host_capabilities, net_domains, Capabilities, CapabilityUnavailable};
pub use parse::parse_manifest;
pub use types::{
    is_known_event, Author, ConfigField, ConfigSchema, Contributes, Hook, HookTransport, Manifest,
    Resource, Surface, CONFIG_BOOL, CONFIG_ENUM, CONFIG_NUMBER, CONFIG_SECRET, CONFIG_STRING,
    EVENT_COMMENT_CREATED, EVENT_ISSUE_CREATED, EVENT_ISSUE_STATUS_CHANGED, EVENT_ISSUE_UPDATED,
    EVENT_TASK_COMPLETED, EVENT_TASK_FAILED, EVENT_TASK_STARTED, MANIFEST_FILENAME,
    MANIFEST_VERSION_1, MAX_MANIFEST_SIZE, MAX_VERSION_LENGTH, RESOURCE_SKILL, SCOPE_AGENTS_READ,
    SCOPE_COMMENTS_READ, SCOPE_COMMENTS_WRITE, SCOPE_ISSUES_READ, SCOPE_ISSUES_WRITE,
    SCOPE_MEMBERS_READ, SCOPE_NET_PREFIX, SCOPE_STORAGE_USER, SCOPE_STORAGE_WORKSPACE,
    SCOPE_TASKS_READ, SCOPE_TASKS_WRITE, SURFACE_ISSUE_PANEL, SURFACE_MODAL, SURFACE_SIDEBAR_PANEL,
    TRANSPORT_HTTP, TRANSPORT_MCP, TRIGGER_AGENT, TRIGGER_EVENT, TRIGGER_MANUAL, TRIGGER_UI,
};
pub use validate::{validate_scope, Error};
