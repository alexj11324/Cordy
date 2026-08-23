//! Manifest validation rules — a faithful port of the Go `Validate` family.
//! Error message strings match the Go originals because tests and consumers
//! match on their substrings.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

use crate::types::{
    event_read_scope, has_scope, is_fixed_scope, is_known_event, Manifest, CONFIG_BOOL,
    CONFIG_ENUM, CONFIG_NUMBER, CONFIG_SECRET, CONFIG_STRING, MANIFEST_VERSION_1,
    MAX_VERSION_LENGTH, RESOURCE_SKILL, SCOPE_NET_PREFIX, SURFACE_ISSUE_PANEL, SURFACE_MODAL,
    SURFACE_SIDEBAR_PANEL, TRANSPORT_HTTP, TRANSPORT_MCP, TRIGGER_AGENT, TRIGGER_EVENT,
    TRIGGER_MANUAL, TRIGGER_UI,
};

/// Validation / parse failure. The payload is the human-readable message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub(crate) String);

impl Error {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Error(message.into())
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// All five manifest patterns, compiled once. `None` means a pattern literal
/// failed to compile, which a unit test pins as impossible; validators surface
/// it as an internal error rather than panicking.
pub(crate) struct Patterns {
    pub plugin_key_segment: Regex,
    pub contribution_key: Regex,
    pub semver: Regex,
    pub net_domain: Regex,
    pub relative_path: Regex,
}

pub(crate) fn patterns() -> Option<&'static Patterns> {
    static PATTERNS: OnceLock<std::result::Result<Patterns, regex::Error>> = OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            Ok(Patterns {
                plugin_key_segment: Regex::new(r"^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$")?,
                contribution_key: Regex::new(r"^[a-z][a-z0-9]*(?:[_-][a-z0-9]+)*$")?,
                semver: Regex::new(
                    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$",
                )?,
                net_domain: Regex::new(
                    r"^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$",
                )?,
                relative_path: Regex::new(r"^[A-Za-z0-9._-]+(?:/[A-Za-z0-9._-]+)*$")?,
            })
        })
        .as_ref()
        .ok()
}

fn missing_patterns() -> Error {
    Error::new("plugin contract validation patterns failed to compile")
}

impl Manifest {
    pub fn validate(&self) -> Result<()> {
        if self.manifest_version != MANIFEST_VERSION_1 {
            return Err(Error::new(format!(
                "manifest_version must be {MANIFEST_VERSION_1}"
            )));
        }
        validate_plugin_key(&self.key)?;
        validate_display_text("name", &self.name, 160)?;
        if self.description.len() > 2000 {
            return Err(Error::new("description exceeds 2000 bytes"));
        }
        if self.description.contains('\r') {
            return Err(Error::new("description must not contain carriage returns"));
        }
        // Bounded before the regex result is trusted: semver permits unbounded
        // build/prerelease segments, and plugin_installation.version is capped
        // at 64. Rejecting here turns a constraint violation at INSERT time
        // into a parse error that names the field.
        if self.version.len() > MAX_VERSION_LENGTH {
            return Err(Error::new(format!(
                "version exceeds {MAX_VERSION_LENGTH} bytes"
            )));
        }
        let Some(p) = patterns() else {
            return Err(missing_patterns());
        };
        if !p.semver.is_match(&self.version) {
            return Err(Error::new(format!(
                "version must be semantic versioning, got {:?}",
                self.version
            )));
        }
        validate_display_text("author.name", &self.author.name, 160)?;
        if !self.author.url.is_empty() {
            validate_https_url("author.url", &self.author.url)?;
        }
        if !self.icon.is_empty() {
            validate_relative_path("icon", &self.icon)?;
        }
        self.validate_scopes()?;
        self.validate_config()?;
        self.validate_contributions()
    }

    fn validate_scopes(&self) -> Result<()> {
        if self.scopes.is_empty() {
            return Err(Error::new("scopes must not be empty"));
        }
        if self.scopes.len() > 64 {
            return Err(Error::new("scopes must not exceed 64 entries"));
        }
        let mut seen = HashSet::new();
        for (index, scope) in self.scopes.iter().enumerate() {
            if !seen.insert(scope.clone()) {
                return Err(Error::new(format!(
                    "scopes contains duplicate value {scope:?}"
                )));
            }
            validate_scope(scope).map_err(|e| Error::new(format!("scopes[{index}]: {e}")))?;
        }
        Ok(())
    }

