//! Plugin key/value storage.
//!
//! Scopes: there are exactly two, and adding a third is a product decision,
//! not a configuration knob.

use chrono::Utc;
use uuid::Uuid;

use crate::plugin::{plugin_errf, PluginError, PluginErrorKind};

/// Team-shared state. scope_id is the workspace id.
pub const PLUGIN_STORAGE_WORKSPACE: &str = "workspace";
/// Per-member state, including that member's own credentials for the plugin's
/// external service. scope_id is the user id.
pub const PLUGIN_STORAGE_USER: &str = "user";

/// Storage quotas. Enforced on write and never by eviction: a plugin that hits
/// its ceiling gets an explicit error, because silently dropping the oldest key
/// would corrupt state the plugin believes it stored.
pub const MAX_PLUGIN_STORAGE_KEY_BYTES: usize = 1024;
pub const MAX_PLUGIN_STORAGE_VALUE_BYTES: usize = 100 * 1024;
pub const MAX_PLUGIN_STORAGE_KEYS: i64 = 1000;
pub const MAX_PLUGIN_STORAGE_TOTAL_BYTES: i64 = 5 * 1024 * 1024;

/// One key in a listing. Values are deliberately absent: listing is for a
/// plugin to discover what it wrote, not to bulk-read state.
#[derive(Debug, serde::Serialize)]
pub struct PluginStorageKey {
    pub key: String,
    pub size_bytes: i64,
    /// RFC3339 UTC, matching Go's `.UTC().Format(time.RFC3339)`.
    pub updated_at: String,
}

