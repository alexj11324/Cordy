//! Plugin skill resources and frontmatter handling.
//!
//! A plugin's `skill` resource becomes an ordinary row in the existing skill
//! table. Not a plugin-owned copy, not a bundle, not an artifact with a digest:
//! the previous plugin system built all of that across fourteen tables and, at
//! the end of it, delivered one SKILL.md that this table could already hold.
//!
//! The only thing the platform has to remember is which installation contributed
//! which skill, so uninstall removes exactly those and nothing a person wrote.
//! That is one nullable column.
//!
//! A resource is not a hook — nothing calls anything. The file is fetched once,
//! at install, from the same origin as the manifest, and after that it is just
//! a skill.

use sqlx::PgConnection;
use uuid::Uuid;

use cordy_db::models::PluginInstallation;
use cordy_plugincontract::{Manifest, Resource, RESOURCE_SKILL};

use crate::plugin::{
    fetch_dev_manifest, is_dev_origin, is_unique_violation_anyhow, plugin_errf,
    read_local_file_pub, PluginError, PluginErrorKind, PluginService, LOCAL_SOURCE_PREFIX,
};

/// Bounds one fetched SKILL.md. Generous for prose, small enough that a source
/// URL cannot use the install path to push a large body into the database.
const MAX_SKILL_BYTES: usize = 256 * 1024;

/// Writes the manifest's skill resources and prunes the ones this installation
/// no longer declares.
///
/// Called inside the install transaction: a plugin that half-installs its skills
/// is worse than one that fails, because the missing half is invisible.
pub(crate) async fn install_skill_resources_in_tx(
    service: &PluginService,
    tx: &mut PgConnection,
    installation: &PluginInstallation,
    manifest: &Manifest,
    source_url: &str,
    user_id: Uuid,
) -> Result<(), PluginError> {
    let resources = skill_resources(manifest);

    // Prune first, so a rename frees its old name before the new one is
    // written. The reverse order would collide with the table's unique
    // (workspace_id, name) on any rename that only changes case or spacing.
    let keep: Vec<String> = resources.iter().map(|r| r.key.clone()).collect();
    cordy_db::queries::plugin::delete_plugin_skills_not_in(&mut *tx, installation.id, &keep)
        .await
        .map_err(|e| {
            PluginError::with_source(
                PluginErrorKind::Unavailable,
                "prune plugin skills",
                crate::plugin::box_anyhow(e),
            )
        })?;
    if resources.is_empty() {
        return Ok(());
    }

    for resource in &resources {
        let content = fetch_skill_resource(service, source_url, &resource.entry).await?;
        let (_, parsed_description) = parse_skill_frontmatter(&content);
        // The manifest key is authoritative for the name, not the frontmatter.
        // The consent screen listed the key, the tool namespace uses it, and a
        // file that disagrees must not silently install under another name.
        let name = resource.key.clone();
        let description = if parsed_description.trim().is_empty() {
            format!("Provided by the {} Plugin.", manifest.name)
        } else {
            parsed_description
        };

        if let Err(e) = cordy_db::queries::plugin::upsert_plugin_skill(
            &mut *tx,
            installation.workspace_id,
            &name,
            &description,
            &content,
            installation.id,
            user_id,
        )
        .await
        {
            if is_unique_violation_anyhow(&e) {
                return Err(plugin_errf(
                    PluginErrorKind::Conflict,
                    format!("a skill named {name:?} already exists in this workspace"),
                ));
            }
            return Err(PluginError::with_source(
                PluginErrorKind::Unavailable,
                "install plugin skill",
                crate::plugin::box_anyhow(e),
            ));
        }
    }
    Ok(())
}

fn skill_resources(manifest: &Manifest) -> Vec<&Resource> {
    manifest
        .contributes
        .resources
        .iter()
        .filter(|resource| resource.resource_type == RESOURCE_SKILL)
        .collect()
}

/// Reads one SKILL.md from beside the manifest.
///
/// Resolved relative to the source URL rather than taken as a URL of its own:
/// the manifest already passed the origin checks, and letting a resource name
/// an arbitrary address would hand the install path a second, unreviewed fetch
/// target. `entry` is validated at parse time to be a relative path with no
/// traversal, which is what makes this safe to join.
async fn fetch_skill_resource(
    service: &PluginService,
    source_url: &str,
    entry: &str,
) -> Result<String, PluginError> {
    if let Some(name) = source_url.strip_prefix(LOCAL_SOURCE_PREFIX) {
        let raw = read_local_file_pub(service, name, entry)?;
        return String::from_utf8(raw).map_err(|e| {
            PluginError::with_source(PluginErrorKind::Invalid, "read plugin skill", e)
        });
    }

    let base = url::Url::parse(source_url)
        .map_err(|_| plugin_errf(PluginErrorKind::Invalid, "source_url is not a valid URL"))?;
    // Go's base.JoinPath("..", entry): resolve one level up from the source
    // path (the manifest filename), then join the entry. url::Url::join with a
    // relative path replaces the last segment — equivalent to joining "..",
    // then the entry.
    let resolved = base.join("../").unwrap_or(base).join(entry).map_err(|_| {
        plugin_errf(
            PluginErrorKind::Invalid,
            format!("plugin skill {entry:?} does not resolve against its source"),
        )
    })?;
    let resolved = resolved.to_string();

    let raw = if is_dev_origin(&service.dev_origins, &resolved) {
        fetch_dev_manifest(&resolved).await?
    } else {
        crate::plugin::fetch_remote_manifest_pub(&resolved).await?
    };
    if raw.len() > MAX_SKILL_BYTES {
        return Err(plugin_errf(
            PluginErrorKind::Invalid,
            format!("plugin skill {entry} exceeds {MAX_SKILL_BYTES} bytes"),
        ));
    }
    let text = String::from_utf8(raw)
        .map_err(|e| PluginError::with_source(PluginErrorKind::Invalid, "read plugin skill", e))?;
    if text.trim().is_empty() {
        return Err(plugin_errf(
            PluginErrorKind::Invalid,
            format!("plugin skill {entry} is empty"),
        ));
    }
    Ok(text)
}