    fn validate_config(&self) -> Result<()> {
        let Some(p) = patterns() else {
            return Err(missing_patterns());
        };
        if self.config.len() > 32 {
            return Err(Error::new("config must not exceed 32 fields"));
        }
        for field in &self.config.fields {
            if !p.contribution_key.is_match(&field.key) {
                return Err(Error::new(format!(
                    "config contains invalid field name {:?}",
                    field.key
                )));
            }
            let label = format!("config.{}", field.key);
            match field.field_type.as_str() {
                CONFIG_STRING | CONFIG_NUMBER | CONFIG_BOOL | CONFIG_SECRET => {
                    if !field.options.is_empty() {
                        return Err(Error::new(format!(
                            "{label}.options is only valid for enum fields"
                        )));
                    }
                    if field.multiline && field.field_type != CONFIG_STRING {
                        return Err(Error::new(format!(
                            "{label}.multiline is only valid for string fields"
                        )));
                    }
                }
                CONFIG_ENUM => {
                    if field.options.is_empty() {
                        return Err(Error::new(format!(
                            "{label}.options must not be empty for enum fields"
                        )));
                    }
                    if field.options.len() > 64 {
                        return Err(Error::new(format!(
                            "{label}.options must not exceed 64 entries"
                        )));
                    }
                    let mut option_seen = HashSet::new();
                    for option in &field.options {
                        validate_display_text(&format!("{label}.options"), option, 160)?;
                        if !option_seen.insert(option.clone()) {
                            return Err(Error::new(format!(
                                "{label}.options contains duplicate value {option:?}"
                            )));
                        }
                    }
                }
                other => {
                    return Err(Error::new(format!(
                        "{label}.type is unsupported: {other:?}"
                    )));
                }
            }
            if field.multiline && field.field_type != CONFIG_STRING {
                return Err(Error::new(format!(
                    "{label}.multiline is only valid for string fields"
                )));
            }
            validate_display_text(&format!("{label}.label"), &field.label, 160)?;
            if field.description.len() > 500 {
                return Err(Error::new(format!("{label}.description exceeds 500 bytes")));
            }
            if field.placeholder.len() > 160 {
                return Err(Error::new(format!("{label}.placeholder exceeds 160 bytes")));
            }
        }
        Ok(())
    }

