//! Plugin credentials — port of `server/internal/service/plugin_token.go`.
//!
//! Two credentials, moving in opposite directions.
//!
//! Neither one ever enters an iframe: a surface still holds nothing and still
//! reaches Cordy only by asking the host page over postMessage. What changes
//! with hooks is that a plugin now has a SERVER, and that server needs a way to
//! be recognised. So the honest statement about the system is no longer "there
//! are no plugin credentials" but "plugin credentials only move between
//! servers".
//!
//! ```text
//! install token  (mpi_…)  plugin -> host, long-lived, rotatable.
//!                         The host only ever verifies it, so it is stored
//!                         hashed and cannot be recovered from the database.
//! callback token          host -> plugin, minutes, one INVOCATION.
//!                         Handed to a hook handler so it can answer using the
//!                         Action API without being given standing access.
//! ```

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use base64::Engine;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::plugin::{plugin_errf, PluginError, PluginErrorKind};

pub const INSTALL_TOKEN_PREFIX: &str = "mpi_";
pub const CALLBACK_TOKEN_PREFIX: &str = "mpc_";
pub const CALLBACK_TOKEN_TTL: Duration = Duration::from_secs(5 * 60);
const CALLBACK_TOKEN_ENTROPY: usize = 32;

fn hash_token(token: &str) -> String {
    let sum = Sha256::digest(token.as_bytes());
    hex::encode(sum)
}

/// Issues a new install token and stores only its hash.
///
/// Returned in plaintext exactly once. There is no endpoint that reads it back —
/// an admin who loses it rotates rather than recovers, which is the same trade
/// every other bearer credential in the product makes.
pub async fn issue_install_token(
    pool: &sqlx::PgPool,
    installation_id: Uuid,
) -> Result<String, PluginError> {
    let mut raw = [0u8; CALLBACK_TOKEN_ENTROPY];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut raw);
    let token = format!(
        "{INSTALL_TOKEN_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
    );
    cordy_db::queries::plugin::set_plugin_installation_token(
        pool,
        installation_id,
        Some(&hash_token(&token)),
    )
    .await
    .map_err(|e| {
        PluginError::with_source(
            PluginErrorKind::Unavailable,
            "store install token",
            crate::plugin::box_anyhow(e),
        )
    })?;
    Ok(token)
}

/// Drops the stored hash, so nothing presented afterwards matches. Rotation is
/// [`issue_install_token`], which overwrites in place.
pub async fn revoke_install_token(
    pool: &sqlx::PgPool,
    installation_id: Uuid,
) -> Result<(), PluginError> {
    cordy_db::queries::plugin::set_plugin_installation_token(pool, installation_id, None)
        .await
        .map_err(|e| {
            PluginError::with_source(
                PluginErrorKind::Unavailable,
                "revoke install token",
                crate::plugin::box_anyhow(e),
            )
        })?;
    Ok(())
}

/// Resolves a presented token to its installation.
pub async fn authenticate_install_token(
    pool: &sqlx::PgPool,
    token: &str,
) -> Result<cordy_db::models::PluginInstallation, PluginError> {
    let token = token.trim();
    if !token.starts_with(INSTALL_TOKEN_PREFIX) {
        return Err(plugin_errf(
            PluginErrorKind::Forbidden,
            "invalid plugin token",
        ));
    }
    let installation = cordy_db::queries::plugin::get_plugin_installation_by_token_hash(
        pool,
        Some(&hash_token(token)),
    )
    .await;
    let installation = match installation {
        Ok(Some(installation)) => installation,
        Ok(None) => {
            return Err(plugin_errf(
                PluginErrorKind::Forbidden,
                "invalid plugin token",
            ))
        }
        Err(e) => {
            return Err(PluginError::with_source(
                PluginErrorKind::Unavailable,
                "load plugin installation",
                crate::plugin::box_anyhow(e),
            ))
        }
    };
    if !installation.enabled {
        return Err(plugin_errf(
            PluginErrorKind::Forbidden,
            "this Plugin is disabled",
        ));
    }
    Ok(installation)
}

