//! Platform built-in skills.
//!
//! Skills are embedded at compile time from this crate's asset directory.
//! Every agent receives these on top of its workspace-bound skills, so they
//! teach platform-wide "how to" workflows (e.g. mentioning) that the runtime
//! brief intentionally leaves to skills.
//!
//! Layout: `builtin_skills/<name>/SKILL.md` plus optional supporting files.
//! The `<name>` directory carries a `cordy-` prefix so its on-disk slug can
//! never collide with a workspace skill a user authored.

use include_dir::{include_dir, Dir};

/// Compile-time embed of the built-in skill tree.
static BUILTIN_SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/builtin_skills");

/// A skill for task execution responses. JSON field names are part of the
/// task-claim API contract.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentSkillData {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hash: String,
    #[serde(skip_serializing_if = "num_is_zero")]
    pub size_bytes: i64,
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<AgentSkillFileData>,
}

/// A supporting file within a skill.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentSkillFileData {
    pub path: String,
    pub content: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sha256: String,
    #[serde(skip_serializing_if = "num_is_zero")]
    pub size_bytes: i64,
}

fn num_is_zero(v: &i64) -> bool {
    *v == 0
}

/// Returns the platform's built-in skills in deterministic (sorted) order.
pub fn load_builtin_skills() -> Vec<AgentSkillData> {
    let mut names: Vec<&str> = BUILTIN_SKILLS
        .dirs()
        .map(|d| d.path().to_str().unwrap_or_default())
        .collect();
    names.sort();
    let mut skills = Vec::with_capacity(names.len());
    for name in names {
        if let Some(skill) = load_builtin_skill(name) {
            skills.push(skill);
        }
    }
    skills
}

/// Loads one skill directory. A directory without a SKILL.md is malformed —
/// skip it rather than ship an empty skill.
fn load_builtin_skill(name: &str) -> Option<AgentSkillData> {
    let dir = BUILTIN_SKILLS.get_dir(name)?;
    // include_dir resolves paths against the embed ROOT, not the subdir —
    // `dir.get_file("SKILL.md")` would look for "<name>/SKILL.md" under the
    // subdir and miss. Query the root with the full relative path.
    let skill_md = BUILTIN_SKILLS.get_file(format!("{name}/SKILL.md"))?;
    let content = std::str::from_utf8(skill_md.contents()).ok()?.to_string();

    let mut skill = AgentSkillData {
        id: String::new(),
        source: String::new(),
        name: name.to_string(),
        description: String::new(),
        hash: String::new(),
        size_bytes: 0,
        content,
        files: Vec::new(),
    };

    // Any other file in the directory becomes a supporting file, preserving
    // its relative path so subdirectories (e.g. rules/styling.md) survive.
    fn walk(dir: &Dir<'_>, prefix: &str, files: &mut Vec<AgentSkillFileData>) {
        for f in dir.files() {
            let rel = f
                .path()
                .to_str()
                .unwrap_or_default()
                .strip_prefix(prefix)
                .unwrap_or_default();
            if rel == "SKILL.md" {
                continue;
            }
            let Ok(content) = std::str::from_utf8(f.contents()) else {
                continue;
            };
            files.push(AgentSkillFileData {
                path: rel.to_string(),
                content: content.to_string(),
                sha256: String::new(),
                size_bytes: 0,
            });
        }
        for sub in dir.dirs() {
            walk(sub, prefix, files);
        }
    }
    walk(dir, &format!("{name}/"), &mut skill.files);
    Some(skill)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_nine_platform_skills_embed() {
        let skills = load_builtin_skills();
        assert_eq!(skills.len(), 9, "expected the 9 shipped platform skills");
        // Deterministic sorted order.
        assert_eq!(skills[0].name, "cordy-autopilots");
        assert_eq!(skills[8].name, "cordy-working-on-issues");
    }

    #[test]
    fn every_skill_has_nonempty_content_and_cordy_prefix() {
        for s in load_builtin_skills() {
            assert!(
                s.name.starts_with("cordy-"),
                "slug must keep the collision-proof prefix"
            );
            assert!(!s.content.is_empty(), "{} has empty SKILL.md", s.name);
        }
    }

    #[test]
    fn supporting_files_keep_relative_paths() {
        let skills = load_builtin_skills();
        let autopilots = skills
            .iter()
            .find(|s| s.name == "cordy-autopilots")
            .unwrap();
        let refs: Vec<_> = autopilots
            .files
            .iter()
            .filter(|f| f.path.starts_with("references/"))
            .collect();
        assert!(
            refs.iter()
                .any(|f| f.path == "references/autopilots-source-map.md"),
            "subdirectory paths must survive: {:?}",
            autopilots.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
        assert!(refs.iter().all(|f| !f.content.is_empty()));
        // SKILL.md itself is not duplicated into files.
        assert!(autopilots.files.iter().all(|f| f.path != "SKILL.md"));
    }

    #[test]
    fn json_field_names_match_api_contract() {
        let s = load_builtin_skills().remove(0);
        let v = serde_json::to_value(&s).unwrap();
        for key in ["id", "name", "content"] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        // omitempty fields absent when zero-valued.
        assert!(v.get("source").is_none());
        assert!(v.get("description").is_none());
        assert!(v.get("hash").is_none());
        assert!(v.get("size_bytes").is_none());
        if s.files.is_empty() {
            assert!(v.get("files").is_none());
        }
    }
}