    fn validate_contributions(&self) -> Result<()> {
        let contributes = &self.contributes;
        let total =
            contributes.surfaces.len() + contributes.hooks.len() + contributes.resources.len();
        if total == 0 {
            return Err(Error::new(
                "contributes must declare at least one surface, hook, or resource",
            ));
        }
        if total > 64 {
            return Err(Error::new("contributes must not exceed 64 entries"));
        }

        let Some(p) = patterns() else {
            return Err(missing_patterns());
        };

        let mut surface_keys = HashSet::new();
        for (index, surface) in contributes.surfaces.iter().enumerate() {
            let field = format!("contributes.surfaces[{index}]");
            if !p.contribution_key.is_match(&surface.key) {
                return Err(Error::new(format!("{field}.key is invalid")));
            }
            if !surface_keys.insert(surface.key.clone()) {
                return Err(Error::new(format!(
                    "duplicate surface key {:?}",
                    surface.key
                )));
            }
            match surface.surface_type.as_str() {
                SURFACE_ISSUE_PANEL | SURFACE_SIDEBAR_PANEL | SURFACE_MODAL => {}
                other => {
                    return Err(Error::new(format!(
                        "{field}.type is unsupported: {other:?}"
                    )));
                }
            }
            validate_display_text(&format!("{field}.name"), &surface.name, 160)?;
            validate_relative_path(&format!("{field}.entry"), &surface.entry)?;
            // The host generates the surface's HTML document and loads this
            // script into it. That is what lets the host attach the CSP derived
            // from the manifest's net: scopes — a plugin-authored HTML document
            // would carry whatever policy its own server sent, so net: would be
            // a claim rather than a control.
            if !surface.entry.ends_with(".js") && !surface.entry.ends_with(".mjs") {
                return Err(Error::new(format!(
                    "{field}.entry must be a .js or .mjs script; the host renders the surface document itself"
                )));
            }
            let mut platform_seen = HashSet::new();
            for platform in &surface.platforms {
                if platform != "web" && platform != "desktop" {
                    return Err(Error::new(format!(
                        "{field}.platforms contains unsupported platform {platform:?}"
                    )));
                }
                if !platform_seen.insert(platform.clone()) {
                    return Err(Error::new(format!(
                        "{field}.platforms contains duplicate platform {platform:?}"
                    )));
                }
            }
        }

        let mut hook_keys = HashSet::new();
        for (index, hook) in contributes.hooks.iter().enumerate() {
            let field = format!("contributes.hooks[{index}]");
            if !p.contribution_key.is_match(&hook.key) {
                return Err(Error::new(format!("{field}.key is invalid")));
            }
            if !hook_keys.insert(hook.key.clone()) {
                return Err(Error::new(format!("duplicate hook key {:?}", hook.key)));
            }
            validate_display_text(&format!("{field}.name"), &hook.name, 160)?;
            // The description is what an agent reads as the MCP tool
            // description, so it is mandatory and may be multi-line.
            if hook.description.trim().is_empty() || hook.description.len() > 2000 {
                return Err(Error::new(format!(
                    "{field}.description must be non-empty and at most 2000 bytes"
                )));
            }
            if let Some(schema) = &hook.input_schema {
                validate_input_schema(&field, schema)?;
            }
            if hook.triggers.is_empty() {
                return Err(Error::new(format!("{field}.triggers must not be empty")));
            }
            let mut trigger_seen = HashSet::new();
            for trigger in &hook.triggers {
                match trigger.as_str() {
                    TRIGGER_UI | TRIGGER_MANUAL | TRIGGER_AGENT | TRIGGER_EVENT => {}
                    other => {
                        return Err(Error::new(format!(
                            "{field}.triggers contains unsupported trigger {other:?}"
                        )));
                    }
                }
                if !trigger_seen.insert(trigger.clone()) {
                    return Err(Error::new(format!(
                        "{field}.triggers contains duplicate trigger {trigger:?}"
                    )));
                }
            }
            if trigger_seen.contains(TRIGGER_EVENT) {
                if hook.events.is_empty() {
                    return Err(Error::new(format!(
                        "{field}.events must not be empty when the event trigger is declared"
                    )));
                }
                let mut event_seen = HashSet::new();
                for event in &hook.events {
                    if !is_known_event(event) {
                        return Err(Error::new(format!(
                            "{field}.events contains unsupported event {event:?}"
                        )));
                    }
                    if !event_seen.insert(event.clone()) {
                        return Err(Error::new(format!(
                            "{field}.events contains duplicate event {event:?}"
                        )));
                    }
                    // An event PUSHES the same content the Action API would have
                    // required a scope to pull: issue.* carries the description,
                    // comment.created carries the body. Without this,
                    // subscribing is a way to receive what reading was not
                    // granted — one dataset with two standards. Enforced at
                    // install, so the consent screen shows the read scope the
                    // subscription actually implies.
                    if let Some(required) = event_read_scope(event) {
                        if !has_scope(&self.scopes, required) {
                            return Err(Error::new(format!(
                                "{field}.events subscribes to {event:?}, which delivers content requiring the {required} scope"
                            )));
                        }
                    }
                }
            } else if !hook.events.is_empty() {
                return Err(Error::new(format!(
                    "{field}.events requires the event trigger"
                )));
            }
            self.validate_hook_transport(&field, &hook.transport)?;
            if hook.timeout_ms != 0 && !(100..=30000).contains(&hook.timeout_ms) {
                return Err(Error::new(format!(
                    "{field}.timeout_ms must be between 100 and 30000"
                )));
            }
        }

        let mut resource_keys = HashSet::new();
        for (index, resource) in contributes.resources.iter().enumerate() {
            let field = format!("contributes.resources[{index}]");
            if resource.resource_type != RESOURCE_SKILL {
                return Err(Error::new(format!(
                    "{field}.type is unsupported: {:?}",
                    resource.resource_type
                )));
            }
            if !p.contribution_key.is_match(&resource.key) {
                return Err(Error::new(format!("{field}.key is invalid")));
            }
            if !resource_keys.insert(resource.key.clone()) {
                return Err(Error::new(format!(
                    "duplicate resource key {:?}",
                    resource.key
                )));
            }
            validate_relative_path(&format!("{field}.entry"), &resource.entry)?;
            let want = format!("skills/{}/SKILL.md", resource.key);
            if resource.entry != want {
                return Err(Error::new(format!("{field}.entry must be {want:?}")));
            }
        }
        Ok(())
    }