// ---------------------------------------------------------------------------
// Frontmatter
// ---------------------------------------------------------------------------

/// Extracts name and description from the YAML frontmatter block of a SKILL.md
/// file. Returns empty strings when the frontmatter is absent or malformed so
/// callers keep treating missing metadata as non-fatal.
///
/// Values are decoded into a generic map and coerced per key rather than
/// unmarshalled into a string struct: a structured value in one field never
/// discards a valid sibling key, mirroring Go's map decode.
///
/// Trimmed because both fields are single-line labels wherever they are
/// consumed, while YAML block scalars carry a trailing newline by clip chomping
/// (MUL-5645).
pub fn parse_skill_frontmatter(content: &str) -> (String, String) {
    let Some(block) = frontmatter_block(content) else {
        return (String::new(), String::new());
    };
    let Ok(fm) = serde_yaml::from_str::<serde_yaml::Value>(&block) else {
        return (String::new(), String::new());
    };
    (
        coerce_frontmatter_value(fm.get("name")).trim().to_string(),
        coerce_frontmatter_value(fm.get("description"))
            .trim()
            .to_string(),
    )
}

/// Splits the `---\n...\n---` fence off the top of the file. Keeping the
/// trailing newline inside the captured group matters: YAML clip chomping only
/// preserves a final newline when the input itself contains one.
fn frontmatter_block(content: &str) -> Option<String> {
    if !content.starts_with("---") {
        return None;
    }
    // Tolerate \r\n line endings like Go's (?s)\A---\r?\n(.*?\r?\n)--- regex.
    let rest = content
        .strip_prefix("---\r\n")
        .or_else(|| content.strip_prefix("---\n"))?;
    let mut candidates = ["\n---", "\r\n---"];
    // Find the earliest closing fence; the regex is non-greedy.
    let mut best: Option<usize> = None;
    for marker in candidates.iter_mut() {
        if let Some(idx) = rest.find(*marker) {
            best = Some(match best {
                Some(current) => current.min(idx),
                None => idx,
            });
        }
    }
    let idx = best?;
    Some(rest[..idx].to_string())
}

/// Renders a decoded YAML value as a string, mirroring the TS side: null
/// becomes empty, strings pass through, other scalars use their literal form,
/// and structured values (sequences/mappings) are JSON-encoded.
fn coerce_frontmatter_value(value: Option<&serde_yaml::Value>) -> String {
    match value {
        None | Some(serde_yaml::Value::Null) => String::new(),
        Some(serde_yaml::Value::String(s)) => s.clone(),
        Some(serde_yaml::Value::Bool(b)) => b.to_string(),
        Some(serde_yaml::Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                // Go's strconv.FormatFloat(f, 'g', -1, 64) shortest form.
                format_go_g(n.as_f64().unwrap_or_default())
            }
        }
        Some(other) => serde_json::to_string(&json_from_yaml(other)).unwrap_or_default(),
    }
}

fn json_from_yaml(value: &serde_yaml::Value) -> serde_json::Value {
    match value {
        serde_yaml::Value::Null => serde_json::Value::Null,
        serde_yaml::Value::Bool(b) => serde_json::Value::Bool(*b),
        serde_yaml::Value::Number(n) => serde_json::Value::Number(
            serde_json::Number::from_f64(n.as_f64().unwrap_or_default())
                .unwrap_or_else(|| serde_json::Number::from(0)),
        ),
        serde_yaml::Value::String(s) => serde_json::Value::String(s.clone()),
        serde_yaml::Value::Sequence(seq) => {
            serde_json::Value::Array(seq.iter().map(json_from_yaml).collect())
        }
        serde_yaml::Value::Mapping(map) => serde_json::Value::Object(
            map.iter()
                .filter_map(|(k, v)| {
                    let key = match k {
                        serde_yaml::Value::String(s) => s.clone(),
                        other => serde_yaml_to_string_key(other)?,
                    };
                    Some((key, json_from_yaml(v)))
                })
                .collect(),
        ),
        serde_yaml::Value::Tagged(tagged) => json_from_yaml(&tagged.value),
    }
}

fn serde_yaml_to_string_key(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Shortest-roundtrip float formatting matching Go's 'g' verb closely enough
/// for JSON encoding of description metadata.
fn format_go_g(value: f64) -> String {
    let mut candidate = format!("{value}");
    if candidate.contains('e') {
        candidate = format!("{value:e}");
    }
    candidate
}
