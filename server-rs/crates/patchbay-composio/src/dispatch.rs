//! The per-task Composio MCP overlay builder.
//!

use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

/// The deterministic key under `mcpServers` used to place the Composio
/// session into the merged MCP config. Daemon-side merge is by server name,
/// so this constant is the integration's namespace: an agent's own
/// `mcp_config` entry named "composio" is overridden by this overlay on
/// purpose — the overlay carries the live user-scoped session URL.
pub const MCP_OVERLAY_SERVER_NAME: &str = "composio";

/// Wire shape of one MCP server entry in the Claude-style
/// `{"mcpServers": {...}}` config every supported runtime consumes.
///
/// `type: http` marks the entry as a streamable HTTP MCP endpoint. Headers
/// carry the Composio API key, so callers must NEVER log this struct
/// without redacting headers.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct ComposioMcpServer {
    #[serde(rename = "type")]
    pub r#type: String,
    pub url: String,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
}

/// The per-task overlay JSON written to agent_task_queue.runtime_mcp_overlay
/// and read by the daemon claim handler at task dispatch.
///
/// Shape is deliberately a subset of agent.mcp_config (Claude-style
/// `mcpServers` map) so the daemon's merge is a flat dictionary union keyed
/// by server name — pure substitution, nothing fancier.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct McpOverlayPayload {
    #[serde(rename = "mcpServers")]
    pub mcp_servers: BTreeMap<String, ComposioMcpServer>,
}

/// The connected-app projection consumed by the runtime-apps sidecar
/// generators (mirrors patchbay_service::runtime_apps::ConnectedApp).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ConnectedApp {
    pub provider: String,
    pub server_name: String,
    pub toolkit_slug: String,
    pub toolkit_name: String,
}

/// What [`build_task_overlay`] produces: the raw overlay JSON plus the
/// connected-app metadata. An empty `mcp_overlay` means "no overlay".
#[derive(Debug, Clone, Default)]
pub struct OverlayResult {
    pub mcp_overlay: Vec<u8>,
    pub connected_apps: Vec<ConnectedApp>,
}

/// Inputs to [`build_task_overlay`], decoupled from any concrete service so
/// both the service wiring and tests can drive it.
pub type SessionResult = anyhow::Result<Option<(String, Vec<(String, String)>)>>;

pub trait SessionSpawner: Send + Sync {
    /// Creates a tool-router session; returns Some((mcp_url, auth_headers))
    /// or None when the upstream produced no session.
    fn create_session(
        &self,
        user_id: String,
        toolkits_enable: &[String],
        pinned: &BTreeMap<String, Vec<String>>,
    ) -> impl std::future::Future<Output = SessionResult> + Send;
}

/// Maps the agent.composio_toolkit_allowlist TEXT[] column into a slug set.
/// Each entry is lowercased + trimmed defensively (the API layer already
/// normalises on write, but DB-level migrations / out-of-band writes might
/// bypass that). An empty result triggers the no-overlay bail-out,
/// identically for NULL columns and `{}` arrays.
pub fn normalise_allowlist_to_set(allow: &[String]) -> HashSet<String> {
    allow.iter().filter_map(|s| lower_trim(s)).collect()
}

/// Intersects active connection rows with the allowlist set and returns the
/// `connected_accounts` map shape the Composio session endpoint expects:
/// one entry per allowlisted toolkit slug → array of connected account ids.
///
/// Newest-wins on duplicates: rows arrive ordered by connected_at DESC, so
/// the first row seen for a given slug is the most recently connected
/// account, matching the single-account-per-toolkit invariant.
pub fn pin_connected_accounts(
    rows: &[crate::service::ComposioConnectionRow],
    allow_set: &HashSet<String>,
) -> BTreeMap<String, Vec<String>> {
    let mut pinned = BTreeMap::new();
    for row in rows {
        let Some(slug) = lower_trim(&row.toolkit_slug) else {
            continue;
        };
        if !allow_set.contains(&slug) || pinned.contains_key(&slug) {
            continue;
        }
        pinned.insert(slug, vec![row.connected_account_id.clone()]);
    }
    pinned
}

/// The tiny inlined helper that keeps allowlist and connection slug
/// comparison consistent: ASCII lowercase + trim; None when empty.
pub fn lower_trim(s: &str) -> Option<String> {
    let t = s.trim_matches([' ', '\t', '\n', '\r']);
    if t.is_empty() {
        return None;
    }
    // ASCII fast path matches Go's byte-loop lowering for slugs.
    if t.is_ascii() {
        Some(t.to_ascii_lowercase())
    } else {
        Some(t.to_lowercase())
    }
}

