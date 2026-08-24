//! The "processing" reaction lifecycle for inbound Slack messages. Port of
//! `server/internal/integrations/slack/typing_indicator.go`.
//!
//! Adds a 👀 reaction when a message is ingested and removes it however the
//! agent's run ends — chat:done, task:failed or task:cancelled. State is held
//! in memory keyed by chat_session_id, the bot token is re-resolved from the DB
//! on clear (only the installation id is held in the map between add and clear,
//! never the token), and every failure is logged and swallowed — the indicator
//! is best-effort and must never block or fail a real reply.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use cordy_channel::RuntimeTasks;
use cordy_db::models::ChannelInstallation;
use cordy_db::queries::channel::get_channel_installation;

use crate::client::SlackClient;
use crate::config::{decode_credentials, DecrypterArc};
use crate::TYPE_SLACK;

/// The Slack reaction name used as the "processing" indicator on the user's
/// message while the agent is working. Slack has no animated "typing" reaction
/// like Feishu's, so we use the universal 👀 ("seen, on it") convention — a
/// built-in emoji present in every workspace. Change this one constant to swap
/// the indicator. The installed Slack app needs the reactions:write scope for
/// the reaction to land; without it the add simply fails and is logged.
const TYPING_EMOJI: &str = "eyes";

/// Bounds how old an inbound message may be before we skip the reaction, so a
/// Socket Mode reconnect that replays old events does not stamp "processing"
/// badges onto long-finished conversations. Mirrors Feishu.
const TYPING_INDICATOR_MAX_AGE: Duration = Duration::from_secs(120);

/// What removing a reaction needs: the (channel, message ts) pair Slack
/// addresses the item by — it removes by emoji name + item ref, so there is no
/// reaction id to store — plus the installation whose bot token put the
/// reaction there. The installation id is recorded at add time because that is
/// the last moment it is certainly resolvable: it is reachable from the
/// session's channel_chat_session_binding, and a session delete drops that row
/// while the cancel it triggers is still on its way to this manager.
///
/// `config_snapshot` is the installation's encrypted config as it stood when
/// the reaction was added, for the one case where the id is no longer enough: a
/// runtime teardown deletes the installation inside the same transaction that
/// cancels the tasks, so by the time the cancel reaches clear there is no row
/// to resolve.
///
/// A FALLBACK, never the primary — a live lookup picks up a credential rotation
/// between add and clear and a snapshot cannot, so it is used only when the row
/// is gone. It holds the same encrypted blob the database holds; the bot token
/// is still decrypted only for the life of the clear.
#[derive(Clone)]
struct TypingState {
    channel_id: String,
    message_ts: String,
    installation_id: Uuid,
    config_snapshot: serde_json::Value,
}

/// Owns the 👀 reaction lifecycle. One instance per process; register it
/// against the bus once at boot.
pub struct TypingIndicatorManager {
    pool: PgPool,
    states: Mutex<HashMap<String, Vec<TypingState>>>, // key = chat_session_id string
}

