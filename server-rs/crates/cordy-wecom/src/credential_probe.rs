//! Proving the installer controls the bot — port of `credential_probe.go`.
//!
//! A BYO install used to persist whatever pair was pasted into the dialog.
//! The bot id is not a secret: it is on the WeCom admin console, and any
//! member of a workspace can read it back out of GET /wecom/installations.
//! Only the secret proves anything, and nothing checked it.
//!
//! Two things followed, and neither needs an insider:
//!
//! The routing slot is global. channel_installation's unique index on
//! (channel_type, config->>'app_id') has no workspace in it, so an admin of
//! any workspace on the deployment could write a row claiming somebody else's
//! bot id with a junk secret and hold that slot indefinitely.
//!
//! Worse, the reclaim is keyed on the same unproven id. A bot the rightful
//! owner has merely DISCONNECTED sits revoked, and a revoked row is what the
//! reclaim hard-deletes — along with every user binding, session binding and
//! pending token beneath it.
//!
//! The fix is the one both peers already use: prove control before
//! persisting. WeCom has no REST surface, so the proof is the protocol's own
//! handshake — dial, aibot_subscribe, read the verdict, hang up. A wrong
//! secret is refused with a non-zero errcode, which is exactly the signal.
//!
//! The probe never runs against a bot somebody else is actively connected to.
//! WeCom allows one live subscriber per bot, so a probe against a live slot
//! would displace the rightful owner's connection. InstallationService.upsert
//! enforces this: it takes the slot's advisory lock, reads the current owner,
//! and returns the conflict without probing for any live owner other than the
//! caller's own row.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::wecom_channel::{DEFAULT_WS_URL, HANDSHAKE_TIMEOUT, SUBSCRIBE_TIMEOUT};
use crate::ws_frame::{subscribe_body, FrameEnvelope, CMD_SUBSCRIBE};
use crate::ws_sender::{new_req_id, DefaultDialer, Dialer, WsConn};
/// WeCom saying the pair is not valid: a wrong secret, a bot that no longer
/// exists, a bot whose API mode is off. It is the answer that must reach the
/// admin, because it is the one they can act on.
///
/// Everything else — the dial failed, the handshake timed out, the network is
/// down — is [`CredentialError::Unverifiable`]. Distinct from rejection on
/// purpose: telling an admin their credentials are wrong when the deployment
/// simply could not reach WeCom sends them to rotate a secret that was fine,
/// and a rotated one cannot be recovered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CredentialError {
    #[error("wecom: WeCom rejected this bot id and secret (errcode {code}: {msg})")]
    Rejected { code: i64, msg: String },
    #[error("wecom: could not reach WeCom to verify this bot (subscribe returned errcode {code}: {msg})")]
    UnverifiableAck { code: i64, msg: String },
    #[error("wecom: could not reach WeCom to verify this bot: {0}")]
    Unverifiable(String),
}

/// `errors.Is(err, ErrCredentialsRejected)` equivalent: walks the chain.
pub fn is_credentials_rejected(err: &anyhow::Error) -> bool {
    err.chain().any(|c| {
        matches!(
            c.downcast_ref::<CredentialError>(),
            Some(CredentialError::Rejected { .. })
        )
    })
}

/// `errors.Is(err, ErrCredentialsUnverifiable)` equivalent.
pub fn is_credentials_unverifiable(err: &anyhow::Error) -> bool {
    err.chain().any(|c| {
        matches!(
            c.downcast_ref::<CredentialError>(),
            Some(CredentialError::Unverifiable { .. } | CredentialError::UnverifiableAck { .. })
        )
    })
}

fn unverifiable(detail: impl std::fmt::Display) -> anyhow::Error {
    anyhow::Error::new(CredentialError::Unverifiable(detail.to_string()))
}

