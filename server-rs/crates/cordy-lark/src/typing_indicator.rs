//! The "processing" reaction lifecycle — port of
//! `server/internal/integrations/lark/typing_indicator.go`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::client::{AddReactionParams, ApiClient, DeleteReactionParams, InstallationCredentials};
use crate::installation::CredentialsResolver;
use crate::resolvers::InstallationLookup;
use crate::store::Installation;
use crate::types::{region_or_default, Region};

/// The Lark emoji_type used for the "processing" indicator. It renders as a
/// small typing-animation badge on the message.
const TYPING_EMOJI: &str = "Typing";

/// How old a message can be before we skip the typing indicator. This prevents
/// stale reactions when a WebSocket reconnect replays old events. Aligned with
/// OpenClaw's 2-minute bound.
const TYPING_INDICATOR_MAX_AGE: Duration = Duration::from_secs(2 * 60);

/// Holds the identifiers needed to remove a reaction, plus the installation
/// whose app credentials added it. The installation id is recorded at add time
/// because that is the last moment it is certainly resolvable: it is reachable
/// from the session's channel_chat_session_binding row, and a session delete
/// drops that row while the cancel it triggers is still on its way to the
/// Patcher.
///
/// `install_snapshot` is the installation row as it stood when the reaction was
/// added, kept for the one case where the id is no longer enough: a runtime
/// teardown deletes the installation inside the same transaction that cancels
/// the tasks (handler/runtime.go, DeleteChannelInstallationsBySystemRuntimeAgents),
/// so by the time the cancel reaches Clear there is no row to resolve.
///
/// It is a FALLBACK, never the primary. A live lookup picks up a credential
/// rotation between add and clear; a snapshot cannot, so it is consulted only
/// when the row is genuinely gone.
///
/// It does not weaken "no decrypted secret lives in the state map": what is
/// held here is the same encrypted blob the database holds, and
/// DecryptAppSecret still runs at clear time.
#[derive(Clone)]
struct TypingIndicatorState {
    message_id: String,
    reaction_id: String,
    installation_id: Uuid,
    install_snapshot: Installation,
}

/// Owns the "processing" reaction lifecycle for inbound Lark messages. When a
/// message is successfully ingested it adds a Typing reaction; when the run
/// ends — with a reply, a failure or a cancellation — it clears the
/// reaction(s) for that chat session.
///
/// The manager is safe for concurrent use. It tolerates missing or stale state
/// gracefully: adding a reaction to a message that already has one simply
/// appends another state entry; clearing a session with no tracked state is a
/// no-op.
pub struct TypingIndicatorManager {
    client: Arc<dyn ApiClient>,
    credentials: Arc<dyn CredentialsResolver>,
    queries: Arc<dyn InstallationLookup>,

    states: RwLock<HashMap<String, Vec<TypingIndicatorState>>>, // key = chat_session_id string
}

impl TypingIndicatorManager {
    /// Constructs a manager over its shared dependencies.
    pub fn new(
        client: Arc<dyn ApiClient>,
        credentials: Arc<dyn CredentialsResolver>,
        queries: Arc<dyn InstallationLookup>,
    ) -> Self {
        Self {
            client,
            credentials,
            queries,
            states: RwLock::new(HashMap::new()),
        }
    }

    /// Sends a Typing reaction to the given message and records the state
    /// under the chat session. Errors are logged and swallowed.
    ///
    /// `create_time` is Lark's epoch-millisecond string
    /// (InboundMessage.CreateTime). Messages older than
    /// TYPING_INDICATOR_MAX_AGE are silently skipped so that WebSocket
    /// replays and stale reconnects do not surface misleading "processing"
    /// badges on long-finished conversations.
    pub async fn add(
        &self,
        _ctx: CancellationToken,
        inst: &Installation,
        chat_session_id: Uuid,
        message_id: &str,
        create_time: &str,
    ) {
        if message_id.is_empty() {
            return;
        }
        if is_message_too_old(create_time) {
            tracing::debug!(
                chat_session_id = %chat_session_id,
                message_id = %message_id,
                create_time = %create_time,
                "lark typing indicator: message too old, skipping"
            );
            return;
        }
        let creds = match self.resolve_credentials(inst) {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!(
                    chat_session_id = %chat_session_id,
                    message_id = %message_id,
                    error = %err,
                    "lark typing indicator: failed to resolve credentials"
                );
                return;
            }
        };

        let reaction_id = match self
            .client
            .add_message_reaction(AddReactionParams {
                installation_id: creds,
                message_id: message_id.to_string(),
                emoji_type: TYPING_EMOJI.to_string(),
            })
            .await
        {
            Ok(id) => id,
            Err(err) => {
                tracing::warn!(
                    chat_session_id = %chat_session_id,
                    message_id = %message_id,
                    error = %err,
                    "lark typing indicator: add reaction failed"
                );
                return;
            }
        };

        let key = chat_session_id.to_string();
        if let Ok(mut map) = self.states.write() {
            map.entry(key.clone())
                .or_default()
                .push(TypingIndicatorState {
                    message_id: message_id.to_string(),
                    reaction_id: reaction_id.clone(),
                    installation_id: inst.id,
                    install_snapshot: inst.clone(),
                });
        }

