//! Local control client consumed by the production CLI daemon commands.
//!
//! The health port is intentionally profile-derived and can collide. Callers
//! must therefore validate the profile identity returned by `/health` before
//! stopping or restarting the process that answered.

use std::fmt;
use std::time::Duration;

use anyhow::Context;
use serde_json::Value;

use crate::config::DEFAULT_HEALTH_PORT;
use crate::health::HealthResponse;

const LOCAL_CONTROL_TIMEOUT: Duration = Duration::from_secs(2);

/// Matches the Go CLI's deterministic profile-to-health-port mapping.
pub fn health_port_for_profile(profile: &str) -> u16 {
    if profile.is_empty() {
        return DEFAULT_HEALTH_PORT as u16;
    }
    let offset = profile
        .as_bytes()
        .iter()
        .fold(0_u16, |sum, byte| (sum + u16::from(*byte)) % 1000);
    DEFAULT_HEALTH_PORT as u16 + 1 + offset
}

#[derive(Debug, Clone)]
pub enum LocalDaemonHealth {
    Stopped,
    Live(DaemonHealthSnapshot),
}

impl LocalDaemonHealth {
    pub fn is_alive(&self) -> bool {
        matches!(self, Self::Live(_))
    }
}

#[derive(Debug, Clone)]
pub struct DaemonHealthSnapshot {
    pub response: HealthResponse,
    profile_identity: ProfileIdentity,
}

impl DaemonHealthSnapshot {
    /// Confirms that a daemon reached through a potentially colliding port is
    /// the requested profile. A missing field is accepted solely for legacy
    /// daemons; a present malformed field fails closed.
    pub fn confirm_profile(&self, expected: &str, port: u16) -> Result<(), ProfileMismatch> {
        match &self.profile_identity {
            ProfileIdentity::Absent => Ok(()),
            ProfileIdentity::Readable(actual) if actual == expected => Ok(()),
            ProfileIdentity::Readable(actual) => Err(ProfileMismatch {
                expected: expected.to_string(),
                actual: Some(actual.clone()),
                port,
            }),
            ProfileIdentity::Unreadable => Err(ProfileMismatch {
                expected: expected.to_string(),
                actual: None,
                port,
            }),
        }
    }
}

#[derive(Debug, Clone)]
enum ProfileIdentity {
    Absent,
    Readable(String),
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileMismatch {
    pub expected: String,
    pub actual: Option<String>,
    pub port: u16,
}

impl fmt::Display for ProfileMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.actual {
            Some(actual) => write!(
                formatter,
                "port {} is serving profile {:?}, not {:?}",
                self.port, actual, self.expected
            ),
            None => write!(
                formatter,
                "port {} is serving a daemon with an unreadable profile identity",
                self.port
            ),
        }
    }
}

impl std::error::Error for ProfileMismatch {}

#[derive(Clone)]
pub struct DaemonControlClient {
    client: reqwest::Client,
}

impl DaemonControlClient {
    pub fn try_new() -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(LOCAL_CONTROL_TIMEOUT)
            .build()
            .context("build local daemon control client")?;
        Ok(Self { client })
    }

    /// Probes localhost. Connection, status, and decode failures all mean
    /// `Stopped`, matching lifecycle command behavior in the Go CLI.
    pub async fn health(&self, port: u16) -> LocalDaemonHealth {
        let url = format!("http://127.0.0.1:{port}/health");
        let Ok(response) = self.client.get(url).send().await else {
            return LocalDaemonHealth::Stopped;
        };
        if !response.status().is_success() {
            return LocalDaemonHealth::Stopped;
        }
        let Ok(value) = response.json::<Value>().await else {
            return LocalDaemonHealth::Stopped;
        };
        parse_health(value).unwrap_or(LocalDaemonHealth::Stopped)
    }

    /// Requests the same graceful root cancellation used by process signals.
    /// A caller may choose a platform-specific forced-kill fallback if this
    /// request cannot be delivered.
    pub async fn request_shutdown(&self, port: u16) -> anyhow::Result<()> {
        let url = format!("http://127.0.0.1:{port}/shutdown");
        let response = self
            .client
            .post(url)
            .send()
            .await
            .context("request daemon shutdown")?;
        anyhow::ensure!(
            response.status().is_success(),
            "daemon shutdown returned status {}",
            response.status()
        );
        Ok(())
    }
}

fn parse_health(value: Value) -> Option<LocalDaemonHealth> {
    let status = value.get("status")?.as_str()?;
    if status != "running" && status != "starting" {
        return Some(LocalDaemonHealth::Stopped);
    }
    let profile_identity = match value.get("profile") {
        None => ProfileIdentity::Absent,
        Some(Value::String(profile)) => ProfileIdentity::Readable(profile.clone()),
        Some(_) => ProfileIdentity::Unreadable,
    };
    // Preserve the fail-closed identity classification while still decoding
    // the remaining diagnostics from an otherwise healthy legacy/malformed
    // response into the strongly typed public snapshot.
    let mut response_value = value;
    if matches!(&profile_identity, ProfileIdentity::Unreadable) {
        response_value["profile"] = Value::String(String::new());
    }
    let response = serde_json::from_value(response_value).ok()?;
    Some(LocalDaemonHealth::Live(DaemonHealthSnapshot {
        response,
        profile_identity,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(value: Value) -> DaemonHealthSnapshot {
        match parse_health(value).expect("health should parse") {
            LocalDaemonHealth::Live(snapshot) => snapshot,
            LocalDaemonHealth::Stopped => panic!("expected live daemon"),
        }
    }

    #[test]
    fn profile_ports_match_go_hash_and_allow_collisions() {
        assert_eq!(health_port_for_profile(""), 19514);
        assert_eq!(health_port_for_profile("ab"), 19710);
        assert_eq!(health_port_for_profile("ab"), health_port_for_profile("ba"));
    }

    #[test]
    fn starting_is_live_and_missing_legacy_identity_is_accepted() {
        let snapshot = live(serde_json::json!({"status":"starting", "pid":42}));
        assert!(snapshot.confirm_profile("named", 19515).is_ok());
    }

    #[test]
    fn readable_profile_collision_is_rejected() {
        let snapshot = live(serde_json::json!({
            "status":"running",
            "pid":42,
            "profile":"ba"
        }));
        assert_eq!(
            snapshot.confirm_profile("ab", 19710),
            Err(ProfileMismatch {
                expected: "ab".to_string(),
                actual: Some("ba".to_string()),
                port: 19710,
            })
        );
    }

    #[test]
    fn malformed_present_profile_fails_closed() {
        let snapshot = live(serde_json::json!({
            "status":"running",
            "pid":42,
            "profile":null
        }));
        assert_eq!(
            snapshot.confirm_profile("", 19514),
            Err(ProfileMismatch {
                expected: String::new(),
                actual: None,
                port: 19514,
            })
        );
    }

    #[test]
    fn malformed_or_non_live_health_is_stopped() {
        assert!(matches!(
            parse_health(serde_json::json!({"status":"stopped"})),
            Some(LocalDaemonHealth::Stopped)
        ));
        assert!(parse_health(serde_json::json!({"status":7})).is_none());
    }
}