/// Who the resulting writes belong to. Follows the trigger rather than the
/// plugin: a ui/manual hook runs because a person pressed something, so its
/// writes stay that person's; an event hook has no person behind it and must
/// not borrow the last one who happened to touch the issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookActor {
    pub actor_type: String,
    /// Zero UUID when the trigger has no person or agent behind it.
    pub id: Uuid,
}

/// What a redeemed callback token proves.
///
/// Its scopes are the installation's, never wider: the callback exists so a hook
/// handler can finish the job it was called for, not so an out-of-band request
/// can do more than the surface could.
#[derive(Debug, Clone)]
pub struct CallbackGrant {
    pub installation_id: Uuid,
    pub workspace_id: Uuid,
    pub hook_key: String,
    pub trigger: String,
    /// Who the resulting writes belong to, decided when the hook was
    /// dispatched. A handler cannot choose to write as somebody else.
    pub actor: HookActor,
    /// Narrows an event callback to the issue that produced it. `None` when the
    /// invocation had no issue.
    pub issue_id: Option<Uuid>,
    pub expires_at: SystemTime,
}

/// Issues and resolves the per-invocation callback tokens.
///
/// Scoped to one INVOCATION, not one request — and that distinction was found by
/// running a real handler rather than by reasoning about it. A single-use token
/// looked stricter and was: the reference handler reads the issue, decides, then
/// posts a comment, and the second call died on an already-spent token. Two calls
/// is the floor for any handler that does something with what it read.
///
/// What still bounds it: minutes, this installation's granted scopes, the actor
/// decided at dispatch, and the issue the invocation was about.
///
/// Held in memory. Across several instances or after a restart a token stops
/// resolving early rather than late, so the failure mode is a handler seeing 403
/// — visible and retriable — not a grant outliving its window.
#[derive(Default)]
pub struct CallbackTokens {
    issued: Mutex<HashMap<String, CallbackGrant>>,
}

impl CallbackTokens {
    pub fn new() -> Self {
        Self::default()
    }

    fn sweep_locked(issued: &mut HashMap<String, CallbackGrant>) {
        let now = SystemTime::now();
        issued.retain(|_, grant| now.duration_since(grant.expires_at).is_err());
    }

    /// Mints a token for one hook invocation. See [`crate::plugin_hook`]'s
    /// `HookInvocation` for the input shape.
    pub fn issue(&self, grant_parts: CallbackGrantParts<'_>) -> Result<String, PluginError> {
        let mut raw = [0u8; CALLBACK_TOKEN_ENTROPY];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut raw);
        let token = format!(
            "{CALLBACK_TOKEN_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
        );

        let grant = CallbackGrant {
            installation_id: grant_parts.installation_id,
            workspace_id: grant_parts.workspace_id,
            hook_key: grant_parts.hook_key.to_string(),
            trigger: grant_parts.trigger.to_string(),
            actor: grant_parts.actor.clone(),
            issue_id: grant_parts.issue_id,
            expires_at: SystemTime::now() + CALLBACK_TOKEN_TTL,
        };

        let mut issued = self.issued.lock().unwrap_or_else(|e| e.into_inner());
        Self::sweep_locked(&mut issued);
        issued.insert(hash_token(&token), grant);
        Ok(token)
    }

    /// Looks a token up. Valid for as many calls as the handler needs until it
    /// expires; see the type comment for why that is the right bound.
    pub fn resolve(&self, token: &str) -> Result<CallbackGrant, PluginError> {
        let token = token.trim();
        if !token.starts_with(CALLBACK_TOKEN_PREFIX) {
            return Err(plugin_errf(
                PluginErrorKind::Forbidden,
                "invalid callback token",
            ));
        }
        let key = hash_token(token);

        let mut issued = self.issued.lock().unwrap_or_else(|e| e.into_inner());
        Self::sweep_locked(&mut issued);
        match issued.get(&key) {
            Some(grant) => {
                if SystemTime::now().duration_since(grant.expires_at).is_ok() {
                    // Expired between sweep and read: remove it.
                    issued.remove(&key);
                    return Err(plugin_errf(
                        PluginErrorKind::Forbidden,
                        "callback token is expired or unknown",
                    ));
                }
                Ok(grant.clone())
            }
            None => Err(plugin_errf(
                PluginErrorKind::Forbidden,
                "callback token is expired or unknown",
            )),
        }
    }

    /// Drops a grant before its expiry. Called when a hook invocation has
    /// finished, so a token stops working the moment the work it was issued for
    /// is over rather than lingering for the rest of its window.
    pub fn revoke(&self, token: &str) {
        if token.is_empty() {
            return;
        }
        let mut issued = self.issued.lock().unwrap_or_else(|e| e.into_inner());
        issued.remove(&hash_token(token.trim()));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.issued.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// The fields of a [`HookInvocation`] needed to mint a callback token — a
/// projection so this module does not depend on the engine's types.
#[derive(Debug, Clone)]
pub struct CallbackGrantParts<'a> {
    pub installation_id: Uuid,
    pub workspace_id: Uuid,
    pub hook_key: &'a str,
    pub trigger: &'a str,
    pub actor: HookActor,
    pub issue_id: Option<Uuid>,
}

