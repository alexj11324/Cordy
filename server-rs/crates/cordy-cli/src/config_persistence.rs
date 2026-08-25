//! Locked, permission-restricted, atomic persistence for CLI profile documents.
//!
//! `Environment` owns the public profile operations and task-context policy;
//! this module owns only the filesystem mechanics shared by those operations.
//! Keeping the mechanics here prevents individual config mutations from
//! drifting in lock, permission, or replacement behavior.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) fn read_config_document(path: &Path) -> Result<Value> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Value::Object(serde_json::Map::new()))
        }
        Err(error) => return Err(error).context("read CLI config"),
    };
    let document: Value = serde_json::from_slice(&data).context("parse CLI config")?;
    if !document.is_object() {
        bail!("parse CLI config: expected a JSON object");
    }
    Ok(document)
}

pub(crate) fn ensure_config_directory(directory: &Path, task_root: Option<&str>) -> Result<()> {
    fs::create_dir_all(directory).context("create CLI config directory")?;
    let Some(task_root) = task_root else {
        return Ok(());
    };
    let task_root = Path::new(task_root);
    let mut current = directory;
    loop {
        restrict_directory_permissions(current)?;
        if current == task_root {
            return Ok(());
        }
        current = current.parent().with_context(|| {
            format!(
                "task-local CLI config directory {:?} escapes root {:?}",
                directory, task_root
            )
        })?;
    }
}

pub(crate) fn write_json_atomically(path: &Path, document: &Value) -> Result<()> {
    let directory = path.parent().context("resolve CLI config directory")?;
    let mut data = serde_json::to_vec_pretty(document).context("encode CLI config")?;
    data.push(b'\n');
    let (mut temporary, temporary_path) = create_config_temp_file(directory)?;
    let result = (|| -> Result<()> {
        temporary
            .write_all(&data)
            .context("write temp config file")?;
        temporary.sync_all().context("sync temp config file")?;
        drop(temporary);
        restrict_file_permissions(&temporary_path)?;
        fs::rename(&temporary_path, path).context("rename config file")?;
        sync_directory(directory)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn create_config_temp_file(directory: &Path) -> Result<(File, PathBuf)> {
    for attempt in 0..100_u8 {
        let path = directory.join(format!(".config-{}-{attempt}.json.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create temp config file"),
        }
    }
    bail!("create temp config file: exhausted unique names")
}

#[cfg(unix)]
pub(crate) fn restrict_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).context("chmod CLI config file")
}

#[cfg(not(unix))]
pub(crate) fn restrict_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn restrict_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .context("restrict task-local CLI config directory")
}

#[cfg(not(unix))]
fn restrict_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<()> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .context("sync CLI config directory")
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> Result<()> {
    Ok(())
}
