//! On-disk cache of
//! downloaded skill bundles keyed by (workspace, source, id, hash), with a
//! per-key reference lock and atomic directory swap.
//!
//! Symbol map:
//! - `SkillBundleCache` → [`SkillBundleCache`]
//! - `NewSkillBundleCache` → [`SkillBundleCache::new`]
//! - `Load` / `Store` / `WithRefLock` → [`SkillBundleCache::load`] /
//!   [`SkillBundleCache::store`] / [`SkillBundleCache::with_ref_lock`]
//! - `validateSkillBundle` → [`validate_skill_bundle`]
//! - `safeSkillFilePath` → [`safe_skill_file_path`]
//! - `safeCacheSegment` → [`safe_cache_segment`]
//!
//! The manifest hash construction matches `patchbay_service::skill_bundle`
//! without introducing a service-crate dependency: length-prefixed
//! parts (`%d:%s\n`) over the sorted file list. It must stay byte-identical.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::types::{SkillData, SkillRefData};

pub(crate) const SOURCE_PLUGIN: &str = "plugin";

pub(crate) struct SkillBundleCache {
    root: String,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl SkillBundleCache {
    pub(crate) fn new(root: &str) -> Self {
        SkillBundleCache {
            root: root.to_string(),
            locks: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn load(&self, workspace_id: &str, r#ref: &SkillRefData) -> Option<SkillData> {
        if self.root.is_empty() {
            return None;
        }
        let key_path = self.bundle_path(workspace_id, r#ref);
        let data = std::fs::read(&key_path).ok()?;
        let bundle: SkillData = match serde_json::from_slice(&data) {
            Ok(b) => b,
            Err(_) => {
                let _ = std::fs::remove_file(&key_path);
                return None;
            }
        };
        if !validate_skill_bundle(r#ref, &bundle) {
            let _ = std::fs::remove_file(&key_path);
            return None;
        }
        Some(bundle)
    }

    pub(crate) fn store(&self, workspace_id: &str, bundle: &SkillData) -> anyhow::Result<()> {
        if self.root.is_empty() {
            return Ok(());
        }
        let r#ref = SkillRefData {
            id: bundle.id.clone(),
            source: bundle.source.clone(),
            hash: bundle.hash.clone(),
            ..Default::default()
        };
        let dir_parent = parent_dir(&self.bundle_path(workspace_id, &r#ref));
        let tmp_root = parent_dir(&dir_parent);

        // Mirror Go's MkdirTemp(filepath.Dir(dir), ".bundle-*") with a retry
        // that first ensures the grandparent exists.
        let tmp = match make_temp_dir(&tmp_root) {
            Ok(t) => t,
            Err(_) => {
                std::fs::create_dir_all(&tmp_root)?;
                make_temp_dir(&tmp_root)?
            }
        };
        let tmp_str = tmp.to_string_lossy().into_owned();
        let result = (|| -> anyhow::Result<()> {
            let data = serde_json::to_vec(bundle)?;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(join(&[&tmp_str, "bundle.json"]))?;
            f.write_all(&data)?;
            f.sync_all()?;
            // Windows refuses to rename a directory while a child file still
            // has an open handle. Close the bundle before publishing the
            // temporary directory atomically.
            drop(f);

            let remove_all = |p: &str| -> std::io::Result<()> {
                if PathBuf::from(p).is_file() {
                    std::fs::remove_file(p)
                } else {
                    std::fs::remove_dir_all(p).or_else(|e| {
                        if e.kind() == std::io::ErrorKind::NotFound {
                            Ok(())
                        } else {
                            Err(e)
                        }
                    })
                }
            };
            let rename =
                |old: &str, new: &str| -> std::io::Result<()> { std::fs::rename(old, new) };

            remove_all(&dir_parent)?;
            if let Err(err) = rename(&tmp_str, &dir_parent) {
                // Go retries only on fs.ErrExist; other errors propagate.
                if !is_exists_error(&err) {
                    return Err(anyhow::anyhow!(
                        "skill cache rename {tmp_str} -> {dir_parent}: {err}"
                    ));
                }
                remove_all(&dir_parent)?;
                rename(&tmp_str, &dir_parent).map_err(|err| {
                    anyhow::anyhow!("skill cache rename {tmp_str} -> {dir_parent}: {err}")
                })?;
            }
            Ok(())
        })();
        // Go defers os.RemoveAll(tmp) regardless of outcome.
        let _ = std::fs::remove_dir_all(&tmp);
        result
    }

    /// `WithRefLock`: serialises cache work per (workspace, source, id, hash).
    pub(crate) fn with_ref_lock<T>(
        &self,
        workspace_id: &str,
        r#ref: &SkillRefData,
        f: impl FnOnce() -> T,
    ) -> T {
        let key = format!(
            "{}\x00{}\x00{}\x00{}",
            workspace_id, r#ref.source, r#ref.id, r#ref.hash
        );
        let lock = self.lock_for_key(key);
        let _guard = lock.lock().expect("ref lock");
        f()
    }

    fn lock_for_key(&self, key: String) -> std::sync::Arc<Mutex<()>> {
        let mut locks = self.locks.lock().expect("locks map");
        locks.entry(key).or_default().clone()
    }

    fn bundle_path(&self, workspace_id: &str, r#ref: &SkillRefData) -> String {
        self.bundle_path_public(workspace_id, r#ref)
    }

    pub(crate) fn bundle_path_public(&self, workspace_id: &str, r#ref: &SkillRefData) -> String {
        join(&[
            &self.root,
            &safe_cache_segment(workspace_id),
            &safe_cache_segment(&r#ref.source),
            &safe_cache_segment(&r#ref.id),
            &safe_cache_segment(&r#ref.hash),
            "bundle.json",
        ])
    }
}

fn is_exists_error(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::AlreadyExists
        || matches!(err.raw_os_error(), Some(libc::EEXIST))
}

fn join(parts: &[&str]) -> String {
    crate::execenv::execenv::join_path(parts)
}

fn parent_dir(p: &str) -> String {
    match p.rfind('/') {
        Some(0) => "/".to_string(),
        Some(idx) => p[..idx].to_string(),
        None => ".".to_string(),
    }
}

fn make_temp_dir(base: &str) -> std::io::Result<PathBuf> {
    for _ in 0..10000 {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let candidate = join(&[base, &format!(".bundle-{}", 1_000_000 + n as u64)]);
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(PathBuf::from(candidate)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "temp dir name exhausted",
    ))
}

/// `validateSkillBundle`: identity fields must match the ref, the file count
/// must agree, every file path must be safe, and the recomputed manifest hash
/// (and optionally size) must equal the ref's.
pub(crate) fn validate_skill_bundle(r#ref: &SkillRefData, bundle: &SkillData) -> bool {
    if bundle.id != r#ref.id || bundle.source != r#ref.source || bundle.hash != r#ref.hash {
        return false;
    }
    if bundle.files.len() as i64 != r#ref.file_count {
        return false;
    }
    let mut files = Vec::with_capacity(bundle.files.len());
    for file in &bundle.files {
        if !safe_skill_file_path(&file.path) {
            return false;
        }
        files.push(SkillBundleFile {
            path: file.path.clone(),
            content: file.content.clone(),
        });
    }
    let manifest = build_manifest(&SkillBundleSkill {
        id: bundle.id.clone(),
        source: bundle.source.clone(),
        name: bundle.name.clone(),
        description: bundle.description.clone(),
        content: bundle.content.clone(),
        files,
    });
    if manifest.hash != r#ref.hash {
        return false;
    }
    if r#ref.size_bytes > 0 && manifest.size_bytes != r#ref.size_bytes {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// Skill-bundle manifest hashing.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub(crate) struct SkillBundleFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SkillBundleSkill {
    pub id: String,
    pub source: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub files: Vec<SkillBundleFile>,
}

fn write_hash_part(h: &mut Sha256, s: &str) {
    let _ = writeln!(h, "{}:{}", s.len(), s);
}

pub(crate) fn build_manifest(skill: &SkillBundleSkill) -> SkillManifestSize {
    let mut files = skill.files.clone();
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
        let digest = format!(
            "sha256:{}",
            hex::encode(Sha256::digest(file.content.as_bytes()))
        );
        write_hash_part(&mut h, &file.path);
        write_hash_part(&mut h, &digest);
        write_hash_part(&mut h, &file.content);
        size += file.content.len() as i64;
        refs.push(SkillFileRefLite {
            path: file.path.clone(),
            sha256: digest,
            size_bytes: file.content.len() as i64,
        });
    }
    SkillManifestSize {
        hash: format!("sha256:{}", hex::encode(h.finalize())),
        size_bytes: size,
        files: refs,
    }
}

pub(crate) struct SkillManifestSize {
    pub hash: String,
    pub size_bytes: i64,
    pub files: Vec<SkillFileRefLite>,
}

#[derive(Debug, Clone)]
pub(crate) struct SkillFileRefLite {
    pub path: String,
    pub sha256: String,
    pub size_bytes: i64,
}

/// `safeSkillFilePath`.
pub(crate) fn safe_skill_file_path(p: &str) -> bool {
    if p.is_empty() || p.contains('\0') || p.starts_with('/') || p.contains('\\') {
        return false;
    }
    let clean = crate::execenv::execenv::clean_path(p);
    if clean == "." || clean != p || clean.starts_with("../") || clean == ".." {
        return false;
    }
    true
}

/// `safeCacheSegment`.
pub(crate) fn safe_cache_segment(s: &str) -> String {
    if s.is_empty() {
        return "_".to_string();
    }
    let mut out = String::with_capacity(s.len());
    for r in s.chars() {
        match r {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => out.push(r),
            _ => out.push('_'),
        }
    }
    if out == "." || out == ".." {
        return format!("_{out}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SkillFileData;

    fn test_skill_bundle() -> (SkillData, SkillRefData) {
        let mut bundle = SkillData {
            id: "skill-1".into(),
            source: "workspace".into(),
            name: "deploy".into(),
            content: "main".into(),
            files: vec![SkillFileData {
                path: "rules.md".into(),
                content: "rules".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        // Derive ref exactly like skillRefFromBundle.
        let files: Vec<SkillBundleFile> = bundle
            .files
            .iter()
            .map(|f| SkillBundleFile {
                path: f.path.clone(),
                content: f.content.clone(),
            })
            .collect();
        let m = build_manifest(&SkillBundleSkill {
            id: bundle.id.clone(),
            source: bundle.source.clone(),
            name: bundle.name.clone(),
            description: bundle.description.clone(),
            content: bundle.content.clone(),
            files,
        });
        let r#ref = SkillRefData {
            id: bundle.id.clone(),
            source: bundle.source.clone(),
            name: String::new(),
            description: String::new(),
            hash: m.hash.clone(),
            size_bytes: m.size_bytes,
            file_count: m.files.len() as i64,
            files: m
                .files
                .iter()
                .map(|f| crate::types::SkillFileRefData {
                    path: f.path.clone(),
                    sha256: f.sha256.clone(),
                    size_bytes: f.size_bytes,
                })
                .collect(),
        };
        bundle.hash = r#ref.hash.clone();
        bundle.size_bytes = r#ref.size_bytes;
        bundle.files[0].sha256 = r#ref.files[0].sha256.clone();
        bundle.files[0].size_bytes = r#ref.files[0].size_bytes;
        (bundle, r#ref)
    }

    fn temp_root(tag: &str) -> String {
        let base = std::env::temp_dir().join(format!(
            "skill-cache-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base.to_string_lossy().into_owned()
    }

    #[test]
    fn load_store_roundtrip() {
        let cache = SkillBundleCache::new(&temp_root("rt"));
        let (bundle, r#ref) = test_skill_bundle();
        cache.store("ws-1", &bundle).unwrap();
        let got = cache.load("ws-1", &r#ref).expect("cache hit");
        assert_eq!(got.content, bundle.content);
        assert_eq!(got.files.len(), 1);
        assert_eq!(got.files[0].content, "rules");
    }

    #[test]
    fn rejects_corrupt_bundle_and_removes_entry() {
        let root = temp_root("corrupt");
        let cache = SkillBundleCache::new(&root);
        let (bundle, r#ref) = test_skill_bundle();
        cache.store("ws-1", &bundle).unwrap();

        let path = cache.bundle_path_public("ws-1", &r#ref);
        std::fs::write(
            &path,
            br#"{"id":"skill-1","source":"workspace","hash":"sha256:bad","content":"tampered"}"#,
        )
        .unwrap();
        assert!(cache.load("ws-1", &r#ref).is_none());
        assert!(!PathBuf::from(&path).exists());
    }

    #[test]
    fn empty_root_disables_cache() {
        let cache = SkillBundleCache::new("");
        let (bundle, r#ref) = test_skill_bundle();
        cache.store("ws", &bundle).unwrap();
        assert!(cache.load("ws", &r#ref).is_none());
    }

    #[test]
    fn safe_cache_segment_rules() {
        assert_eq!(safe_cache_segment(""), "_");
        assert_eq!(safe_cache_segment("."), "_.");
        assert_eq!(safe_cache_segment(".."), "_..");
        assert_eq!(safe_cache_segment("a/b c"), "a_b_c");
        assert_eq!(safe_cache_segment("ok-name_1.x"), "ok-name_1.x");
    }

    #[test]
    fn safe_skill_file_path_rules() {
        assert!(!safe_skill_file_path(""));
        assert!(!safe_skill_file_path("/abs"));
        assert!(!safe_skill_file_path("a\\b"));
        assert!(!safe_skill_file_path("../up"));
        assert!(!safe_skill_file_path("./x"));
        assert!(safe_skill_file_path("sub/dir/file.md"));
    }
}