impl Default for HookActor {
    fn default() -> Self {
        Self {
            actor_type: "plugin".to_string(),
            id: Uuid::nil(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::uuid_string;
    use std::time::Duration;

    fn parts(hook: &str) -> CallbackGrantParts<'_> {
        CallbackGrantParts {
            installation_id: Uuid::now_v7(),
            workspace_id: Uuid::now_v7(),
            hook_key: hook,
            trigger: "event",
            actor: HookActor {
                actor_type: "plugin".to_string(),
                id: Uuid::nil(),
            },
            issue_id: None,
        }
    }

    #[test]
    fn issue_then_resolve_roundtrips_the_grant() {
        let tokens = CallbackTokens::new();
        let grant_parts = parts("summarize");
        let expected_installation = grant_parts.installation_id;
        let token = tokens.issue(grant_parts).unwrap();
        assert!(token.starts_with(CALLBACK_TOKEN_PREFIX));

        let grant = tokens.resolve(&token).unwrap();
        assert_eq!(grant.installation_id, expected_installation);
        assert_eq!(grant.hook_key, "summarize");
        assert_eq!(grant.trigger, "event");
    }

    #[test]
    fn resolve_is_valid_for_multiple_calls_until_expiry() {
        // A single-use token was tried and rejected: a handler reads then acts.
        let tokens = CallbackTokens::new();
        let token = tokens.issue(parts("k")).unwrap();
        tokens.resolve(&token).unwrap();
        tokens.resolve(&token).unwrap();
    }

    #[test]
    fn revoke_stops_a_token_before_its_expiry() {
        let tokens = CallbackTokens::new();
        let token = tokens.issue(parts("k")).unwrap();
        tokens.revoke(&token);
        assert!(tokens.resolve(&token).is_err());
    }

    #[test]
    fn unknown_or_wrong_prefix_tokens_are_refused() {
        let tokens = CallbackTokens::new();
        assert!(tokens.resolve("mpc_nope").is_err());
        assert!(tokens.resolve("mpi_wrong-prefix-for-callback").is_err());
    }

    #[test]
    fn sweep_drops_expired_grants_so_the_map_cannot_grow_unbounded() {
        let tokens = CallbackTokens::new();
        for _ in 0..8 {
            let _ = tokens.issue(parts("k")).unwrap();
        }
        assert_eq!(tokens.len(), 8);

        // Age everything out manually.
        {
            let mut issued = tokens.issued.lock().unwrap();
            for grant in issued.values_mut() {
                grant.expires_at = SystemTime::now() - Duration::from_secs(1);
            }
        }
        assert!(
            tokens.resolve("mpc_unknown").is_err(),
            "a lookup sweeps expired grants"
        );
        assert_eq!(tokens.len(), 0);
    }

    #[test]
    fn stored_hash_is_sha256_hex_of_the_full_prefix_plus_body() {
        let raw = "abc";
        assert_eq!(hash_token(raw), hex::encode(Sha256::digest(b"abc")));
    }

    #[test]
    fn uuid_string_helper_matches_display_form() {
        let id = Uuid::now_v7();
        assert_eq!(uuid_string(id), id.to_string());
    }
}