/// The WeCom global error codes (document/path/90313) documented as a refusal
/// of the credential pair itself — the only answers entitled to tell an admin
/// their Bot ID or secret is wrong:
/// - 40001 不合法的secret参数 — the secret does not match this bot
/// - 40013 不合法的CorpID — the identity half of the pair is not valid
///
/// Everything else non-zero is unverifiable, deliberately. WeCom only
/// guarantees that 0 means success; the subscribe path is also under
/// frequency and concurrency protection (45009, 45033), and the platform can
/// fail on its own account. Reading any non-zero code as "wrong secret"
/// pushes an admin to rotate a long-connection secret that was fine. So the
/// list is a whitelist and the default is fail-closed: refuse the install,
/// keep the stored credentials untouched, and say we could not verify.
///
/// Codes only, never errmsg text: the message is human-facing Chinese prose
/// that WeCom is free to reword.
const REJECTION_ERR_CODES: [i64; 2] = [40001, 40013];

/// Turns a non-zero errcode on an aibot_subscribe ack into one of the two
/// credential verdicts above.
///
/// Both readers of that ack go through here — the probe at install time, and
/// the channel's own handshake on every reconnect. It is one function rather
/// than two copies of the same map lookup so the two cannot give different
/// answers to the same code: a 45009 throttle that is "unverifiable, wait" at
/// install time must not be "somebody go fix this installation" on the
/// connect path.
pub fn classify_subscribe_ack(err_code: i64, err_msg: &str) -> anyhow::Error {
    if REJECTION_ERR_CODES.contains(&err_code) {
        // The one answer the admin can act on, and the only branch that
        // blames their credentials.
        return anyhow::Error::new(CredentialError::Rejected {
            code: err_code,
            msg: err_msg.to_string(),
        });
    }
    // Unknown non-zero: throttling, a platform-side failure, or a code WeCom
    // added since. Fail closed rather than blame the secret. The raw pair is
    // logged here so an operator can see it even if the caller only surfaces
    // the sentinel.
    tracing::warn!(
        errcode = err_code,
        errmsg = %err_msg,
        "wecom: unrecognized subscribe errcode, treating as unverifiable"
    );
    anyhow::Error::new(CredentialError::UnverifiableAck {
        code: err_code,
        msg: err_msg.to_string(),
    })
}

/// Bounds the whole probe. It has to cover a dial and one round trip and
/// nothing else, and it is spent with the admin watching a spinner, so it is
/// short. Sized off the connection's own two constants rather than invented.
pub const CREDENTIAL_PROBE_TIMEOUT: Duration =
    Duration::from_millis((HANDSHAKE_TIMEOUT.as_millis() + SUBSCRIBE_TIMEOUT.as_millis()) as u64);

/// Proves a `(bot_id, secret)` pair is live. Tests substitute a fake;
/// production always gets the handshake probe.
#[async_trait]
pub trait CredentialProbe: Send + Sync {
    async fn probe(
        &self,
        ctx: &CancellationToken,
        bot_id: &str,
        secret: &str,
    ) -> anyhow::Result<()>;
}

/// The production probe: one connection, one subscribe, no read loop, no
/// registration, no sender published. Nothing else in the process learns it
/// happened.
pub struct HandshakeProbe {
    dialer: Option<Arc<dyn Dialer>>,
    ws_url: String,
}

impl HandshakeProbe {
    /// Builds the probe the install service uses. `dialer` and `ws_url` are
    /// test seams; production passes None and "".
    pub fn new(dialer: Option<Arc<dyn Dialer>>, ws_url: &str) -> Self {
        let ws_url = if ws_url.is_empty() {
            DEFAULT_WS_URL.to_string()
        } else {
            ws_url.to_string()
        };
        Self { dialer, ws_url }
    }
}

#[async_trait]
impl CredentialProbe for HandshakeProbe {
    async fn probe(
        &self,
        ctx: &CancellationToken,
        bot_id: &str,
        secret: &str,
    ) -> anyhow::Result<()> {
        let dialer: Arc<dyn Dialer> = match &self.dialer {
            Some(d) => d.clone(),
            None => Arc::new(DefaultDialer::new().map_err(unverifiable)?),
        };
        let deadline = Instant::now() + CREDENTIAL_PROBE_TIMEOUT;
        let conn = dialer.dial(ctx, &self.ws_url).await.map_err(unverifiable)?;
        let result = run_handshake(&*conn, ctx, deadline, bot_id, secret).await;
        conn.close().await;
        result
    }
}

