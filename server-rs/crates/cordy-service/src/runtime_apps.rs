//! Runtime app metadata — port of
//! `server/internal/runtimeapps/connected_app.go`.

use serde::Serialize;

/// Non-secret task-scoped metadata that tells the daemon which external app
/// capabilities were actually mounted for a run.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectedApp {
    pub provider: String,
    pub server_name: String,
    pub toolkit_slug: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub toolkit_name: String,
}

/// Carries both the secret-bearing MCP config overlay and the non-secret app
/// list used to brief the agent.
#[derive(Debug, Clone, Default)]
pub struct McpOverlayResult {
    /// Raw JSON overlay (Go json.RawMessage); kept opaque here.
    pub mcp_overlay: Option<serde_json::Value>,
    pub connected_apps: Vec<ConnectedApp>,
}

/// Returns a compact human-readable label without making a catalog call in
/// the enqueue path. Brand casing here is intentionally best-effort; the slug
/// remains the functional identifier in every brief.
pub fn display_name_for_toolkit_slug(slug: &str) -> String {
    let slug = slug.trim();
    if slug.is_empty() {
        return String::new();
    }
    match slug {
        "github" => return "GitHub".to_string(),
        "gmail" => return "Gmail".to_string(),
        "linkedin" => return "LinkedIn".to_string(),
        _ => {}
    }
    let words: Vec<String> = slug
        .split(['_', '-'])
        .filter(|w| !w.is_empty())
        .map(title_ascii)
        .collect();
    if words.is_empty() {
        return slug.to_string();
    }
    words.join(" ")
}

fn title_ascii(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut b = lower.into_bytes();
    if let Some(first) = b.first_mut() {
        if (*first >= b'a') && (*first <= b'z') {
            *first -= b'a' - b'A';
        }
    }
    String::from_utf8(b).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_brands_keep_casing() {
        assert_eq!(display_name_for_toolkit_slug("github"), "GitHub");
        assert_eq!(display_name_for_toolkit_slug("gmail"), "Gmail");
        assert_eq!(display_name_for_toolkit_slug("linkedin"), "LinkedIn");
    }

    #[test]
    fn unknown_slugs_title_case_on_separators() {
        assert_eq!(display_name_for_toolkit_slug("notion_docs"), "Notion Docs");
        assert_eq!(display_name_for_toolkit_slug("slack-emoji"), "Slack Emoji");
        // Go's strings.FieldsFunc drops separator runs entirely.
        assert_eq!(display_name_for_toolkit_slug("--a__b--"), "A B");
    }

    #[test]
    fn empty_and_blank_pass_through() {
        assert_eq!(display_name_for_toolkit_slug(""), "");
        assert_eq!(display_name_for_toolkit_slug("   "), "");
    }

    #[test]
    fn connected_app_json_field_names_match_go_tags() {
        let app = ConnectedApp {
            provider: "composio".into(),
            server_name: "gh".into(),
            toolkit_slug: "github".into(),
            toolkit_name: String::new(),
        };
        let v = serde_json::to_value(&app).unwrap();
        assert_eq!(v["provider"], "composio");
        assert_eq!(v["server_name"], "gh");
        assert_eq!(v["toolkit_slug"], "github");
        // omitempty on toolkit_name.
        assert!(v.get("toolkit_name").is_none());
    }
}