/// Maps a scope name to the row's scope_id.
pub fn resolve_storage_scope(
    scope_type: &str,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Uuid, PluginError> {
    match scope_type {
        PLUGIN_STORAGE_WORKSPACE => Ok(workspace_id),
        PLUGIN_STORAGE_USER => Ok(user_id),
        _ => Err(plugin_errf(
            PluginErrorKind::Invalid,
            format!(
                "storage scope must be {:?} or {:?}",
                PLUGIN_STORAGE_WORKSPACE, PLUGIN_STORAGE_USER
            ),
        )),
    }
}

fn validate_storage_key(key: &str) -> Result<(), PluginError> {
    if key.is_empty() {
        return Err(plugin_errf(
            PluginErrorKind::Invalid,
            "storage key is required",
        ));
    }
    if key.len() > MAX_PLUGIN_STORAGE_KEY_BYTES {
        return Err(plugin_errf(
            PluginErrorKind::Quota,
            format!("storage key exceeds {MAX_PLUGIN_STORAGE_KEY_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// Reads one value. This path can never reach plugin_secret: secrets live in a
/// different table with no read query that returns ciphertext to a request
/// handler.
pub async fn get_storage_value(
    pool: &sqlx::PgPool,
    installation_id: Uuid,
    scope_type: &str,
    scope_id: Uuid,
    key: &str,
) -> Result<String, PluginError> {
    validate_storage_key(key)?;
    let row = cordy_db::queries::plugin::get_plugin_storage_value(
        pool,
        installation_id,
        scope_type,
        scope_id,
        key,
    )
    .await;
    match row {
        Ok(Some(row)) => Ok(row.value),
        Ok(None) => Err(plugin_errf(
            PluginErrorKind::NotFound,
            "storage key not found",
        )),
        Err(e) => Err(PluginError::with_source(
            PluginErrorKind::Unavailable,
            "read plugin storage",
            crate::plugin::box_anyhow(e),
        )),
    }
}

pub async fn list_storage_keys(
    pool: &sqlx::PgPool,
    installation_id: Uuid,
    scope_type: &str,
    scope_id: Uuid,
) -> Result<Vec<PluginStorageKey>, PluginError> {
    let rows = cordy_db::queries::plugin::list_plugin_storage_keys(
        pool,
        installation_id,
        scope_type,
        scope_id,
    )
    .await
    .map_err(|e| {
        PluginError::with_source(
            PluginErrorKind::Unavailable,
            "list plugin storage",
            crate::plugin::box_anyhow(e),
        )
    })?;
    Ok(rows
        .into_iter()
        .map(|row| PluginStorageKey {
            updated_at: row
                .updated_at
                .map(|t| t.with_timezone(&Utc))
                .unwrap_or_else(Utc::now)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            key: row.key,
            size_bytes: row.size_bytes,
        })
        .collect())
}

/// Decides whether one write fits, given the scope's usage with the candidate
/// key excluded. Kept pure and separate from the write so the three limits have
/// one canonical test, and so it is obvious that every bound is compared in
/// bytes — the usage query reports octet_length for the same reason.
///
/// Known bound: usage is read before the write without a lock, so concurrent
/// writers to the same scope can overshoot by their own sizes. Serializing every
/// plugin KV write to close that gap costs more than the overshoot is worth; the
/// limits exist to stop runaway growth, not to be exact to the byte.
pub fn enforce_storage_quota(
    usage: cordy_db::queries::plugin::GetPluginStorageUsageRow,
    value_bytes: usize,
) -> Result<(), PluginError> {
    if value_bytes > MAX_PLUGIN_STORAGE_VALUE_BYTES {
        return Err(plugin_errf(
            PluginErrorKind::Quota,
            format!("storage value exceeds {MAX_PLUGIN_STORAGE_VALUE_BYTES} bytes"),
        ));
    }
    if usage.key_count + 1 > MAX_PLUGIN_STORAGE_KEYS {
        return Err(plugin_errf(
            PluginErrorKind::Quota,
            format!("storage scope already holds the maximum of {MAX_PLUGIN_STORAGE_KEYS} keys"),
        ));
    }
    if usage.total_bytes + value_bytes as i64 > MAX_PLUGIN_STORAGE_TOTAL_BYTES {
        return Err(plugin_errf(
            PluginErrorKind::Quota,
            format!("storage scope exceeds its {MAX_PLUGIN_STORAGE_TOTAL_BYTES} byte budget"),
        ));
    }
    Ok(())
}

/// Writes one value after checking every quota. The usage query excludes the
/// key being written, so replacing a value is measured as a replacement and
/// cannot fail a limit the existing row already occupies.
pub async fn set_storage_value(
    pool: &sqlx::PgPool,
    installation_id: Uuid,
    scope_type: &str,
    scope_id: Uuid,
    key: &str,
    value: &str,
) -> Result<(), PluginError> {
    validate_storage_key(key)?;
    let usage = cordy_db::queries::plugin::get_plugin_storage_usage(
        pool,
        installation_id,
        scope_type,
        scope_id,
        key,
    )
    .await
    .map_err(|e| {
        PluginError::with_source(
            PluginErrorKind::Unavailable,
            "read plugin storage usage",
            crate::plugin::box_anyhow(e),
        )
    })?
    .unwrap_or(cordy_db::queries::plugin::GetPluginStorageUsageRow {
        key_count: 0,
        total_bytes: 0,
    });
    enforce_storage_quota(usage, value.len())?;
    cordy_db::queries::plugin::upsert_plugin_storage_value(
        pool,
        installation_id,
        scope_type,
        scope_id,
        key,
        value,
    )
    .await
    .map_err(|e| {
        PluginError::with_source(
            PluginErrorKind::Unavailable,
            "write plugin storage",
            crate::plugin::box_anyhow(e),
        )
    })?;
    Ok(())
}

pub async fn delete_storage_value(
    pool: &sqlx::PgPool,
    installation_id: Uuid,
    scope_type: &str,
    scope_id: Uuid,
    key: &str,
) -> Result<(), PluginError> {
    validate_storage_key(key)?;
    let deleted = cordy_db::queries::plugin::delete_plugin_storage_value(
        pool,
        installation_id,
        scope_type,
        scope_id,
        key,
    )
    .await
    .map_err(|e| {
        PluginError::with_source(
            PluginErrorKind::Unavailable,
            "delete plugin storage",
            crate::plugin::box_anyhow(e),
        )
    })?;
    if deleted == 0 {
        return Err(plugin_errf(
            PluginErrorKind::NotFound,
            "storage key not found",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_storage_scope_accepts_only_the_two_scopes() {
        let ws = Uuid::now_v7();
        let user = Uuid::now_v7();
        assert_eq!(
            resolve_storage_scope("workspace", ws, user).unwrap(),
            ws,
            "workspace scope resolves to the workspace id"
        );
        assert_eq!(
            resolve_storage_scope("user", ws, user).unwrap(),
            user,
            "user scope resolves to the user id"
        );
        assert_eq!(
            resolve_storage_scope("global", ws, user).unwrap_err().kind,
            PluginErrorKind::Invalid,
            "a third scope is a product decision, not a config knob"
        );
    }

    fn usage(
        key_count: i64,
        total_bytes: i64,
    ) -> cordy_db::queries::plugin::GetPluginStorageUsageRow {
        cordy_db::queries::plugin::GetPluginStorageUsageRow {
            key_count,
            total_bytes,
        }
    }

    #[test]
    fn enforce_storage_quota_rejects_each_bound_with_its_own_message() {
        assert_eq!(
            enforce_storage_quota(usage(0, 0), MAX_PLUGIN_STORAGE_VALUE_BYTES + 1)
                .unwrap_err()
                .message,
            format!("storage value exceeds {MAX_PLUGIN_STORAGE_VALUE_BYTES} bytes")
        );
        assert_eq!(
            enforce_storage_quota(usage(MAX_PLUGIN_STORAGE_KEYS, 0), 1)
                .unwrap_err()
                .message,
            format!("storage scope already holds the maximum of {MAX_PLUGIN_STORAGE_KEYS} keys")
        );
        assert_eq!(
            enforce_storage_quota(usage(0, MAX_PLUGIN_STORAGE_TOTAL_BYTES), 1)
                .unwrap_err()
                .message,
            format!("storage scope exceeds its {MAX_PLUGIN_STORAGE_TOTAL_BYTES} byte budget")
        );

        // Exactly at each bound passes — the limits are ceilings, not floors.
        assert!(enforce_storage_quota(usage(0, 0), MAX_PLUGIN_STORAGE_VALUE_BYTES).is_ok());
        assert!(enforce_storage_quota(usage(MAX_PLUGIN_STORAGE_KEYS - 1, 0), 1).is_ok());
        assert!(enforce_storage_quota(usage(0, MAX_PLUGIN_STORAGE_TOTAL_BYTES - 1), 1).is_ok());
    }

    #[test]
    fn validate_storage_key_bounds() {
        assert!(validate_storage_key("").is_err());
        assert!(validate_storage_key(&"k".repeat(MAX_PLUGIN_STORAGE_KEY_BYTES)).is_ok());
        assert!(validate_storage_key(&"k".repeat(MAX_PLUGIN_STORAGE_KEY_BYTES + 1)).is_err());
    }
}