impl TypingIndicatorManager {
    /// Builds a manager over the pool. The Slack API client is constructed per
    /// call from the installation's decrypted bot token (`xoxb-`), exactly like
    /// the outbound sender.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            states: Mutex::new(HashMap::new()),
        }
    }

    /// Reacts to the just-ingested message and records the state under the
    /// chat session. inst is the resolved installation row whose config blob
    /// carries the encrypted bot token. It runs detached from the Router ACK
    /// path; errors are logged and swallowed.
    pub async fn add(
        &self,
        _ctx: CancellationToken,
        inst: &ChannelInstallation,
        decrypt: Option<&DecrypterArc>,
        session_id: Uuid,
        channel_id: &str,
        message_ts: &str,
    ) {
        if channel_id.is_empty() || message_ts.is_empty() {
            return;
        }
        if is_message_too_old(message_ts) {
            tracing::debug!(
                chat_session_id = %session_id,
                message_ts = %message_ts,
                "slack typing indicator: message too old, skipping"
            );
            return;
        }
        let creds = match decode_credentials(&inst.config, decrypt.map(|d| d.as_ref())) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    chat_session_id = %session_id,
                    error = %e,
                    "slack typing indicator: decode credentials failed"
                );
                return;
            }
        };
        let api = SlackClient::new(&creds.bot_token);
        if let Err(e) = api
            .reactions_add(TYPING_EMOJI, channel_id, message_ts)
            .await
        {
            tracing::warn!(
                chat_session_id = %session_id,
                message_ts = %message_ts,
                error = %e,
                "slack typing indicator: add reaction failed"
            );
            return;
        }
        let key = session_id.to_string();
        self.states
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key)
            .or_default()
            .push(TypingState {
                channel_id: channel_id.to_string(),
                message_ts: message_ts.to_string(),
                installation_id: inst.id,
                config_snapshot: inst.config.clone(),
            });
    }

    /// Removes every tracked reaction for the chat session and drops the state.
    /// It re-resolves the bot token from the installation each state recorded,
    /// so no decrypted token is held in memory between add and clear.
    /// Individual failures are logged but do not abort the loop. Best-effort
    /// throughout.
    ///
    /// The installation is read straight from the state rather than looked up
    /// through the session's binding, because a clear can outlive that binding:
    /// deleting a chat session drops the binding row inside the same
    /// transaction that cancels the session's tasks, and the task:cancelled
    /// events that reach this manager are broadcast after that transaction
    /// commits. A binding lookup would miss, and since the state has already
    /// been taken here, there would be nothing left to clear from.
    /// Installation rows survive the session.
    pub async fn clear(
        &self,
        ctx: CancellationToken,
        session_id: Uuid,
        decrypt: Option<&DecrypterArc>,
    ) {
        let key = session_id.to_string();
        let states = self
            .states
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key)
            .unwrap_or_default();
        if states.is_empty() {
            return;
        }

        // One session's reactions normally share an installation, so the
        // resolved clients are memoised; a session rebound to another
        // installation mid-run still clears every reaction through the app that
        // added it. A None entry records an installation that failed to
        // resolve, so it is not retried once per reaction.
        let mut apis: HashMap<String, Option<SlackClient>> = HashMap::with_capacity(1);
        for s in states {
            let inst_key = s.installation_id.to_string();
            let api = if let Some(cached) = apis.get(&inst_key) {
                cached.clone()
            } else {
                let resolved = self
                    .api_for_installation(
                        ctx.clone(),
                        s.installation_id,
                        &s.config_snapshot,
                        decrypt,
                    )
                    .await;
                if let Err(e) = &resolved {
                    tracing::warn!(
                        chat_session_id = %key,
                        installation_id = %inst_key,
                        error = %e,
                        "slack typing indicator: resolve installation for clear failed"
                    );
                }
                let client = resolved.ok();
                apis.insert(inst_key, client.clone());
                client
            };
            let Some(api) = api else { continue };
            if ctx.is_cancelled() {
                return;
            }
            if let Err(e) = api
                .reactions_remove(TYPING_EMOJI, &s.channel_id, &s.message_ts)
                .await
            {
                tracing::warn!(
                    chat_session_id = %key,
                    message_ts = %s.message_ts,
                    error = %e,
                    "slack typing indicator: remove reaction failed"
                );
            }
        }
    }

    /// Subscribes the manager to every task-lifecycle event that ends a run,
    /// so the reaction comes off however the run finished. The outbound reply
    /// subscriber only handles chat:done, so this is the only path that removes
    /// the reaction on the other two endings.
    ///
    /// task:cancelled has to be here or a cancelled run leaves the 👀 on the
    /// user's message for good: a cancellation publishes no chat-done and no
    /// task-failed, so nothing else would ever take the reaction off.
    ///
    /// Two holes are left, and neither is a missing subscription:
    ///
    /// Archiving an agent cancels its tasks without broadcasting per row, on
    /// the grounds that the agent:archived event already invalidates every
    /// client's task list. No client-side list refresh takes a reaction off a
    /// Slack message, so archiving an agent mid-run leaves the 👀 in place.
    ///
    /// And an ending that arrives while the reaction is still being added
    /// clears nothing: add records its state only after the Slack call returns,
    /// so clear finds an empty map, and the reaction lands after it with nothing
    /// left to take it off. The Router adds on a detached task, so a cancelled
    /// or very fast run gets there first.
    ///
    /// Call once at boot against a fresh bus; register it before the outbound
    /// subscriber so the reaction clears ahead of the reply on chat:done (bus
    /// delivery is synchronous, in subscription order).
    pub fn register(
        self: &Arc<Self>,
        bus: &cordy_events::Bus,
        decrypt: Option<DecrypterArc>,
        tasks: Arc<cordy_channel::RuntimeTasks>,
    ) {
        let subscribe = {
            let me = Arc::clone(self);
            let decrypt = decrypt.clone();
            let tasks = tasks.clone();
            move |bus: &cordy_events::Bus, event_type: &str| {
                let me = Arc::clone(&me);
                let decrypt = decrypt.clone();
                let tasks = tasks.clone();
                bus.subscribe(event_type, move |e: &cordy_events::Event| {
                    me.handle_event(e, decrypt.as_ref(), &tasks);
                });
            }
        };
        subscribe(bus, cordy_protocol::EVENT_CHAT_DONE);
        subscribe(bus, cordy_protocol::EVENT_TASK_FAILED);
        subscribe(bus, cordy_protocol::EVENT_TASK_CANCELLED);
    }

    fn handle_event(
        self: &Arc<Self>,
        e: &cordy_events::Event,
        decrypt: Option<&DecrypterArc>,
        tasks: &RuntimeTasks,
    ) {
        // Issue / autopilot tasks carry no chat_session — nothing to clear.
        let Some(session_id) = chat_session_id_from_event(e) else {
            return;
        };
        // Bus delivery is synchronous; bound the reaction calls so a stuck
        // Slack HTTP request cannot wedge the publish call site.
        let me = Arc::clone(self);
        let decrypt = decrypt.cloned();
        tasks.spawn(async move {
            let ctx = tokio_util::sync::CancellationToken::new();
            if tokio::time::timeout(
                Duration::from_secs(10),
                me.clear(ctx, session_id, decrypt.as_ref()),
            )
            .await
            .is_err()
            {
                tracing::warn!(%session_id, "slack typing indicator clear timed out");
            }
        });
    }

    /// Loads an installation row and turns its encrypted config into a
    /// reaction client. The decrypted bot token exists only for the life of
    /// this call.
    async fn api_for_installation(
        &self,
        _ctx: CancellationToken,
        id: Uuid,
        snapshot: &serde_json::Value,
        decrypt: Option<&DecrypterArc>,
    ) -> anyhow::Result<SlackClient> {
        let mut config = snapshot.clone();
        match get_channel_installation(&self.pool, id, TYPE_SLACK).await {
            Ok(Some(inst)) => config = inst.config,
            Ok(None) => {
                if snapshot.is_null() {
                    anyhow::bail!("lookup installation: not found");
                }
                // The row is gone, which on this path means the runtime
                // teardown deleted it in the transaction that cancelled these
                // tasks. The reaction it added is still on the message and the
                // snapshot is the only thing left that can take it off.
            }
            Err(e) => anyhow::bail!("lookup installation: {e:#}"),
        }
        let creds = decode_credentials(&config, decrypt.map(|d| d.as_ref()))
            .map_err(|e| anyhow::anyhow!("decode credentials: {e}"))?;
        Ok(SlackClient::new(creds.bot_token))
    }
}