/// Builds the overlay payload for a task dispatching `agent`, or a zero
/// result when ANY gate trips — meaning no Composio session is created and
/// no token is provisioned.
///
/// Invocation and credential authority are deliberately separate. The caller
/// selects `capability_user_id` at an enforcement boundary; this function can
/// only mount that user's active connections. A shared agent therefore never
/// inherits its definition owner's private connection merely by being invoked.
///
/// Gates, in order:
///  1. no capability user — cannot resolve a
///     connected-apps view. No overlay.
///  2. allowlist empty — the agent definition never opted into any toolkit.
///     Default OFF: no overlay.
///  3. After intersecting with the capability user's active connections, no toolkit
///     has an active connection. Nothing to mount.
///  4. Composio returns a session with no URL — defensive.
pub async fn build_task_overlay<S: SessionSpawner>(
    spawner: &S,
    capability_user_id: Option<Uuid>,
    composio_toolkit_allowlist: &[String],
    active_connections: &[crate::service::ComposioConnectionRow],
    display_name_for_slug: impl Fn(&str) -> String,
) -> anyhow::Result<OverlayResult> {
    // Gate 1: the overlay is the capability user's connected-apps view.
    let Some(capability_user_id) = capability_user_id else {
        return Ok(OverlayResult::default());
    };
    // Gate 2: NULL and empty `{}` are treated identically — "no overlay".
    let allow_set = normalise_allowlist_to_set(composio_toolkit_allowlist);
    if allow_set.is_empty() {
        return Ok(OverlayResult::default());
    }

    // The intersection is the canonical input both for filtering the
    // CreateSession call AND for the early bail-out below.
    let pinned = pin_connected_accounts(active_connections, &allow_set);
    if pinned.is_empty() {
        // Gate 3: the capability user has not connected an allowlisted toolkit (or
        // have revoked since).
        return Ok(OverlayResult::default());
    }
    let slugs: Vec<String> = pinned.keys().cloned().collect();

    // `toolkits.enable` narrows what the tool-router exposes; pair it with
    // the connected-account pin so the session can never surface an account
    // outside the (allowlist ∩ active connections) set.
    let Some((url, headers)) = spawner
        .create_session(capability_user_id.to_string(), &slugs, &pinned)
        .await?
    else {
        return Ok(OverlayResult::default());
    };
    // Gate 4: Composio answered 200 with no MCP URL. Treat as "no overlay"
    // rather than wire up a server with an empty URL — every runtime fails
    // noisily on that.
    if url.is_empty() {
        return Ok(OverlayResult::default());
    }

    let mut mcp_servers = BTreeMap::new();
    mcp_servers.insert(
        MCP_OVERLAY_SERVER_NAME.to_string(),
        ComposioMcpServer {
            r#type: "http".to_string(),
            url,
            headers: headers.into_iter().collect(),
        },
    );
    let payload = McpOverlayPayload { mcp_servers };
    let raw = serde_json::to_vec(&payload)?;

    let apps = slugs
        .iter()
        .map(|slug| ConnectedApp {
            provider: "composio".to_string(),
            server_name: MCP_OVERLAY_SERVER_NAME.to_string(),
            toolkit_slug: slug.clone(),
            toolkit_name: display_name_for_slug(slug),
        })
        .collect();

    Ok(OverlayResult {
        mcp_overlay: raw,
        connected_apps: apps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::ComposioConnectionRow;

    fn conn(slug: &str, account: &str) -> ComposioConnectionRow {
        ComposioConnectionRow {
            id: Uuid::now_v7(),
            toolkit_slug: slug.into(),
            status: "active".into(),
            connected_account_id: account.into(),
            connected_at_unix: 0,
        }
    }

    #[test]
    fn allowlist_normalisation_trims_and_lowercases() {
        let set = normalise_allowlist_to_set(&[
            " GitHub ".into(),
            "NOTION".into(),
            "".into(),
            "  ".into(),
        ]);
        assert_eq!(set.len(), 2);
        assert!(set.contains("github") && set.contains("notion"));
        assert!(normalise_allowlist_to_set(&[]).is_empty());
    }

    #[test]
    fn pin_intersects_and_dedupes_newest_first() {
        let rows = vec![
            conn("github", "ca_new"),
            conn("github", "ca_old"),
            conn("notion", "ca_n"),
            conn("slack", "ca_s"), // not allowlisted
        ];
        let allow: HashSet<String> = ["github".to_string(), "notion".to_string()].into();
        let pinned = pin_connected_accounts(&rows, &allow);
        assert_eq!(pinned.len(), 2);
        assert_eq!(pinned["github"], vec!["ca_new"], "newest wins");
        assert_eq!(pinned["notion"], vec!["ca_n"]);
    }

    #[test]
    fn lower_trim_ascii_fast_path() {
        assert_eq!(lower_trim(" MiXeD ").as_deref(), Some("mixed"));
        assert_eq!(lower_trim("   "), None);
        assert_eq!(lower_trim(""), None);
        assert_eq!(lower_trim("\t\nx\r").as_deref(), Some("x"));
    }

    // ── gate tests through a fake spawner ────────────────────────────────

    struct FakeSpawner {
        url: &'static str,
        expected_user: Option<Uuid>,
    }

    impl SessionSpawner for FakeSpawner {
        async fn create_session(
            &self,
            user_id: String,
            toolkits_enable: &[String],
            pinned: &BTreeMap<String, Vec<String>>,
        ) -> anyhow::Result<Option<(String, Vec<(String, String)>)>> {
            if let Some(expected) = self.expected_user {
                assert_eq!(user_id, expected.to_string());
            } else {
                assert!(!user_id.is_empty());
            }
            assert!(!toolkits_enable.is_empty());
            assert!(!pinned.is_empty());
            if self.url.is_empty() {
                return Ok(None);
            }
            Ok(Some((
                self.url.to_string(),
                vec![("x-api-key".into(), "k".into())],
            )))
        }
    }

    const CALLER: uuid::Uuid = uuid::uuid!("0198c0de-0000-7000-8000-000000000002");

    #[tokio::test]
    async fn gates_short_circuit_to_no_overlay() {
        let spawner = FakeSpawner {
            url: "https://mcp",
            expected_user: None,
        };
        let conns = vec![conn("github", "ca_1")];
        // Gate 1: no owner.
        let r = build_task_overlay(&spawner, None, &["github".into()], &conns, |s| s.into())
            .await
            .unwrap();
        assert!(r.mcp_overlay.is_empty());
        // Gate 2: empty allowlist.
        let r = build_task_overlay(&spawner, Some(CALLER), &[], &conns, |s| s.into())
            .await
            .unwrap();
        assert!(r.mcp_overlay.is_empty());
        // Gate 3: allowlist misses all connections.
        let r = build_task_overlay(&spawner, Some(CALLER), &["notion".into()], &conns, |s| {
            s.into()
        })
        .await
        .unwrap();
        assert!(r.mcp_overlay.is_empty());
        // Gate 4: session without URL.
        let empty = FakeSpawner {
            url: "",
            expected_user: None,
        };
        let r = build_task_overlay(&empty, Some(CALLER), &["github".into()], &conns, |s| {
            s.into()
        })
        .await
        .unwrap();
        assert!(r.mcp_overlay.is_empty());
    }

    #[tokio::test]
    async fn happy_path_builds_overlay_with_sorted_apps() {
        let spawner = FakeSpawner {
            url: "https://mcp/sess",
            expected_user: Some(CALLER),
        };
        let conns = vec![
            conn("github", "ca_g"),
            conn("notion", "ca_n"),
            conn("slack", "ca_s"),
        ];
        let r = build_task_overlay(
            &spawner,
            Some(CALLER),
            &["Notion".into(), "github".into()],
            &conns,
            |s| s.to_uppercase(),
        )
        .await
        .unwrap();
        assert!(!r.mcp_overlay.is_empty());
        let parsed: McpOverlayPayload = serde_json::from_slice(&r.mcp_overlay).unwrap();
        let server = parsed.mcp_servers.get(MCP_OVERLAY_SERVER_NAME).unwrap();
        assert_eq!(server.r#type, "http");
        assert_eq!(server.url, "https://mcp/sess");
        assert_eq!(server.headers["x-api-key"], "k");

        assert_eq!(r.connected_apps.len(), 2);
        // BTreeMap ordering → github before notion regardless of input order.
        assert_eq!(r.connected_apps[0].toolkit_slug, "github");
        assert_eq!(r.connected_apps[1].toolkit_name, "NOTION");
        assert_eq!(r.connected_apps[1].provider, "composio");
    }

    #[tokio::test]
    async fn shared_agent_session_is_minted_for_caller_not_definition_owner() {
        const DEFINITION_OWNER: Uuid = uuid::uuid!("0198c0de-0000-7000-8000-000000000001");
        assert_ne!(CALLER, DEFINITION_OWNER);
        let spawner = FakeSpawner {
            url: "https://mcp/sess",
            expected_user: Some(CALLER),
        };
        let result = build_task_overlay(
            &spawner,
            Some(CALLER),
            &["github".into()],
            &[conn("github", "caller-account")],
            str::to_string,
        )
        .await
        .unwrap();
        assert!(!result.mcp_overlay.is_empty());
    }

    #[test]
    fn overlay_payload_serializes_go_field_names() {
        let mut servers = BTreeMap::new();
        servers.insert(
            "composio".to_string(),
            ComposioMcpServer {
                r#type: "http".into(),
                url: "u".into(),
                headers: [("h".to_string(), "v".to_string())].into_iter().collect(),
            },
        );
        let json = serde_json::to_value(&McpOverlayPayload {
            mcp_servers: servers,
        })
        .unwrap();
        assert_eq!(json["mcpServers"]["composio"]["type"], "http");
        assert_eq!(json["mcpServers"]["composio"]["headers"]["h"], "v");
    }
}