async fn run_handshake(
    conn: &dyn WsConn,
    ctx: &CancellationToken,
    deadline: Instant,
    bot_id: &str,
    secret: &str,
) -> anyhow::Result<()> {
    let req_id = new_req_id();
    let frame = json!({
        "cmd": CMD_SUBSCRIBE,
        "headers": { "req_id": req_id },
        "body": subscribe_body(bot_id, secret),
    });
    let payload = serde_json::to_string(&frame)
        .map_err(|e| unverifiable(format!("encode subscribe: {e}")))?;
    conn.write_message(payload, Some(deadline))
        .await
        .map_err(|e| unverifiable(format!("send subscribe: {e}")))?;

    // Read until our own ack comes back. The server may push other frames
    // first; anything that is not the answer to this req_id is not ours.
    loop {
        if ctx.is_cancelled() {
            return Err(unverifiable("context cancelled"));
        }
        let payload = conn
            .read_message(Some(deadline))
            .await
            .map_err(|e| unverifiable(format!("read subscribe ack: {e}")))?;
        let Ok(env) = serde_json::from_slice::<FrameEnvelope>(&payload) else {
            continue;
        };
        if env.headers.req_id != req_id {
            continue;
        }
        if env.err_code != 0 {
            return Err(classify_subscribe_ack(env.err_code, &env.err_msg));
        }
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ws_frame::FrameEnvelope;
    use crate::ws_frame::FrameHeaders;
    use std::sync::Mutex;

    struct ScriptedConn {
        responses: Mutex<Vec<Vec<u8>>>,
        written: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl WsConn for ScriptedConn {
        async fn read_message(&self, _d: Option<Instant>) -> anyhow::Result<Vec<u8>> {
            let mut q = self.responses.lock().unwrap_or_else(|e| e.into_inner());
            if q.is_empty() {
                anyhow::bail!("script exhausted");
            }
            let mut frame = q.remove(0);
            // Substitute the sentinel with the req_id the probe actually
            // wrote, so the scripted ack matches whatever random id the
            // handshake minted.
            if let Ok(mut env) = serde_json::from_slice::<FrameEnvelope>(&frame) {
                if env.headers.req_id == PROBE_REQ_SENTINEL {
                    env.headers.req_id = self.subscribe_req_id();
                    frame = serde_json::to_vec(&env).unwrap();
                }
            }
            Ok(frame)
        }
        async fn write_message(&self, data: String, _d: Option<Instant>) -> anyhow::Result<()> {
            self.written
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(data);
            Ok(())
        }
        async fn close(&self) {}
    }

    /// Placeholder req_id in a scripted ack; ScriptedConn substitutes the
    /// probe's real id at read time.
    const PROBE_REQ_SENTINEL: &str = "probe-req-id";

    impl ScriptedConn {
        /// The req_id the probe's subscribe frame carried (its write is the
        /// LAST one on this conn — the seed write predates the handshake).
        fn subscribe_req_id(&self) -> String {
            let written = self.written.lock().unwrap_or_else(|e| e.into_inner());
            let frame: serde_json::Value =
                serde_json::from_str(written.last().map(String::as_str).unwrap_or("{}"))
                    .unwrap_or_default();
            frame["headers"]["req_id"]
                .as_str()
                .unwrap_or("")
                .to_string()
        }
    }

    #[test]
    fn classify_rejects_only_the_documented_codes() {
        for code in REJECTION_ERR_CODES {
            let e = classify_subscribe_ack(code, "不合法的secret");
            assert!(is_credentials_rejected(&e), "{code}: {e}");
            assert!(!is_credentials_unverifiable(&e));
            assert!(e.to_string().contains("rejected"));
        }
        for code in [0, 45009, 45033, 99999] {
            let e = classify_subscribe_ack(code, "throttled");
            assert!(is_credentials_unverifiable(&e), "{code}: {e}");
            assert!(!is_credentials_rejected(&e));
        }
    }

    /// Builds a scripted conn whose single ack echoes the req_id the probe
    /// actually wrote (the probe mints a random one per call).
    async fn conn_with_ack(frame: FrameEnvelope) -> ScriptedConn {
        let conn = ScriptedConn {
            responses: Mutex::new(vec![]),
            written: Mutex::new(Vec::new()),
        };
        // Drive one write through the conn so subscribe_req_id() sees it.
        conn.write_message(
            serde_json::to_string(&json!({
                "cmd": "aibot_subscribe",
                "headers": {"req_id": "probe-writes-first"}
            }))
            .unwrap(),
            None,
        )
        .await
        .unwrap();
        let mut frame = frame;
        frame.headers.req_id = PROBE_REQ_SENTINEL.to_string();
        *conn.responses.lock().unwrap_or_else(|e| e.into_inner()) =
            vec![serde_json::to_vec(&FrameEnvelope { ..frame }).unwrap()];
        conn
    }

    /// A scripted conn with raw frames, for the skip test.
    async fn conn_with_frames(frames: Vec<serde_json::Value>) -> ScriptedConn {
        let conn = ScriptedConn {
            responses: Mutex::new(vec![]),
            written: Mutex::new(Vec::new()),
        };
        conn.write_message(
            serde_json::to_string(&json!({
                "cmd": "aibot_subscribe",
                "headers": {"req_id": "seed"}
            }))
            .unwrap(),
            None,
        )
        .await
        .unwrap();
        // First the foreign frames, then our ack (sentinel-substituted at
        // read time).
        let mut scripts: Vec<Vec<u8>> = frames
            .into_iter()
            .map(|v| serde_json::to_vec(&v).unwrap())
            .collect();
        scripts.push(
            serde_json::to_vec(&FrameEnvelope {
                headers: FrameHeaders {
                    req_id: PROBE_REQ_SENTINEL.to_string(),
                },
                ..Default::default()
            })
            .unwrap(),
        );
        *conn.responses.lock().unwrap_or_else(|e| e.into_inner()) = scripts;
        conn
    }

    #[tokio::test]
    async fn handshake_accepts_a_zero_ack_for_its_own_req_id() {
        let conn = conn_with_ack(FrameEnvelope {
            err_code: 0,
            ..Default::default()
        })
        .await;
        let res = run_handshake(
            &conn,
            &CancellationToken::new(),
            Instant::now() + Duration::from_secs(5),
            "bot",
            "secret",
        )
        .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn handshake_refuses_on_a_nonzero_ack() {
        let conn = conn_with_ack(FrameEnvelope {
            err_code: 40001,
            err_msg: "bad".to_string(),
            ..Default::default()
        })
        .await;
        let res = run_handshake(
            &conn,
            &CancellationToken::new(),
            Instant::now() + Duration::from_secs(5),
            "bot",
            "secret",
        )
        .await
        .unwrap_err();
        assert!(is_credentials_rejected(&res), "{res}");
    }

    #[tokio::test]
    async fn handshake_skips_frames_for_other_req_ids() {
        let conn = conn_with_frames(vec![json!({
            "cmd": "aibot_msg_callback",
            "headers": {"req_id": "someone-else"},
        })])
        .await;
        let res = run_handshake(
            &conn,
            &CancellationToken::new(),
            Instant::now() + Duration::from_secs(5),
            "bot",
            "secret",
        )
        .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn transport_failures_are_unverifiable_not_rejected() {
        let conn = ScriptedConn {
            responses: Mutex::new(vec![]),
            written: Mutex::new(Vec::new()),
        };
        let res = run_handshake(
            &conn,
            &CancellationToken::new(),
            Instant::now() + Duration::from_secs(5),
            "bot",
            "secret",
        )
        .await
        .unwrap_err();
        assert!(is_credentials_unverifiable(&res), "{res}");
        assert!(!is_credentials_rejected(&res));
    }
}