/// Recovers the chat session id from a task-lifecycle event. chat:done sets it
/// on the envelope; task:failed carries it only in the broadcast payload map
/// (chat tasks only), so both are checked. Every task:cancelled publisher sets
/// both.
fn chat_session_id_from_event(e: &cordy_events::Event) -> Option<Uuid> {
    if !e.chat_session_id.is_empty() {
        if let Ok(id) = Uuid::parse_str(&e.chat_session_id) {
            if !id.is_nil() {
                return Some(id);
            }
        }
    }
    let s = e.payload.get("chat_session_id")?.as_str()?;
    Uuid::parse_str(s).ok().filter(|id| !id.is_nil())
}

/// Reports whether a Slack message ts ("<seconds>.<micros>") is older than
/// TYPING_INDICATOR_MAX_AGE. A malformed or empty ts is treated as fresh (not
/// skipped) — we would rather over-react than drop a real message.
fn is_message_too_old(ts: &str) -> bool {
    if ts.is_empty() {
        return false;
    }
    let Ok(secs) = ts.parse::<f64>() else {
        return false;
    };
    match chrono::DateTime::from_timestamp(secs as i64, 0) {
        Some(t) => {
            let age = chrono::Utc::now().signed_duration_since(t);
            age > chrono::Duration::from_std(TYPING_INDICATOR_MAX_AGE).unwrap_or_default()
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_from_envelope_then_payload() {
        let id = Uuid::now_v7();
        let mk = |session: &str, payload: serde_json::Value| cordy_events::Event {
            event_type: String::new(),
            workspace_id: String::new(),
            actor_type: String::new(),
            actor_id: String::new(),
            payload,
            task_id: String::new(),
            chat_session_id: session.to_string(),
        };

        assert_eq!(
            chat_session_id_from_event(&mk(&id.to_string(), serde_json::json!({}))),
            Some(id)
        );
        assert_eq!(
            chat_session_id_from_event(&mk(
                "",
                serde_json::json!({"chat_session_id": id.to_string()})
            )),
            Some(id)
        );
        // Nil / malformed ids read as absent.
        assert_eq!(
            chat_session_id_from_event(&mk(&Uuid::nil().to_string(), serde_json::json!({}))),
            None
        );
        assert_eq!(
            chat_session_id_from_event(&mk("junk", serde_json::json!({}))),
            None
        );
        assert_eq!(
            chat_session_id_from_event(&mk("", serde_json::json!({}))),
            None
        );
    }

    #[test]
    fn old_messages_are_skipped_but_malformed_ones_pass() {
        assert!(!is_message_too_old("")); // empty → fresh
        assert!(!is_message_too_old("junk")); // malformed → fresh
                                              // Fresh timestamp (now).
        let now = chrono::Utc::now().timestamp();
        assert!(!is_message_too_old(&format!("{now}.000001")));
        // Three minutes ago exceeds the 2-minute window.
        let old = chrono::Utc::now().timestamp() - 180;
        assert!(is_message_too_old(&format!("{old}.000001")));
    }
}
