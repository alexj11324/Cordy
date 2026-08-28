//! Built-in system agents.

/// Marks the workspace's built-in Chief of Staff agent. It is the agent's
/// identity for every server-side decision — never its display name, which
/// owners are free to change.
///
/// The row stays kind='user': kind='system' means "invisible execution
/// carrier" in this schema (hidden from agent lists and assignment surfaces,
/// and hard deleted when its runtime goes away), and Mika needs the opposite
/// of all three.
pub const MIKA_SYSTEM_KEY: &str = "mika";

/// The name the agent is created with. Owners may rename it; nothing
/// server-side keys off the name, and the prompt is templated on whatever the
/// current name is.
pub const MIKA_DEFAULT_NAME: &str = "Mika";

/// Substituted in the embedded prompt with the agent's current display name.
const MIKA_NAME_PLACEHOLDER: &str = "{{AGENT_NAME}}";

/// The system half of the prompt. `{{AGENT_NAME}}` is substituted with the
/// agent's current display name — a placeholder rather than a format verb so
/// a stray % in the prompt can never turn into a formatting error.
///
/// Single source of truth: the file is packaged with this crate and embedded
/// in the service binary at compile time.
const MIKA_INSTRUCTIONS_MD: &str = include_str!("../assets/builtin_agents/mika/INSTRUCTIONS.md");

/// Introduces the workspace's own additions and states how they rank against
/// the system half.
///
/// It lives here rather than at the end of the embedded file because a
/// workspace with no notes — every workspace, at first — would otherwise end
/// its prompt announcing a section that has nothing under it. Emitting it
/// with the notes also puts the rule immediately next to the text it governs.
pub const MIKA_WORKSPACE_NOTES_SECTION: &str = r#"## Workspace notes

Workspace notes below add this team's context and preferences — repositories, languages, conventions, routing defaults. Follow them ahead of your own defaults; they refine how you apply these instructions, and they do not remove the identity or confirmation duties above.

Added by this workspace's admins:"#;

/// Returns the product-owned half of Mika's prompt for an agent displayed
/// under the given name.
///
/// This is the whole point of the system-agent model: the text ships with the
/// server binary rather than being copied into agent.instructions at creation,
/// so editing the embedded file and deploying updates every existing workspace
/// on its next task. Nothing is written to any agent row, so a workspace's own
/// notes can never be overwritten by a release.
pub fn mika_system_instructions(display_name: &str) -> String {
    let name = display_name.trim();
    let name = if name.is_empty() {
        MIKA_DEFAULT_NAME
    } else {
        name
    };
    mika_instructions_md()
        .trim_end_matches('\n')
        .replace(MIKA_NAME_PLACEHOLDER, name)
}

/// Layers the workspace's notes under the product-owned system instructions.
/// `workspace_notes` is agent.instructions — the only half a workspace can
/// write.
pub fn compose_mika_instructions(display_name: &str, workspace_notes: &str) -> String {
    let system = mika_system_instructions(display_name);
    let notes = workspace_notes.trim();
    if notes.is_empty() {
        return system;
    }
    format!(
        "{}\n\n{}\n\n{}",
        system, MIKA_WORKSPACE_NOTES_SECTION, notes
    )
}

fn mika_instructions_md() -> &'static str {
    MIKA_INSTRUCTIONS_MD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_is_substituted_with_display_name() {
        let out = mika_system_instructions("Atlas");
        assert!(out.contains("You are Atlas,"), "prompt must greet as Atlas");
        assert!(!out.contains("{{AGENT_NAME}}"), "placeholder must be gone");
    }

    #[test]
    fn empty_display_name_falls_back_to_mika() {
        let out = mika_system_instructions("   ");
        assert!(out.contains("You are Mika,"));
    }

    #[test]
    fn embedded_prompt_is_nonempty_and_carries_identity_duties() {
        let md = mika_instructions_md();
        assert!(md.len() > 1_000, "embedded file should be substantial");
        assert!(md.contains("Chief of Staff"));
    }

    #[test]
    fn compose_without_notes_returns_system_only() {
        let system = mika_system_instructions("Mika");
        let composed = compose_mika_instructions("Mika", "");
        assert_eq!(composed, system);
        // Trailing whitespace-only notes count as empty too.
        assert_eq!(compose_mika_instructions("Mika", "   \n\t "), system);
    }

    #[test]
    fn compose_layers_notes_under_workspace_section() {
        let composed = compose_mika_instructions("Mika", "Always reply in French.");
        assert!(composed.contains(MIKA_WORKSPACE_NOTES_SECTION));
        assert!(composed.contains("Always reply in French."));
        // System half comes first.
        let notes_pos = composed.find(MIKA_WORKSPACE_NOTES_SECTION).unwrap();
        assert!(composed.starts_with(&composed[..notes_pos].to_string()));
    }
}
