//! Skill manifest hashing.
//!
//! The daemon verifies skill bundles against this manifest, so the hash
//! construction must stay byte-identical to the Go implementation: length-
//! prefixed parts (`%d:%s\n`) over the sorted file list.

use sha2::{Digest, Sha256};

pub const SOURCE_WORKSPACE: &str = "workspace";
pub const SOURCE_BUILTIN: &str = "builtin";
pub const SOURCE_PLUGIN: &str = "plugin";

#[derive(Debug, Clone, Default)]
pub struct File {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct Skill {
    pub id: String,
    pub source: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub files: Vec<File>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FileRef {
    pub path: String,
    pub sha256: String,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Manifest {
    pub hash: String,
    pub size_bytes: i64,
    pub file_count: usize,
    pub files: Vec<FileRef>,
}

/// Builds the verifiable manifest for a skill. Files are sorted by path so
/// the hash is independent of the caller's enumeration order.
pub fn build_manifest(skill: Skill) -> Manifest {
    let mut files = skill.files;
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let mut h = Sha256::new();
    write_hash_part(&mut h, "v1");
    write_hash_part(&mut h, &skill.source);
    write_hash_part(&mut h, &skill.id);
    write_hash_part(&mut h, &skill.name);
    write_hash_part(&mut h, &skill.description);
    write_hash_part(&mut h, &skill.content);

    let mut size = skill.content.len() as i64;
    let mut refs = Vec::with_capacity(files.len());
    for file in &files {
        let file_digest = format!(
            "sha256:{}",
            hex::encode(Sha256::digest(file.content.as_bytes()))
        );
        write_hash_part(&mut h, &file.path);
        write_hash_part(&mut h, &file_digest);
        write_hash_part(&mut h, &file.content);
        size += file.content.len() as i64;
        refs.push(FileRef {
            path: file.path.clone(),
            sha256: file_digest,
            size_bytes: file.content.len() as i64,
        });
    }

    Manifest {
        hash: format!("sha256:{}", hex::encode(h.finalize())),
        size_bytes: size,
        file_count: refs.len(),
        files: refs,
    }
}

fn write_hash_part(h: &mut Sha256, value: &str) {
    use std::io::Write as _;
    let _ = writeln!(h, "{}:{}", value.len(), value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_order_independent() {
        let a = build_manifest(Skill {
            id: "s1".into(),
            source: SOURCE_WORKSPACE.into(),
            name: "n".into(),
            description: String::new(),
            content: "body".into(),
            files: vec![
                File {
                    path: "b.md".into(),
                    content: "bb".into(),
                },
                File {
                    path: "a.md".into(),
                    content: "aa".into(),
                },
            ],
        });
        let b = build_manifest(Skill {
            id: "s1".into(),
            source: SOURCE_WORKSPACE.into(),
            name: "n".into(),
            description: String::new(),
            content: "body".into(),
            files: vec![
                File {
                    path: "a.md".into(),
                    content: "aa".into(),
                },
                File {
                    path: "b.md".into(),
                    content: "bb".into(),
                },
            ],
        });
        assert_eq!(a.hash, b.hash);
        assert_eq!(
            a.files.iter().map(|f| &f.path).collect::<Vec<_>>(),
            ["a.md", "b.md"]
        );
        assert_eq!(a.size_bytes, 4 + 2 + 2);
        assert_eq!(a.file_count, 2);
    }

    #[test]
    fn hash_changes_when_any_part_changes() {
        let base = Skill {
            id: "s1".into(),
            source: SOURCE_BUILTIN.into(),
            name: "n".into(),
            description: "d".into(),
            content: "c".into(),
            files: vec![],
        };
        let m0 = build_manifest(base.clone());
        let mut m1_skill = base.clone();
        m1_skill.description = "d2".into();
        let m1 = build_manifest(m1_skill);
        let mut m2_skill = base;
        m2_skill.files.push(File {
            path: "x".into(),
            content: "y".into(),
        });
        let m2 = build_manifest(m2_skill);
        assert_ne!(m0.hash, m1.hash);
        assert_ne!(m0.hash, m2.hash);
    }

    #[test]
    fn length_prefix_shape_matches_go_fmt() {
        // writeHashPart emits "%d:%s\n" — verify via a known-vector style
        // check on the internal writer.
        let mut h = Sha256::new();
        write_hash_part(&mut h, "ab");
        let digest = hex::encode(h.finalize());
        let mut expected = Sha256::new();
        std::io::Write::write_all(&mut expected, b"2:ab\n").unwrap();
        assert_eq!(digest, hex::encode(expected.finalize()));
    }
}
