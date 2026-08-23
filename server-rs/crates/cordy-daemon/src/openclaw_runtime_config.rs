//! Port of `server/internal/daemon/openclaw_runtime_config.go` (lines 1–86).
//!
//! Schema the daemon expects under an openclaw agent's `runtime_config`
//! JSONB column. All fields are optional; absence (or the agent's whole
//! runtime_config being null/empty) keeps the historical embedded behaviour
//! so existing agents are unaffected.
//!
//! Schema (issue #3260):
//!
//! ```json
//! {
//!   "mode": "local" | "gateway",
//!   "gateway": { "host": "...", "port": 18789, "token": "...", "tls": false }
//! }
//! ```
//!
//! Other providers' runtime_config payloads pass through untouched — this
//! decoder only reads keys that have meaning for the openclaw backend.
//!
//! Deviations from Go:
//! - `log/slog` → `tracing` with identical message text (no logger parameter;
//!   the global subscriber plays slog's role).
//! - `execenv.OpenclawGatewayPin` → local [`OpenclawGatewayPin`] seam that
//!   mirrors the execenv stand-in field-for-field (S9-integration below).

// S9-integration: dead_code until Daemon core wires this.
#![allow(dead_code)]

use serde::Deserialize;

// ---------------------------------------------------------------------------
// S9-integration seam stand-ins (openclaw_runtime_config.go imports execenv).
// ---------------------------------------------------------------------------

/// S9-integration seam: mirrors `execenv.OpenclawGatewayPin`
/// (execenv/execenv.rs:1528–1573) so the type can be swapped 1:1 when the
/// daemon core wires this decoder into task dispatch. Field shape matches
/// openclawRuntimeGatewayConfig's pin projection (openclaw_config.go).
///
/// Deviation vs Go (inherited from the execenv stand-in): the public Go type
/// masks Token in MarshalJSON and Display. This seam serializes plainly (the
/// isolation helper protocol requires the real token over stdin anyway) and
/// masks only in `Display`/`Debug`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct OpenclawGatewayPin {
    pub host: String,
    pub port: i64,
    pub token: String,
    pub tls: bool,
}

impl OpenclawGatewayPin {
    /// IsZero reports whether every field is zero, i.e. there is nothing to
    /// pin (`execenv.OpenclawGatewayPin.IsZero`, openclaw_config.go).
    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }
}

impl std::fmt::Display for OpenclawGatewayPin {
    /// Masks the bearer token when the pin is rendered as a string
    /// (issue #3260 CR).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tok = if self.token.is_empty() { "" } else { "***" };
        write!(
            f,
            "OpenclawGatewayPin{{Host:{:?} Port:{} Token:{} TLS:{}}}",
            self.host, self.port, tok, self.tls
        )
    }
}

