//! OpenClaw runtime configuration decoding.
//!
//! S9-integration: consumed by dispatch wiring that lands with integration.
//!
//! Symbol map (Go → Rust):
//! - `openclawRuntimeConfig` → [`OpenclawRuntimeConfig`]
//! - `openclawRuntimeGatewayConfig` → [`OpenclawRuntimeGatewayConfig`]
//! - `decodeOpenclawRuntimeConfig` → [`decode_openclaw_runtime_config`]
//!
//! Port notes: consumes [`OpenclawGatewayPin`](crate::execenv::execenv::OpenclawGatewayPin)
//! from the execenv module (lane E1a). A malformed payload degrades to local
//! mode rather than failing dispatch, exactly as Go does.

#![allow(dead_code)]

use serde::Deserialize;
use tracing::warn;

use crate::execenv::execenv::OpenclawGatewayPin;

/// The schema the daemon expects under an openclaw agent's `runtime_config`
/// JSONB column (openclaw_runtime_config.go:29–32). All fields optional.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct OpenclawRuntimeConfig {
    mode: String,
    gateway: OpenclawRuntimeGatewayConfig,
}

/// The owner-supplied Gateway endpoint (openclaw_runtime_config.go:43–48).
///
/// Trust boundary: in gateway mode the daemon writes this host:port into the
/// per-task wrapper and the spawned openclaw CLI dials it. Operators running a
/// SHARED / managed daemon host should treat it as an SSRF surface and
/// gate/allowlist gateway targets accordingly.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct OpenclawRuntimeGatewayConfig {
    host: String,
    port: i64,
    token: String,
    tls: bool,
}

/// Extracts the openclaw-specific knobs from an agent's runtime_config payload.
///
/// Returns the routing mode plus the gateway pin shaped for execenv. The pin
/// is non-zero only in gateway mode — any other mode drops it so a local-mode
/// payload can't smuggle a bearer token into the per-task wrapper. A malformed
/// payload logs a warning and degrades to local mode (`""`, zero gateway)
/// rather than failing dispatch.
pub(crate) fn decode_openclaw_runtime_config(
    raw: &serde_json::Value,
) -> (String, OpenclawGatewayPin) {
    // Go receives json.RawMessage and treats len(raw)==0 / "null" as absent.
    // The daemon-side Task carries resource payloads as serde_json::Value, so
    // absence is Null here; an empty object behaves identically because every
    // field defaults.
    let raw = match raw {
        serde_json::Value::Null => return (String::new(), OpenclawGatewayPin::default()),
        value => value,
    };
    let cfg: OpenclawRuntimeConfig = match serde_json::from_value(raw.clone()) {
        Ok(cfg) => cfg,
        Err(err) => {
            warn!(
                error = %err,
                "openclaw runtime_config: parse failed; falling back to local mode"
            );
            return (String::new(), OpenclawGatewayPin::default());
        }
    };
    // Surface an unrecognized non-empty mode instead of silently treating it
    // as local — a typo like "gatway" would otherwise leave the user wondering
    // why their gateway config is ignored.
    if cfg.mode != "local" && cfg.mode != "gateway" {
        if !cfg.mode.is_empty() {
            warn!(
                mode = %cfg.mode,
                "openclaw runtime_config: unrecognized mode; falling back to local mode"
            );
        }
        return (cfg.mode, OpenclawGatewayPin::default());
    }
    if cfg.mode != "gateway" {
        return (cfg.mode, OpenclawGatewayPin::default());
    }
    (
        cfg.mode.clone(),
        OpenclawGatewayPin {
            host: cfg.gateway.host,
            port: cfg.gateway.port,
            token: cfg.gateway.token,
            tls: cfg.gateway.tls,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(raw: serde_json::Value) -> (String, OpenclawGatewayPin) {
        decode_openclaw_runtime_config(&raw)
    }

    #[test]
    fn empty_payload_yields_local_mode_and_zero_pin() {
        let (mode, pin) = decode(serde_json::Value::Null);
        assert_eq!(mode, "");
        assert!(pin.is_zero());
    }

    #[test]
    fn gateway_mode_carries_the_pin() {
        let (mode, pin) = decode(json!({
            "mode": "gateway",
            "gateway": {"host": "gw.example.com", "port": 18789, "token": "sekret", "tls": true},
        }));
        assert_eq!(mode, "gateway");
        assert_eq!(pin.host, "gw.example.com");
        assert_eq!(pin.port, 18789);
        assert_eq!(pin.token, "sekret");
        assert!(pin.tls);
    }

    #[test]
    fn malformed_payload_fails_soft_to_local_mode() {
        let (mode, pin) = decode(json!({"mode": 42}));
        assert_eq!(mode, "");
        assert!(pin.is_zero());
    }

    #[test]
    fn mode_only_keeps_the_mode_and_drops_the_pin() {
        for probe in [json!({"mode": ""}), json!({}), json!("")] {
            let (mode, pin) = decode(probe);
            assert_eq!(mode, "");
            assert!(pin.is_zero());
        }
        let (mode, pin) = decode(json!({"mode": "local"}));
        assert_eq!(mode, "local");
        assert!(pin.is_zero());
    }

    #[test]
    fn local_mode_drops_a_smuggled_gateway_pin() {
        let (mode, pin) = decode(json!({
            "mode": "local",
            "gateway": {"host": "internal", "port": 1, "token": "bearer", "tls": false},
        }));
        assert_eq!(mode, "local");
        assert!(
            pin.is_zero(),
            "local mode must never carry the bearer token"
        );
    }

    #[test]
    fn unknown_mode_warns_and_drops_the_pin() {
        let (mode, pin) = decode(json!({
            "mode": "gatway",
            "gateway": {"host": "h", "port": 2, "token": "t", "tls": false},
        }));
        assert_eq!(mode, "gatway");
        assert!(pin.is_zero());
    }
}
