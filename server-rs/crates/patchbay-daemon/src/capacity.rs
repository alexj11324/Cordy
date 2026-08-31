//! Local ACP capacity snapshots.
//!
//! Capacity is deliberately a daemon concern.  Providers may expose usage
//! through local account files or authenticated local commands, but the raw
//! credential and provider response never leave this process.  The control
//! plane receives only the redacted [`CapacitySnapshot`] projection.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);
pub const DEFAULT_STALE_AFTER: Duration = Duration::from_secs(30 * 60);
pub const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(60 * 60);

/// Providers currently understood by the daemon's capacity adapter.  The
/// adapter remains provider-neutral so unsupported providers can be reported
/// honestly instead of inventing a quota value.
pub const SUPPORTED_PROVIDERS: &[&str] = &[
    "claude",
    "codex",
    "gemini",
    "opencode-go",
    "kimi",
    "minimax",
    "grok",
    "antigravity",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityState {
    Available,
    RateLimited,
    Stale,
    Unsupported,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityErrorCode {
    Authentication,
    Io,
    Parse,
    RateLimit,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageWindow {
    pub name: String,
    pub used_percent: Option<f64>,
    pub reset_at: Option<DateTime<Utc>>,
    pub window_start: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapacitySnapshot {
    pub provider: String,
    /// A one-way identifier.  Never use an email, token, or account path as
    /// the value sent to the control plane.
    pub account_fingerprint: Option<String>,
    pub plan: Option<String>,
    pub windows: Vec<UsageWindow>,
    pub used_percent: Option<f64>,
    pub reset_at: Option<DateTime<Utc>>,
    pub state: CapacityState,
    pub sampled_at: DateTime<Utc>,
    pub error_code: Option<CapacityErrorCode>,
}

impl CapacitySnapshot {
    pub fn available(
        provider: impl Into<String>,
        account_fingerprint: Option<String>,
        plan: Option<String>,
        windows: Vec<UsageWindow>,
        sampled_at: DateTime<Utc>,
    ) -> Self {
        let used_percent = windows
            .iter()
            .filter_map(|window| window.used_percent)
            .reduce(f64::max);
        let reset_at = windows.iter().filter_map(|window| window.reset_at).min();
        Self {
            provider: provider.into(),
            account_fingerprint,
            plan,
            windows,
            used_percent,
            reset_at,
            state: CapacityState::Available,
            sampled_at,
            error_code: None,
        }
    }

    pub fn unsupported(provider: impl Into<String>, sampled_at: DateTime<Utc>) -> Self {
        Self {
            provider: provider.into(),
            account_fingerprint: None,
            plan: None,
            windows: Vec::new(),
            used_percent: None,
            reset_at: None,
            state: CapacityState::Unsupported,
            sampled_at,
            error_code: Some(CapacityErrorCode::Unsupported),
        }
    }

    pub fn with_state(mut self, state: CapacityState, error_code: Option<CapacityErrorCode>) -> Self {
        self.state = state;
        self.error_code = error_code;
        self
    }

    pub fn is_admissible(&self, now: DateTime<Utc>, stale_after: Duration) -> bool {
        self.state == CapacityState::Available
            && now
                .signed_duration_since(self.sampled_at)
                .to_std()
                .is_ok_and(|age| age <= stale_after)
    }

    pub fn mark_stale_if_needed(&mut self, now: DateTime<Utc>, stale_after: Duration) {
        let stale = now
            .signed_duration_since(self.sampled_at)
            .to_std()
            .is_ok_and(|age| age > stale_after);
        if stale && self.state == CapacityState::Available {
            self.state = CapacityState::Stale;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawCapacitySnapshot {
    pub provider: String,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub account_fingerprint: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub windows: Vec<UsageWindow>,
    #[serde(default)]
    pub used_percent: Option<f64>,
    #[serde(default)]
    pub reset_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub state: Option<CapacityState>,
    #[serde(default)]
    pub sampled_at: Option<DateTime<Utc>>,
}

impl RawCapacitySnapshot {
    fn into_snapshot(self, now: DateTime<Utc>) -> Result<CapacitySnapshot, CapacityError> {
        validate_percent(self.used_percent)?;
        for window in &self.windows {
            validate_percent(window.used_percent)?;
        }
        let fingerprint = self
            .account_fingerprint
            .or_else(|| self.account.as_deref().map(fingerprint_account));
        let mut snapshot = CapacitySnapshot::available(
            self.provider,
            fingerprint,
            self.plan,
            self.windows,
            self.sampled_at.unwrap_or(now),
        );
        if let Some(percent) = self.used_percent {
            snapshot.used_percent = Some(percent);
        }
        if self.reset_at.is_some() {
            snapshot.reset_at = self.reset_at;
        }
        if let Some(state) = self.state {
            snapshot = snapshot.with_state(
                state,
                (state != CapacityState::Available).then_some(match state {
                    CapacityState::RateLimited => CapacityErrorCode::RateLimit,
                    CapacityState::Unsupported => CapacityErrorCode::Unsupported,
                    CapacityState::Stale | CapacityState::Error => CapacityErrorCode::Unknown,
                    CapacityState::Available => CapacityErrorCode::Unknown,
                }),
            );
        }
        Ok(snapshot)
    }
}

fn validate_percent(value: Option<f64>) -> Result<(), CapacityError> {
    if value.is_some_and(|percent| !percent.is_finite() || !(0.0..=100.0).contains(&percent)) {
        return Err(CapacityError::new(
            CapacityErrorCode::Parse,
            "usage percent must be a finite value between 0 and 100",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("capacity {code:?}: {message}")]
pub struct CapacityError {
    pub code: CapacityErrorCode,
    pub message: String,
}

impl CapacityError {
    pub fn new(code: CapacityErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub fn provider_supported(provider: &str) -> bool {
    SUPPORTED_PROVIDERS.contains(&provider.trim().to_ascii_lowercase().as_str())
}

/// Return a stable, non-secret account identifier suitable for telemetry and
/// the control plane.  The input is intentionally accepted only by the local
/// daemon adapter and is never serialized as-is.
pub fn fingerprint_account(account: &str) -> String {
    let digest = Sha256::digest(account.trim().as_bytes());
    format!("sha256:{}", hex::encode(&digest[..16]))
}

pub trait CapacityReader: Send + Sync {
    fn read(&self, provider: &str, now: DateTime<Utc>) -> Result<CapacitySnapshot, CapacityError>;
}

/// Reads daemon-owned JSON snapshots.  This is intentionally a fixture-like
/// adapter: provider-specific credential parsing belongs in a provider's
/// local implementation, while this shared reader validates and redacts the
/// common projection.
#[derive(Debug, Clone)]
pub struct LocalSnapshotReader {
    root: PathBuf,
}

impl LocalSnapshotReader {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path_for(&self, provider: &str) -> PathBuf {
        self.root.join(format!("{provider}.json"))
    }
}

impl CapacityReader for LocalSnapshotReader {
    fn read(&self, provider: &str, now: DateTime<Utc>) -> Result<CapacitySnapshot, CapacityError> {
        let path = self.path_for(provider);
        let bytes = fs::read(&path).map_err(|error| {
            CapacityError::new(CapacityErrorCode::Io, format!("read local usage snapshot: {error}"))
        })?;
        let raw: RawCapacitySnapshot = serde_json::from_slice(&bytes).map_err(|error| {
            CapacityError::new(CapacityErrorCode::Parse, format!("parse local usage snapshot: {error}"))
        })?;
        if raw.provider.trim().eq_ignore_ascii_case(provider) {
            raw.into_snapshot(now)
        } else {
            Err(CapacityError::new(
                CapacityErrorCode::Parse,
                "usage snapshot provider does not match the requested provider",
            ))
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RefreshPolicy {
    pub refresh_interval: Duration,
    pub stale_after: Duration,
    pub max_backoff: Duration,
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        Self {
            refresh_interval: DEFAULT_REFRESH_INTERVAL,
            stale_after: DEFAULT_STALE_AFTER,
            max_backoff: DEFAULT_MAX_BACKOFF,
        }
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    snapshot: CapacitySnapshot,
    next_attempt: DateTime<Utc>,
    backoff: Duration,
}

/// In-process cache with bounded retry backoff.  It never stores raw provider
/// credentials; only the redacted snapshot and error classification remain.
#[derive(Debug)]
pub struct CapacityCache {
    policy: RefreshPolicy,
    entries: std::sync::Mutex<HashMap<String, CacheEntry>>,
}

impl CapacityCache {
    pub fn new(policy: RefreshPolicy) -> Self {
        Self {
            policy,
            entries: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn snapshot(&self, provider: &str, now: DateTime<Utc>) -> Option<CapacitySnapshot> {
        let mut entries = self.entries.lock().expect("capacity cache lock poisoned");
        let entry = entries.get_mut(provider)?;
        entry
            .snapshot
            .mark_stale_if_needed(now, self.policy.stale_after);
        Some(entry.snapshot.clone())
    }

    pub fn should_refresh(&self, provider: &str, now: DateTime<Utc>, force: bool) -> bool {
        if force {
            return true;
        }
        self.entries
            .lock()
            .expect("capacity cache lock poisoned")
            .get(provider)
            .is_none_or(|entry| now >= entry.next_attempt)
    }

    pub fn refresh<R: CapacityReader>(
        &self,
        reader: &R,
        provider: &str,
        now: DateTime<Utc>,
        force: bool,
    ) -> Option<CapacitySnapshot> {
        if !self.should_refresh(provider, now, force) {
            return self.snapshot(provider, now);
        }
        match reader.read(provider, now) {
            Ok(snapshot) => {
                self.entries
                    .lock()
                    .expect("capacity cache lock poisoned")
                    .insert(
                        provider.to_string(),
                        CacheEntry {
                            snapshot: snapshot.clone(),
                            next_attempt: now + chrono_duration(self.policy.refresh_interval),
                            backoff: self.policy.refresh_interval,
                        },
                    );
                Some(snapshot)
            }
            Err(error) => {
                let mut entries = self.entries.lock().expect("capacity cache lock poisoned");
                let previous = entries.get(provider).cloned();
                let backoff = previous
                    .as_ref()
                    .map(|entry| next_backoff(entry.backoff, self.policy.max_backoff))
                    .unwrap_or(self.policy.refresh_interval);
                let snapshot = previous
                    .map(|entry| {
                        entry.snapshot.with_state(
                            if error.code == CapacityErrorCode::RateLimit {
                                CapacityState::RateLimited
                            } else {
                                CapacityState::Error
                            },
                            Some(error.code),
                        )
                    })
                    .unwrap_or_else(|| CapacitySnapshot {
                        provider: provider.to_string(),
                        account_fingerprint: None,
                        plan: None,
                        windows: Vec::new(),
                        used_percent: None,
                        reset_at: None,
                        state: if error.code == CapacityErrorCode::RateLimit {
                            CapacityState::RateLimited
                        } else {
                            CapacityState::Error
                        },
                        sampled_at: now,
                        error_code: Some(error.code),
                    });
                entries.insert(
                    provider.to_string(),
                    CacheEntry {
                        snapshot: snapshot.clone(),
                        next_attempt: now + chrono_duration(backoff),
                        backoff,
                    },
                );
                Some(snapshot)
            }
        }
    }
}

fn next_backoff(current: Duration, max: Duration) -> Duration {
    current.checked_mul(2).unwrap_or(max).min(max)
}

fn chrono_duration(duration: Duration) -> chrono::Duration {
    chrono::Duration::from_std(duration).unwrap_or_else(|_| chrono::Duration::hours(1))
}

/// A small per-home lock prevents two local daemon processes from refreshing
/// the same provider credentials at once.  The lock file contains no secret;
/// it is removed when the guard is dropped.
#[derive(Debug)]
pub struct HomeCapacityLock {
    path: PathBuf,
    file: Option<File>,
}

impl HomeCapacityLock {
    pub fn acquire(home: &Path) -> Result<Option<Self>, CapacityError> {
        let path = home.join(".patchbay-capacity.lock");
        let file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
            Err(error) => {
                return Err(CapacityError::new(
                    CapacityErrorCode::Io,
                    format!("create capacity lock: {error}"),
                ));
            }
        };
        Ok(Some(Self {
            path,
            file: Some(file),
        }))
    }
}

impl Drop for HomeCapacityLock {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

pub fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_fingerprint_is_stable_and_redacted() {
        let first = fingerprint_account(" user@example.com ");
        assert_eq!(first, fingerprint_account("user@example.com"));
        assert!(!first.contains("user@example.com"));
        assert!(first.starts_with("sha256:"));
    }

    #[test]
    fn unsupported_provider_is_not_admissible() {
        let snapshot = CapacitySnapshot::unsupported("unknown", Utc::now());
        assert!(!snapshot.is_admissible(Utc::now(), DEFAULT_STALE_AFTER));
    }

    #[test]
    fn raw_snapshot_rejects_invalid_percent() {
        let result = RawCapacitySnapshot {
            provider: "codex".into(),
            account: None,
            account_fingerprint: None,
            plan: None,
            windows: Vec::new(),
            used_percent: Some(101.0),
            reset_at: None,
            state: None,
            sampled_at: None,
        }
        .into_snapshot(Utc::now());
        assert!(matches!(result, Err(CapacityError { code: CapacityErrorCode::Parse, .. })));
    }

    #[test]
    fn lock_is_exclusive() {
        let home = std::env::temp_dir().join(format!("patchbay-capacity-{}", Uuid::now_v7()));
        fs::create_dir_all(&home).expect("create temp home");
        let first = HomeCapacityLock::acquire(&home).expect("acquire lock");
        assert!(first.is_some());
        assert!(HomeCapacityLock::acquire(&home)
            .expect("second acquire")
            .is_none());
        drop(first);
        assert!(HomeCapacityLock::acquire(&home)
            .expect("reacquire")
            .is_some());
        let _ = fs::remove_dir_all(home);
    }
}