impl std::fmt::Debug for OpenclawGatewayPin {
    /// Masks the bearer token (Go `%+v` routes through MarshalJSON, which
    /// redacts; the Rust analogue is a hand-written Debug).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tok = if self.token.is_empty() { "" } else { "***" };
        f.debug_struct("OpenclawGatewayPin")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("token", &tok)
            .field("tls", &self.tls)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Wire schema (openclaw_runtime_config.go:29–48).
// ---------------------------------------------------------------------------

/// `openclawRuntimeConfig` (openclaw_runtime_config.go:29–32).
#[derive(Debug, Default, Deserialize)]
struct OpenclawRuntimeConfig {
    #[serde(default)]
    mode: String,
    #[serde(default)]
    gateway: OpenclawRuntimeGatewayConfig,
}

/// `openclawRuntimeGatewayConfig` (openclaw_runtime_config.go:43–48): the
/// owner-supplied Gateway endpoint.
///
/// Trust boundary: in gateway mode the daemon writes this host:port into the
/// per-task wrapper and the spawned openclaw CLI dials it. For self-hosted,
/// single-tenant daemons this is the same trust level as custom_args /
/// custom_env — the owner already controls the daemon host. Operators running
/// a SHARED / managed daemon host should treat it as an SSRF surface (an agent
/// owner could point the daemon at an arbitrary internal address) and
/// gate/allowlist gateway targets accordingly.
#[derive(Debug, Default, Deserialize)]
struct OpenclawRuntimeGatewayConfig {
    #[serde(default)]
    host: String,
    #[serde(default)]
    port: i64,
    #[serde(default)]
    token: String,
    #[serde(default)]
    tls: bool,
}

/// `decodeOpenclawRuntimeConfig` (openclaw_runtime_config.go:50–86): extracts
/// the openclaw-specific knobs from an agent's runtime_config payload.
/// Returns the routing mode plus the gateway pin shaped for execenv. The pin
/// is non-zero only in gateway mode — any other mode drops it so a local-mode
/// payload can't smuggle a bearer token into the per-task wrapper. A malformed
/// payload logs a warning and degrades to local mode (mode="", zero gateway)
/// rather than failing dispatch — the alternative would let one bad save block
/// every task that agent runs.
pub(crate) fn decode_openclaw_runtime_config(raw: &[u8]) -> (String, OpenclawGatewayPin) {
    if raw.is_empty() {
        return (String::new(), OpenclawGatewayPin::default());
    }
    let cfg: OpenclawRuntimeConfig = match serde_json::from_slice(raw) {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "openclaw runtime_config: parse failed; falling back to local mode"
            );
            return (String::new(), OpenclawGatewayPin::default());
        }
    };
    // Surface an unrecognized non-empty mode instead of silently treating it
    // as local — a typo like "gatway" would otherwise leave the user wondering
    // why their gateway config is ignored.
    if !cfg.mode.is_empty() && cfg.mode != "local" && cfg.mode != "gateway" {
        tracing::warn!(
            mode = %cfg.mode,
            "openclaw runtime_config: unrecognized mode; falling back to local mode"
        );
    }
    // Only gateway mode consults the pin. For every other mode (local / empty /
    // unrecognized) drop the gateway block so a stray
    // {"mode":"local","gateway":{...,"token":"..."}} never writes the bearer
    // token into the 0o600 per-task wrapper that `--local` makes openclaw ignore.
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

    /// Minimal capturing subscriber so tests can assert WARN text without a
    /// tracing-subscriber dependency (test-only; mirrors quietLogger /
    /// buffer-backed slog handlers in openclaw_runtime_config_test.go).
    struct CaptureSubscriber {
        buf: std::sync::Arc<std::sync::Mutex<String>>,
    }

    impl tracing::Subscriber for CaptureSubscriber {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::Id {
            tracing::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut out = self.buf.lock().unwrap();
            out.push_str(event.metadata().name());
            out.push('\n');
            struct Visitor(String);
            impl tracing::field::Visit for Visitor {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    self.0.push_str(field.name());
                    self.0.push('=');
                    self.0.push_str(&format!("{value:?}"));
                    self.0.push(' ');
                }
            }
            let mut visitor = Visitor(String::new());
            event.record(&mut visitor);
            out.push_str(&visitor.0);
            out.push('\n');
        }

        fn enter(&self, _span: &tracing::span::Id) {}
        fn exit(&self, _span: &tracing::span::Id) {}
        fn clone_span(&self, id: &tracing::span::Id) -> tracing::Id {
            id.clone()
        }
        fn try_close(&self, _id: tracing::span::Id) -> bool {
            true
        }
    }

    #[test]
    fn decode_openclaw_runtime_config_empty() {
        let (mode, gw) = decode_openclaw_runtime_config(b"");
        assert_eq!(mode, "", "mode for nil payload");
        assert!(gw.is_zero(), "gateway for nil payload: {gw:?}");
    }

    #[test]
    fn decode_openclaw_runtime_config_gateway_mode() {
        let raw = br#"{
            "mode": "gateway",
            "gateway": {"host": "gw.internal", "port": 18789, "token": "secret", "tls": true}
        }"#;
        let (mode, gw) = decode_openclaw_runtime_config(raw);
        assert_eq!(mode, "gateway");
        assert_eq!(
            gw,
            OpenclawGatewayPin {
                host: "gw.internal".into(),
                port: 18789,
                token: "secret".into(),
                tls: true,
            }
        );
    }

    #[test]
    fn decode_openclaw_runtime_config_malformed_fails_soft_to_local() {
        // A broken JSON blob must never block dispatch — the agent runs in the
        // historical embedded mode until the user fixes the config.
        let (mode, gw) = decode_openclaw_runtime_config(br#"{"mode": "gateway""#);
        assert_eq!(mode, "", "mode for malformed payload");
        assert!(gw.is_zero(), "gateway for malformed payload: {gw:?}");
    }

    #[test]
    fn decode_openclaw_runtime_config_mode_only() {
        // Users may switch to gateway mode and rely on the daemon host's local
        // ~/.openclaw/openclaw.json for the endpoint — gateway block stays zero.
        let (mode, gw) = decode_openclaw_runtime_config(br#"{"mode": "gateway"}"#);
        assert_eq!(mode, "gateway");
        assert!(gw.is_zero(), "gateway: {gw:?}");
    }

    /// TestOpenclawGatewayPinDefaultFormattingMasksToken — default formatters
    /// must NOT print the bearer token verbatim. Guards against the secondary
    /// leak path called out in the issue #3260 CR.
    ///
    /// Deviation vs Go: the Go test also asserts MarshalJSON redaction; this
    /// seam intentionally serializes plainly (see module docs), so only the
    /// formatter paths are asserted here.
    #[test]
    fn openclaw_gateway_pin_default_formatting_masks_token() {
        let pin = OpenclawGatewayPin {
            host: "gw.internal".into(),
            port: 18789,
            token: "real-secret".into(),
            tls: true,
        };

        let disp = format!("{pin}");
        assert!(!disp.contains("real-secret"), "Display leaks token: {disp}");
        assert!(disp.contains("gw.internal"), "Display dropped host: {disp}");

        let dbg = format!("{pin:?}");
        assert!(!dbg.contains("real-secret"), "Debug leaks token: {dbg}");
    }

    /// TestDecodeOpenclawRuntimeConfigLocalModeDropsGatewayPin — a local-mode
    /// payload that still carries a gateway block (craftable via a direct
    /// PATCH) must not surface the pin.
    #[test]
    fn decode_openclaw_runtime_config_local_mode_drops_gateway_pin() {
        let raw = br#"{
            "mode": "local",
            "gateway": {"host": "gw.internal", "port": 18789, "token": "secret", "tls": true}
        }"#;
        let (mode, gw) = decode_openclaw_runtime_config(raw);
        assert_eq!(mode, "local");
        assert!(gw.is_zero(), "gateway for local mode: {gw:?}");
    }

    /// TestDecodeOpenclawRuntimeConfigUnknownModeWarnsAndDropsPin — a typo'd
    /// mode neither behaves like gateway nor silently like local: it falls
    /// back to local (zero pin) AND logs a WARN so the misconfiguration is
    /// discoverable.
    #[test]
    fn decode_openclaw_runtime_config_unknown_mode_warns_and_drops_pin() {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let subscriber = CaptureSubscriber {
            buf: std::sync::Arc::clone(&buf),
        };
        let raw = br#"{
            "mode": "gatway",
            "gateway": {"host": "gw.internal", "port": 18789, "token": "secret"}
        }"#;
        let (mode, gw) =
            tracing::subscriber::with_default(subscriber, || decode_openclaw_runtime_config(raw));
        assert_eq!(mode, "gatway");
        assert!(gw.is_zero(), "gateway for unknown mode: {gw:?}");
        let captured = buf.lock().unwrap().clone();
        assert!(
            captured.contains("unrecognized mode"),
            "expected WARN about unrecognized mode, got: {captured:?}"
        );
    }
}
