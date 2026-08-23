//! Port of `server/internal/daemon/skill_cache.go` (192 lines).
//!
//! Deviations from Go:
//! - `sync.Mutex` per-key lock map → `Mutex<HashMap<String, Arc<Mutex<()>>>>`.
//! - `os.Rename`/`os.RemoveAll` injection fields → boxed closures behind a
//!   mutex, preserving the test seam (`cache.rename = ...`).
//! - `skillbundle.BuildManifest` → local seam stand-in [`skillbundle`]
//!   (byte-identical port of `server/pkg/skillbundle/hash.go`; the identical
//!   implementation already exists in `cordy-service::skill_bundle`, but this
//!   crate does not depend on cordy-service and Cargo.toml is out of scope).
//! - `skillRefFromBundle` (daemon.go:6116–6140) is hosted here because the
//!   cache validation and tests need it; the daemon-core lane can re-use it.
//! - Blocking fs calls are used directly, matching Go's synchronous os API.

// S9-integration: consumed by local_skills/manager wiring that lands with
// integration; silence dead-code until then.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use sha2::Digest as _;

use crate::types::{SkillData, SkillFileData, SkillFileRefData, SkillRefData};

// ---------------------------------------------------------------------------
// skillbundle seam stand-in (server/pkg/skillbundle/hash.go).
// ---------------------------------------------------------------------------

/// S9-integration: mirrors `skillbundle.File` / `Skill` / `Manifest` /
/// `BuildManifest` from server/pkg/skillbundle/hash.go:10–83. Byte-identical
/// to cordy-service::skill_bundle; keep both in sync with the Go hash.
pub(crate) mod skillbundle {
    use sha2::{Digest, Sha256};

    #[derive(Debug, Clone)]
    pub(crate) struct File {
        pub path: String,
        pub content: String,
    }

    pub(crate) struct Skill {
        pub id: String,
        pub source: String,
        pub name: String,
        pub description: String,
        pub content: String,
        pub files: Vec<File>,
    }

    pub(crate) struct Manifest {
        pub hash: String,
        pub size_bytes: i64,
        #[allow(dead_code)]
        pub file_count: usize,
    }

    /// `BuildManifest` (hash.go:43–79): length-prefixed parts over the sorted
    /// file list.
    pub(crate) fn build_manifest(skill: Skill) -> Manifest {
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
        for file in &files {
            let file_digest = format!("sha256:{}", hex::encode(Sha256::digest(file.content.as_bytes())));
            write_hash_part(&mut h, &file.path);
            write_hash_part(&mut h, &file_digest);
            write_hash_part(&mut h, &file.content);
            size += file.content.len() as i64;
        }

        Manifest {
            hash: format!("sha256:{}", hex::encode(h.finalize())),
            size_bytes: size,
            file_count: files.len(),
        }
    }

    /// `writeHashPart` (hash.go:81–83): `%d:%s\n`.
    fn write_hash_part(h: &mut Sha256, value: &str) {
        use std::io::Write as _;
        let _ = writeln!(h, "{}:{}", value.len(), value);
    }
}

/// `skillRefFromBundle` (daemon.go:6116–6140): derives the wire ref (hash,
/// size, per-file digests) from a bundle via the manifest builder.
pub(crate) fn skill_ref_from_bundle(bundle: &SkillData) -> SkillRefData {
    let files: Vec<skillbundle::File> = bundle
        .files
        .iter()
        .map(|file| skillbundle::File {
            path: file.path.clone(),
            content: file.content.clone(),
        })
        .collect();
    let manifest = skillbundle::build_manifest(skillbundle::Skill {
        id: bundle.id.clone(),
        source: bundle.source.clone(),
        name: bundle.name.clone(),
        description: bundle.description.clone(),
        content: bundle.content.clone(),
        files,
    });
    // manifest.files is not retained by the seam stand-in; recompute refs in
    // the same sorted order as build_manifest.
    let mut sorted: Vec<&SkillFileData> = bundle.files.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let file_refs: Vec<SkillFileRefData> = sorted
        .iter()
        .map(|file| SkillFileRefData {
            path: file.path.clone(),
            sha256: format!("sha256:{}", hex::encode(sha2::Sha256::digest(file.content.as_bytes()))),
            size_bytes: file.content.len() as i64,
        })
        .collect();
    SkillRefData {
        id: bundle.id.clone(),
        source: bundle.source.clone(),
        name: String::new(),
        description: String::new(),
        hash: manifest.hash,
        size_bytes: manifest.size_bytes,
        file_count: manifest.file_count as i64,
        files: file_refs,
    }
}

