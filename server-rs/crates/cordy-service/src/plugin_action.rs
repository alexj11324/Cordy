//! Action API authorization — port of `server/internal/service/plugin_action.go`.
//!
//! [`PluginActionCaller`] is one authorized Action API call: which installation
//! is speaking, in which workspace, on whose behalf.
//!
//! The plugin never holds a credential. A surface talks to the host page over
//! postMessage and the host re-issues the call with the signed-in user's own
//! session, so the identity here is always a real member — a plugin cannot act
//! as anyone but the person using it.

use serde::Serialize;
use uuid::Uuid;

use cordy_db::models::{Issue, PluginInstallation, User, Workspace};

use crate::plugin::{
    decode_scopes, json_bytes, parse_uuid_value, plugin_errf, uuid_string, PluginError,
    PluginErrorKind,
};

#[derive(Debug, Clone)]
pub struct PluginActionCaller {
    pub installation: PluginInstallation,
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    /// The consented set, read from the installation row rather than from the
    /// manifest the source URL serves today.
    pub scopes: Vec<String>,
    /// When set, is the ONLY issue this caller may touch.
    ///
    /// Set for a callback token issued against a specific issue. A hook called
    /// about one issue has no business reaching another: without this the grant
    /// is worth every issue in the workspace that its actor can see, for the
    /// whole five minutes it lives. None means unrestricted — a session caller,
    /// or an invocation that had no issue — and the ordinary workspace and
    /// membership checks still apply on top.
    pub issue_scope: Option<Uuid>,
}

/// Performs the two checks that belong to the plugin: the installation is real
/// and enabled, and it holds the scope this call needs.
///
/// It deliberately does NOT decide whether the caller may touch the resource.
/// That is the third check, and it stays with the normal resource loaders so a
/// plugin inherits exactly the permissions of the user behind it — no more, and
/// no separate copy of the permission rules to drift.
pub async fn authorize_plugin_action(
    pool: &sqlx::PgPool,
    installation_id: &str,
    user_id: Uuid,
    scope: &str,
) -> Result<PluginActionCaller, PluginError> {
    if installation_id.is_empty() {
        return Err(plugin_errf(
            PluginErrorKind::Invalid,
            "plugin installation is required",
        ));
    }
    let parsed = parse_uuid_value(installation_id)
        .map_err(|_| plugin_errf(PluginErrorKind::NotFound, "plugin installation not found"))?;
    let installation = cordy_db::queries::plugin::get_plugin_installation(pool, parsed).await;
    let installation = match installation {
        Ok(Some(installation)) => installation,
        Ok(None) => {
            return Err(plugin_errf(
                PluginErrorKind::NotFound,
                "plugin installation not found",
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
    // A disabled plugin is off, not merely hidden: an iframe left open in a
    // stale tab must not keep working after an admin disables it.
    if !installation.enabled {
        return Err(plugin_errf(
            PluginErrorKind::Forbidden,
            "this Plugin is disabled",
        ));
    }

    let scopes = decode_scopes(&json_bytes(&installation.granted_scopes)).unwrap_or_default();
    if !scope.is_empty() && !has_scope(&scopes, scope) {
        return Err(plugin_errf(
            PluginErrorKind::Forbidden,
            format!("this Plugin was not granted the {scope} scope"),
        ));
    }

    Ok(PluginActionCaller {
        workspace_id: installation.workspace_id,
        scopes,
        installation,
        user_id,
        issue_scope: None,
    })
}

pub fn has_scope(scopes: &[String], want: &str) -> bool {
    scopes.iter().any(|scope| scope == want)
}

/// Renders the human-facing issue key ("MUL-42"). Callers that resolve the
/// workspace prefix defensively may pass "": a failed workspace lookup should
/// not surface as a stray "-42", so the number stands alone as "#42". The HTTP
/// layer never passes "" — the handler derives a prefix from the workspace name
/// before rendering anything.
pub fn issue_identifier(issue_prefix: &str, number: i32) -> String {
    if issue_prefix.is_empty() {
        format!("#{number}")
    } else {
        format!("{issue_prefix}-{number}")
    }
}

/// What a surface gets before it has asked for anything: who is looking, where,
/// and which issue the panel is mounted on. It requires no scope because it
/// contains nothing the user in front of the iframe cannot already see on the
/// page around it.
#[derive(Debug, Serialize)]
pub struct PluginContext {
    pub workspace: PluginContextWorkspace,
    /// Absent when the caller is the plugin itself — an event hook or a
    /// standing install token has no person behind it, and inventing one here
    /// would let a handler believe it is acting for somebody.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<PluginContextUser>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<PluginContextIssue>,
    pub config: serde_json::Map<String, serde_json::Value>,
    #[serde(rename = "granted_net_domains")]
    pub granted_urls: Vec<String>,
    /// Names which of the two it is, so a handler can branch without inferring
    /// it from a missing field.
    pub actor: String,
}

#[derive(Debug, Serialize)]
pub struct PluginContextWorkspace {
    pub id: String,
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Serialize)]
pub struct PluginContextUser {
    pub id: String,
    pub name: String,
    // Email is deliberately absent: a surface that needs to identify a member
    // to its own backend should use the opaque id, not a mailbox.
}

#[derive(Debug, Serialize)]
pub struct PluginContextIssue {
    pub id: String,
    pub identifier: String,
    pub title: String,
}

/// Assembles the context payload. Config carries only the non-secret
/// installation values — secrets live in their own table and have no read path,
/// so there is no way for one to reach the iframe.
pub fn build_plugin_context(
    caller: &PluginActionCaller,
    workspace: &Workspace,
    user: Option<&User>,
    issue: Option<&Issue>,
) -> PluginContext {
    let config = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&json_bytes(
        &caller.installation.config,
    ))
    .unwrap_or_default();

    let mut payload = PluginContext {
        workspace: PluginContextWorkspace {
            id: uuid_string(workspace.id),
            name: workspace.name.clone(),
            slug: workspace.slug.clone(),
        },
        config,
        granted_urls: cordy_plugincontract::net_domains(&caller.scopes),
        actor: "plugin".to_string(),
        user: None,
        issue: None,
    };
    if let Some(user) = user {
        payload.actor = "member".to_string();
        payload.user = Some(PluginContextUser {
            id: uuid_string(user.id),
            name: user.name.clone(),
        });
    }
    if let Some(issue) = issue {
        payload.issue = Some(PluginContextIssue {
            id: uuid_string(issue.id),
            identifier: issue_identifier(&workspace.issue_prefix, issue.number),
            title: issue.title.clone(),
        });
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_identifier_renders_the_human_facing_key() {
        assert_eq!(issue_identifier("MUL", 42), "MUL-42");
        assert_eq!(issue_identifier("", 42), "#42");
    }

    #[test]
    fn has_scope_is_an_exact_match() {
        let scopes = vec!["issues:read".to_string(), "net:x.com".to_string()];
        assert!(has_scope(&scopes, "issues:read"));
        assert!(!has_scope(&scopes, "issues:read:more"));
        assert!(!has_scope(&scopes, "issues:write"));
    }
}