    // A hook can only reach a host the manifest declared through a `net:`
    // scope. Without this the consent screen would not describe where plugin
    // data actually goes.
    fn validate_hook_transport(
        &self,
        field: &str,
        transport: &crate::types::HookTransport,
    ) -> Result<()> {
        match transport.transport_type.as_str() {
            TRANSPORT_HTTP | TRANSPORT_MCP => {}
            other => {
                return Err(Error::new(format!(
                    "{field}.transport.type is unsupported: {other:?}"
                )));
            }
        }
        validate_https_url(&format!("{field}.transport.url"), &transport.url)?;
        let endpoint = url::Url::parse(&transport.url)
            .map_err(|_| Error::new(format!("{field}.transport.url is invalid")))?;
        // Exact host, never a suffix match. The consent screen renders one line
        // per scope ("send data to example.com"), and the same scope list
        // becomes the iframe's CSP connect-src, which is exact-host. A suffix
        // match here would make one scope string mean two different things in
        // two places. A plugin that needs a subdomain declares it:
        // net:api.example.com.
        let raw_host = endpoint.host_str().unwrap_or_default();
        let host = raw_host
            .strip_suffix('.')
            .unwrap_or(raw_host)
            .to_lowercase();
        for domain in crate::capabilities::net_domains(&self.scopes) {
            if host == domain {
                return Ok(());
            }
        }
        Err(Error::new(format!(
            "{field}.transport.url host {host:?} is not covered by a net: scope"
        )))
    }
}

fn validate_input_schema(field: &str, schema: &serde_json::value::RawValue) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(schema.get())
        .map_err(|e| Error::new(format!("{field}.input_schema must be a JSON object: {e}")))?;
    match value {
        serde_json::Value::Object(map) => {
            if map.get("type").and_then(|v| v.as_str()) != Some("object") {
                return Err(Error::new(format!(
                    "{field}.input_schema.type must be object"
                )));
            }
            Ok(())
        }
        // Go unmarshals `null` into a nil map without error, then fails the
        // type check — mirror that outcome.
        serde_json::Value::Null => Err(Error::new(format!(
            "{field}.input_schema.type must be object"
        ))),
        _ => Err(Error::new(format!(
            "{field}.input_schema must be a JSON object"
        ))),
    }
}

/// Reports whether a single scope string is one this host defines.
pub fn validate_scope(scope: &str) -> Result<()> {
    if is_fixed_scope(scope) {
        return Ok(());
    }
    if let Some(domain) = scope.strip_prefix(SCOPE_NET_PREFIX) {
        let Some(p) = patterns() else {
            return Err(missing_patterns());
        };
        if domain.len() > 253 || !p.net_domain.is_match(domain) {
            return Err(Error::new(format!(
                "net scope has an invalid domain {domain:?}"
            )));
        }
        return Ok(());
    }
    Err(Error::new(format!("unsupported scope {scope:?}")))
}

fn validate_plugin_key(key: &str) -> Result<()> {
    if key.len() > 255 {
        return Err(Error::new("key exceeds 255 bytes"));
    }
    let segments: Vec<&str> = key.split('.').collect();
    if segments.len() < 2 {
        return Err(Error::new("key must use a reverse-DNS namespace"));
    }
    let Some(p) = patterns() else {
        return Err(missing_patterns());
    };
    for segment in segments {
        if !p.plugin_key_segment.is_match(segment) {
            return Err(Error::new(format!(
                "key contains invalid segment {segment:?}"
            )));
        }
    }
    Ok(())
}

fn validate_display_text(field: &str, value: &str, max_bytes: usize) -> Result<()> {
    if value.is_empty() || value.trim() != value {
        return Err(Error::new(format!(
            "{field} must be non-empty without surrounding whitespace"
        )));
    }
    if value.contains('\r') || value.contains('\n') {
        return Err(Error::new(format!("{field} must be single-line")));
    }
    if value.len() > max_bytes {
        return Err(Error::new(format!("{field} exceeds {max_bytes} bytes")));
    }
    Ok(())
}

fn validate_relative_path(field: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 1024 {
        return Err(Error::new(format!(
            "{field} must be a relative path of at most 1024 bytes"
        )));
    }
    let Some(p) = patterns() else {
        return Err(missing_patterns());
    };
    if !p.relative_path.is_match(value) {
        return Err(Error::new(format!(
            "{field} must be a relative path without protocol, leading slash, or traversal"
        )));
    }
    for segment in value.split('/') {
        if segment == "." || segment == ".." {
            return Err(Error::new(format!(
                "{field} must not contain path traversal"
            )));
        }
    }
    Ok(())
}

fn validate_https_url(field: &str, value: &str) -> Result<()> {
    if value.len() > 2048 {
        return Err(Error::new(format!("{field} exceeds 2048 bytes")));
    }
    let reject = || Error::new(format!("{field} must be a plain HTTPS URL"));
    let parsed = url::Url::parse(value.trim()).map_err(|_| reject())?;
    if parsed.scheme() != "https"
        || parsed.host_str().unwrap_or_default().is_empty()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(reject());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Validators surface a compile failure as an internal error instead of
    // panicking; this pins that every pattern literal actually compiles so the
    // fallback path stays unreachable.
    #[test]
    fn validation_patterns_compile() {
        assert!(patterns().is_some());
    }
}