        tracing::debug!(
            chat_session_id = %key,
            message_id = %message_id,
            reaction_id = %reaction_id,
            "lark typing indicator: reaction added"
        );
    }

    /// Removes every tracked Typing reaction for the chat session and drops
    /// the state entry, so the reaction is gone before the agent's reply is
    /// sent — a clean visual transition. Individual delete failures are logged
    /// but do not abort the loop.
    ///
    /// Credentials come from the installation each state recorded, not from
    /// the session's binding, because a clear can outlive that binding:
    /// deleting a chat session drops the binding row inside the same
    /// transaction that cancels the session's tasks, and the task:cancelled
    /// events that reach the Patcher are broadcast after that transaction
    /// commits. A binding lookup would miss, and since the state has already
    /// been taken here, there would be nothing left to clear from.
    /// Installation rows survive a session delete.
    ///
    /// They do NOT survive a runtime teardown, which deletes them in the same
    /// transaction — so each state also carries the installation as it stood
    /// at add time, consulted only when the row is gone. See
    /// [`TypingIndicatorState`].
    pub async fn clear(&self, _ctx: CancellationToken, chat_session_id: Uuid) {
        let key = chat_session_id.to_string();
        let states = match self.states.write() {
            Ok(mut map) => map.remove(&key).unwrap_or_default(),
            Err(_) => return,
        };
        if states.is_empty() {
            return;
        }

        // One session's reactions normally share an installation, so the
        // resolved credentials are memoised; a session rebound to another
        // installation mid-run still clears every reaction through the app
        // that added it. A None entry records an installation that failed to
        // resolve, so it is not retried once per reaction.
        let mut resolved: HashMap<String, Option<InstallationCredentials>> = HashMap::new();
        for s in &states {
            if s.reaction_id.is_empty() {
                continue;
            }
            let inst_key = s.installation_id.to_string();
            let creds = match resolved.get(&inst_key) {
                Some(cached) => cached.clone(),
                None => {
                    let creds = self
                        .credentials_for_installation(&key, s.installation_id, &s.install_snapshot)
                        .await;
                    resolved.insert(inst_key, creds.clone());
                    creds
                }
            };
            let Some(creds) = creds else { continue };
            if let Err(err) = self
                .client
                .delete_message_reaction(DeleteReactionParams {
                    installation_id: creds,
                    message_id: s.message_id.clone(),
                    reaction_id: s.reaction_id.clone(),
                })
                .await
            {
                tracing::warn!(
                    chat_session_id = %key,
                    message_id = %s.message_id,
                    reaction_id = %s.reaction_id,
                    error = %err,
                    "lark typing indicator: delete reaction failed"
                );
                continue;
            }
            tracing::debug!(
                chat_session_id = %key,
                message_id = %s.message_id,
                reaction_id = %s.reaction_id,
                "lark typing indicator: reaction removed"
            );
        }
    }

    /// Loads an installation row and decrypts its app secret. None means the
    /// clear cannot proceed for that installation; the reason is already
    /// logged. The decrypted secret exists only from here on, never in the
    /// state map.
    async fn credentials_for_installation(
        &self,
        session_key: &str,
        id: Uuid,
        snapshot: &Installation,
    ) -> Option<InstallationCredentials> {
        match self.queries.get_lark_installation(id).await {
            Ok(inst) => self.resolve_credentials(&inst).ok().or_else(|| {
                tracing::warn!(
                    chat_session_id = %session_key,
                    installation_id = %id,
                    "lark typing indicator: failed to resolve credentials for clear"
                );
                None
            }),
            // The row is gone, which on this path means the runtime teardown
            // deleted it in the transaction that cancelled these tasks. The
            // reaction it added is still on the message and this is the only
            // thing left that can take it off.
            Err(_) => {
                tracing::debug!(
                    chat_session_id = %session_key,
                    installation_id = %id,
                    "lark typing indicator: installation gone, clearing from the snapshot taken at add time"
                );
                self.resolve_credentials(snapshot).ok().or_else(|| {
                    tracing::warn!(
                        chat_session_id = %session_key,
                        installation_id = %id,
                        "lark typing indicator: failed to resolve credentials for clear"
                    );
                    None
                })
            }
        }
    }

    fn resolve_credentials(&self, inst: &Installation) -> anyhow::Result<InstallationCredentials> {
        let secret = self.credentials.decrypt_app_secret(inst)?;
        let mut creds = InstallationCredentials {
            app_id: inst.app_id.clone(),
            app_secret: secret,
            tenant_key: String::new(),
            region: region_or_default(&inst.region),
        };
        if let Some(tk) = &inst.tenant_key {
            creds.tenant_key = tk.clone();
        }
        Ok(creds)
    }
}

fn is_message_too_old(create_time: &str) -> bool {
    if create_time.is_empty() {
        return false;
    }
    let Ok(ms) = create_time.parse::<i64>() else {
        return false;
    };
    let t = SystemTime::UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64);
    match SystemTime::now().duration_since(t) {
        Ok(age) => age > TYPING_INDICATOR_MAX_AGE,
        Err(_) => false, // clock skew / future timestamp → not stale
    }
}

// Keeps the region import referenced when only the default branch compiles.
#[allow(unused)]
fn _region_marker(r: Region) -> &'static str {
    r.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_messages_are_detected_and_garbage_is_not() {
        assert!(!is_message_too_old(""));
        assert!(!is_message_too_old("not-a-number"));
        // Far past → stale.
        assert!(is_message_too_old("1000"));
        // Future timestamp → not stale (clock-skew tolerant).
        let future_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            + 60_000;
        assert!(!is_message_too_old(&future_ms.to_string()));
        // Recent timestamp (now - 10s) → not stale.
        let recent_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            - 10_000;
        assert!(!is_message_too_old(&recent_ms.to_string()));
    }

    #[test]
    fn max_age_is_two_minutes() {
        assert_eq!(TYPING_INDICATOR_MAX_AGE, Duration::from_secs(120));
    }
}
