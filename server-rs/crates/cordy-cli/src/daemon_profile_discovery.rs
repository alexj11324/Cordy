//! Daemon profile discovery and validation.
//!
//! Profile filesystem traversal is shared by status, logs, and lifecycle
//! commands. Keeping it independent from health/output code avoids making
//! those commands own the profile discovery policy.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

use super::config::Environment;

pub(crate) fn require_known_daemon_profile(environment: &Environment, profile: &str) -> Result<()> {
    if profile.is_empty() {
        return Ok(());
    }
    let config_path = environment.config_path(profile)?;
    let profile_dir = config_path
        .parent()
        .context("resolve daemon profile directory")?;
    if profile_dir.is_dir() {
        return Ok(());
    }

    let known = known_daemon_profiles(environment);
    if known.is_empty() {
        bail!("unknown profile {profile:?}: no named profiles exist yet");
    }
    bail!(
        "unknown profile {profile:?}\nKnown profiles: {}",
        known.join(", ")
    );
}

pub(crate) fn known_daemon_profiles(environment: &Environment) -> Vec<String> {
    let Ok(config_path) = environment.config_path("") else {
        return Vec::new();
    };
    let Some(config_dir) = config_path.parent() else {
        return Vec::new();
    };
    let profiles_root = config_dir.join("profiles");
    let mut names = Vec::new();
    collect_daemon_profiles(&profiles_root, Path::new(""), &mut names);
    names.sort();
    names
}

fn collect_daemon_profiles(root: &Path, relative: &Path, names: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let child_relative = relative.join(entry.file_name());
        let child = entry.path();
        if child.join("config.json").is_file() {
            names.push(child_relative.to_string_lossy().replace('\\', "/"));
        }
        collect_daemon_profiles(&child, &child_relative, names);
    }
}