// ---------------------------------------------------------------------------
// SkillBundleCache.
// ---------------------------------------------------------------------------

type FsOp = Box<dyn Fn(&Path, &Path) -> io::Result<()> + Send + Sync>;

/// `SkillBundleCache` (skill_cache.go:17–23): on-disk cache of validated
/// skill bundles keyed by workspace/source/id/hash.
pub(crate) struct SkillBundleCache {
    root: PathBuf,
    rename: Mutex<FsOp>,
    remove_all: Mutex<FsOp>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl SkillBundleCache {
    /// `NewSkillBundleCache` (skill_cache.go:25–32).
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            rename: Mutex::new(Box::new(|from, to| std::fs::rename(from, to))),
            remove_all: Mutex::new(Box::new(|path, _| {
                match std::fs::remove_dir_all(path) {
                    Ok(()) => Ok(()),
                    // os.RemoveAll returns nil for missing paths.
                    Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(err) => Err(err),
                }
            })),
            locks: Mutex::new(HashMap::new()),
        }
    }

    /// Test seam mirroring `cache.rename = ...` (skill_cache_test.go:32–41).
    #[cfg(test)]
    fn set_rename(&self, f: impl Fn(&Path, &Path) -> io::Result<()> + Send + Sync + 'static) {
        *self.rename.lock().unwrap() = Box::new(f);
    }

    /// `Load` (skill_cache.go:34–49): reads and validates the cached bundle;
    /// a corrupt entry is removed and reported as a miss. An empty root (or
    /// nil receiver in Go) disables the cache.
    pub(crate) fn load(&self, workspace_id: &str, r#ref: &SkillRefData) -> Option<SkillData> {
        if self.root.as_os_str().is_empty() {
            return None;
        }
        let key_path = self.bundle_path(workspace_id, r#ref);
        let data = match std::fs::read(&key_path) {
            Ok(data) => data,
            Err(_) => return None,
        };
        let parsed: Result<SkillData, _> = serde_json::from_slice(&data);
        let bundle = match parsed {
            Ok(bundle) => bundle,
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

    /// `Store` (skill_cache.go:51–91): atomically swaps the bundle directory
    /// via a temp dir + remove + rename, tolerating an EEXIST race on rename.
    pub(crate) fn store(&self, workspace_id: &str, bundle: &SkillData) -> anyhow::Result<()> {
        if self.root.as_os_str().is_empty() {
            return Ok(());
        }
        let r#ref = SkillRefData {
            id: bundle.id.clone(),
            source: bundle.source.clone(),
            hash: bundle.hash.clone(),
            ..Default::default()
        };
        // Go: dir = filepath.Dir(bundlePath) — the hash directory; tmp dirs
        // are created one level above it.
        let bundle_file = self.bundle_path(workspace_id, &r#ref);
        let dir = bundle_file
            .parent()
            .with_context(|| format!("bundle path has no parent: {}", bundle_file.display()))?
            .to_path_buf();
        let tmp_parent = dir
            .parent()
            .with_context(|| format!("bundle dir has no parent: {}", dir.display()))?
            .to_path_buf();

        let tmp = match tempfile_builder(&tmp_parent) {
            Ok(tmp) => tmp,
            Err(_) => {
                std::fs::create_dir_all(&tmp_parent)?;
                tempfile_builder(&tmp_parent)?
            }
        };

        let result = (|| -> anyhow::Result<()> {
            let data = serde_json::to_vec(bundle).context("marshal skill bundle")?;
            std::fs::write(tmp.join("bundle.json"), data).context("write cached bundle")?;
            (self.remove_all.lock().unwrap())(&dir, &dir).context("remove stale bundle dir")?;
            if let Err(err) = (self.rename.lock().unwrap())(&tmp, &dir) {
                if err.kind() != io::ErrorKind::AlreadyExists {
                    return Err(err).context("rename bundle into place");
                }
                (self.remove_all.lock().unwrap())(&dir, &dir)
                    .context("remove existing bundle dir")?;
                (self.rename.lock().unwrap())(&tmp, &dir)
                    .context("rename bundle after EEXIST recovery")?;
            }
            Ok(())
        })();

        // defer os.RemoveAll(tmp) (skill_cache.go:67); a no-op after a
        // successful rename.
        let _ = std::fs::remove_dir_all(&tmp);
        result
    }

    /// `WithRefLock` (skill_cache.go:93–102): serializes work per
    /// workspace+source+id+hash key.
    pub(crate) fn with_ref_lock(
        &self,
        workspace_id: &str,
        r#ref: &SkillRefData,
        f: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let key = format!(
            "{}\x00{}\x00{}\x00{}",
            workspace_id, r#ref.source, r#ref.id, r#ref.hash
        );
        let lock = self.lock_for_key(&key);
        let _guard = lock.lock().unwrap();
        f()
    }

    /// `lockForKey` (skill_cache.go:104–113).
    fn lock_for_key(&self, key: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().unwrap();
        locks.entry(key.to_string()).or_default().clone()
    }

    /// `bundlePath` (skill_cache.go:115–124).
    fn bundle_path(&self, workspace_id: &str, r#ref: &SkillRefData) -> PathBuf {
        self.root
            .join(safe_cache_segment(workspace_id))
            .join(safe_cache_segment(&r#ref.source))
            .join(safe_cache_segment(&r#ref.id))
            .join(safe_cache_segment(&r#ref.hash))
            .join("bundle.json")
    }
}

/// `os.MkdirTemp(parent, ".bundle-*")`: creates a unique `.bundle-*`
/// directory under `parent`.
fn tempfile_builder(parent: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(parent)?;
    tempfile::Builder::new()
        .prefix(".bundle-")
        .tempdir_in(parent)
        .map(|t| t.keep())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}

/// `validateSkillBundle` (skill_cache.go:126–155): identity check, file-count
/// check, safe paths, then manifest-hash verification.
fn validate_skill_bundle(r#ref: &SkillRefData, bundle: &SkillData) -> bool {
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
        files.push(skillbundle::File {
            path: file.path.clone(),
            content: file.content.clone(),
        });
    }
    let manifest = skillbundle::build_manifest(skillbundle::Skill {
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

/// `safeSkillFilePath` (skill_cache.go:157–166): relative slash paths only,
/// already lexically clean.
fn safe_skill_file_path(p: &str) -> bool {
    if p.is_empty() || p.contains('\0') || p.starts_with('/') || p.contains('\\') {
        return false;
    }
    let clean = go_path_clean(p);
    !(clean == "." || clean != p || clean.starts_with("../") || clean == "..")
}

/// Port of Go's `path.Clean` (slash-separated, lexical) — needed because
/// `std::path` normalization differs on separators and drive letters.
pub(crate) fn go_path_clean(p: &str) -> String {
    let rooted = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for comp in p.split('/') {
        match comp {
            "" | "." => {}
            ".." => match out.last() {
                Some(last) if *last != ".." => {
                    out.pop();
                }
                _ if !rooted => out.push(".."),
                _ => {}
            },
            c => out.push(c),
        }
    }
    let joined = out.join("/");
    if rooted {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

/// `safeCacheSegment` (skill_cache.go:168–192): filesystem-safe path segment.
fn safe_cache_segment(s: &str) -> String {
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

    /// testSkillBundle (skill_cache_test.go:77–90).
    fn test_skill_bundle() -> SkillData {
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
        let r#ref = skill_ref_from_bundle(&bundle);
        bundle.hash = r#ref.hash.clone();
        bundle.size_bytes = r#ref.size_bytes;
        bundle.files[0].sha256 = r#ref.files[0].sha256.clone();
        bundle.files[0].size_bytes = r#ref.files[0].size_bytes;
        bundle
    }

    /// TestSkillBundleCacheLoadStore (skill_cache_test.go:10–25).
    #[test]
    fn load_store_roundtrip() {
        let cache = SkillBundleCache::new(tempfile::tempdir().unwrap().keep());
        let bundle = test_skill_bundle();
        let r#ref = skill_ref_from_bundle(&bundle);

        cache.store("ws-1", &bundle).unwrap();
        let got = cache.load("ws-1", &r#ref).expect("expected cache hit");
        assert_eq!(got.content, bundle.content);
        assert_eq!(got.files.len(), 1);
        assert_eq!(got.files[0].content, "rules");
    }

    /// TestSkillBundleCacheStoreRetriesRenameWhenDestinationExists
    /// (skill_cache_test.go:27–55): first rename creates the destination and
    /// fails with EEXIST; Store must recover and land the bundle.
    #[test]
    fn store_retries_rename_when_destination_exists() {
        let cache = SkillBundleCache::new(tempfile::tempdir().unwrap().keep());
        let bundle = test_skill_bundle();
        let r#ref = skill_ref_from_bundle(&bundle);

        let rename_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = rename_calls.clone();
        cache.set_rename(move |old_path, new_path| {
            let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if n == 1 {
                std::fs::create_dir_all(new_path)?;
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, "file exists"));
            }
            std::fs::rename(old_path, new_path)
        });

        cache.store("ws-1", &bundle).unwrap();
        assert_eq!(rename_calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(
            cache.load("ws-1", &r#ref).is_some(),
            "expected cache hit after EEXIST recovery"
        );
        let dir = cache.bundle_path("ws-1", &r#ref);
        assert!(dir.parent().unwrap().exists(), "stored bundle directory missing");
    }

    /// TestSkillBundleCacheRejectsCorruptBundle (skill_cache_test.go:57–75):
    /// a tampered payload is a miss and the cache file is removed.
    #[test]
    fn rejects_corrupt_bundle() {
        let cache = SkillBundleCache::new(tempfile::tempdir().unwrap().keep());
        let bundle = test_skill_bundle();
        let r#ref = skill_ref_from_bundle(&bundle);
        cache.store("ws-1", &bundle).unwrap();

        let path = cache.bundle_path("ws-1", &r#ref);
        std::fs::write(
            &path,
            br#"{"id":"skill-1","source":"workspace","hash":"sha256:bad","content":"tampered"}"#,
        )
        .unwrap();
        assert!(cache.load("ws-1", &r#ref).is_none(), "expected corrupt cache miss");
        assert!(!path.exists(), "expected corrupt cache file to be removed");
    }

    /// safeCacheSegment behavior (skill_cache.go:168–192).
    #[test]
    fn cache_segments_are_sanitized() {
        assert_eq!(safe_cache_segment(""), "_");
        assert_eq!(safe_cache_segment("ws/1"), "ws_1");
        assert_eq!(safe_cache_segment("."), "_.");
        assert_eq!(safe_cache_segment(".."), "_..");
        assert_eq!(safe_cache_segment("abc-DEF_9.x"), "abc-DEF_9.x");
    }

    /// safeSkillFilePath behavior (skill_cache.go:157–166).
    #[test]
    fn skill_file_paths_must_be_clean_relative_slash_paths() {
        assert!(safe_skill_file_path("rules.md"));
        assert!(safe_skill_file_path("a/b/c.md"));
        assert!(!safe_skill_file_path(""));
        assert!(!safe_skill_file_path("/abs.md"));
        assert!(!safe_skill_file_path("back\\slash.md"));
        assert!(!safe_skill_file_path("./rel.md"));
        assert!(!safe_skill_file_path("../up.md"));
        assert!(!safe_skill_file_path("a/../b.md"));
        assert!(!safe_skill_file_path("a/\0b.md"));
    }
}
