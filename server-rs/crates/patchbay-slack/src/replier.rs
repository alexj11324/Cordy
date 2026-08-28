//! The Slack OutboundReplier — the engine seam that delivers a verdict-driven
//! reply back to the user (PB-3666). Port of
//!
//! Posts through the same bot-token send path as the chat:done outbound
//! subscriber, so it needs no new transport.
//!
//! Outcomes handled:
//!   - NeedsBinding: the sender is unbound. Mint a single-use binding token and
//!     reply with a "link your account" prompt pointing at the in-product
//!     redeem page. After they bind, their next message reaches the agent.
//!   - AgentOffline / AgentArchived: a status notice so the user is not left
//!     wondering why nothing happened.
//!   - Ingested with an /issue created: a confirmation of the new issue.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use patchbay_channel::{InboundMessage, OutboundMessage};
use patchbay_channel_engine::resolvers::{Outcome, ResolvedInstallation, Result as EngineResult};

use crate::binding::BindingTokenService;
use crate::channel::SlackSender;
use crate::config::{decode_credentials, Decrypter};
use crate::resolvers::{install_team_id, ORIGIN_SLACK_CHAT};

const AGENT_OFFLINE_TEXT: &str = "⚠️ The agent is offline right now. Your message was received and will be handled once it's back online.";
const AGENT_ARCHIVED_TEXT: &str =
    "⚠️ This agent has been archived and can't respond. Please contact your workspace admin.";
const FRESH_PENDING_TEXT: &str =
    "✅ Fresh start ready. Your next chat message will run without previous context.";
const ISSUE_USAGE_TEXT: &str =
    "Please include an issue title. Use:\n\n`/issue <title>`\n`[description]` (optional)";

/// Configures the replier. Binding + AppURL are required for the NeedsBinding
/// prompt to work; without them the prompt is skipped (the offline/archived/
/// issue notices still fire).
pub struct OutboundReplierConfig {
    pub pool: PgPool,
    pub decrypt: Option<Arc<Decrypter>>,
    /// The Patchbay web app host the user clicks into to redeem the binding token
    /// (e.g. https://patchbay.example). It comes from PATCHBAY_APP_URL (falling back
    /// to FRONTEND_ORIGIN) and is intentionally separate from PATCHBAY_PUBLIC_URL,
    /// which is the backend/API public URL used for webhook and daemon-facing
    /// endpoints — the bind page (/slack/bind) is served by the web app, so the
    /// link must point at the app host, not the API host. Mirrors the Lark
    /// replier's AppURL.
    pub app_url: String,
    /// Default "/slack/bind".
    pub binding_path: String,
}

/// Implements `engine.OutboundReplier` for Slack.
pub struct OutboundReplier {
    binding: BindingTokenService,
    decrypt: Option<Arc<Decrypter>>,
    app_url: String,
    binding_path: String,
}

impl OutboundReplier {
    /// Builds the replier. The sender factory mirrors the outbound subscriber:
    /// only the bot token is needed to post.
    pub fn new(cfg: OutboundReplierConfig) -> Self {
        let mut binding_path = cfg.binding_path;
        if binding_path.is_empty() {
            binding_path = "/slack/bind".to_string();
        }
        if !binding_path.starts_with('/') {
            binding_path = format!("/{binding_path}");
        }
        Self {
            binding: BindingTokenService::new(cfg.pool),
            decrypt: cfg.decrypt,
            app_url: cfg.app_url.trim_end_matches('/').to_string(),
            binding_path,
        }
    }

    /// Resolves the installation's bot token from the carried platform row and
    /// sends text back into the originating channel / thread.
    async fn post(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        text: &str,
    ) -> anyhow::Result<()> {
        let Some(row) = inst
            .platform
            .downcast_ref::<patchbay_db::models::ChannelInstallation>()
        else {
            anyhow::bail!("installation platform row unavailable");
        };
        let creds = decode_credentials(&row.config, self.decrypt.as_deref())
            .map_err(|e| anyhow::anyhow!("decode credentials: {e}"))?;
        SlackSender::new(&creds.bot_token)
            .send(
                ctx,
                OutboundMessage {
                    chat_id: msg.source.chat_id.clone(),
                    text: text.to_string(),
                    thread_id: msg.source.thread_id.clone(),
                    reply_to: String::new(),
                },
            )
            .await
            .map_err(|e| anyhow::anyhow!("post slack reply: {e}"))?;
        Ok(())
    }

    async fn send_binding_prompt(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        res: &EngineResult,
    ) -> anyhow::Result<()> {
        let mut sender = res.sender.clone();
        if sender.is_empty() {
            sender = msg.source.sender_id.clone();
        }
        if sender.is_empty() {
            anyhow::bail!("missing sender id");
        }
        if self.app_url.is_empty() {
            anyhow::bail!("app url not configured");
        }
        let token = self
            .binding
            .mint(inst.workspace_id, inst.id, &sender)
            .await
            .map_err(|e| anyhow::anyhow!("mint binding token: {e:#}"))?;
        let bind_url = format!(
            "{}{}?token={}",
            self.app_url,
            self.binding_path,
            query_escape(&token.raw)
        );
        // Wrap the URL as an explicit Slack link <url|label>: formatMrkdwn
        // protects these from its markdown passes, so the base64url token's
        // `_`/`-` chars are not mangled into italics.
        let text = format!(
            "👋 To start chatting with me, link your Slack account to Patchbay: <{bind_url}|link your account>\n(This link expires in 15 minutes.)"
        );
        self.post(ctx, inst, msg, &text).await
    }
}

/// Percent-encodes a query value the way Go's url.QueryEscape does (space
/// becomes '+').
pub(crate) fn query_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[async_trait]
impl patchbay_channel_engine::resolvers::OutboundReplier for OutboundReplier {
    /// Routes each outcome to its user-visible message. Errors are logged, not
    /// propagated: the replier runs detached from the inbound ACK path.
    async fn reply(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        res: &EngineResult,
    ) {
        let Some(outcome) = &res.outcome else {
            return;
        };
        let result = match outcome {
            o if *o == Outcome::needs_binding() => self
                .send_binding_prompt(ctx, inst, msg, res)
                .await
                .map_err(|e| ("binding prompt", e)),
            o if *o == Outcome::agent_offline() => self
                .post(ctx, inst, msg, AGENT_OFFLINE_TEXT)
                .await
                .map_err(|e| ("offline notice", e)),
            o if *o == Outcome::agent_archived() => self
                .post(ctx, inst, msg, AGENT_ARCHIVED_TEXT)
                .await
                .map_err(|e| ("archived notice", e)),
            o if *o == Outcome::fresh_pending() => self
                .post(ctx, inst, msg, FRESH_PENDING_TEXT)
                .await
                .map_err(|e| ("fresh-start confirmation", e)),
            o if *o == Outcome::issue_usage() => self
                .post(ctx, inst, msg, ISSUE_USAGE_TEXT)
                .await
                .map_err(|e| ("issue usage reply", e)),
            o if *o == Outcome::ingested() => {
                // Only an /issue product result warrants an immediate reply; a
                // plain chat message stays silent (the agent's own reply lands
                // via ChatDone).
                if res.issue_id.is_some_and(|id| !id.is_nil()) {
                    let mut text = issue_created_text(res);
                    if res.issue_duplicate {
                        text = issue_duplicate_text(res);
                    }
                    self.post(ctx, inst, msg, &text)
                        .await
                        .map_err(|e| ("issue outcome reply", e))
                } else {
                    Ok(())
                }
            }
            o if *o == Outcome::dropped() => Ok(()),
            _ => Ok(()),
        };
        if let Err((what, err)) = result {
            tracing::warn!(
                installation_id = %inst.id,
                error = %err,
                "slack replier: {what} failed"
            );
        }
    }
}

pub fn issue_created_text(res: &EngineResult) -> String {
    let id = issue_result_identifier(res);
    let title = member_issue_title(res.issue_title.trim());
    if title.is_empty() {
        return format!("✅ Created {id}");
    }
    format!("✅ Created {id} — {title}")
}

pub fn issue_duplicate_text(res: &EngineResult) -> String {
    let id = issue_result_identifier(res);
    let title = member_issue_title(res.issue_title.trim());
    if title.is_empty() {
        return format!("⚠️ Not created — active issue {id} already exists.");
    }
    format!("⚠️ Not created — active issue {id} already exists: {title}")
}

fn member_issue_title(title: &str) -> String {
    let title = patchbay_channel::break_markdown_link_adjacency(title);
    // formatMrkdwn deliberately preserves existing Slack entities such as
    // <url|label> and <@user>. Encode their opening delimiter before that pass
    // so member-authored links and mentions are handled as visible text.
    title.replace('<', "&lt;")
}

fn issue_result_identifier(res: &EngineResult) -> String {
    if !res.issue_identifier.is_empty() {
        return res.issue_identifier.clone();
    }
    format!("#{}", res.issue_number)
}

// Referenced so wiring modules can resolve origin labels next to the replier
// without importing resolvers directly.
#[allow(dead_code)]
fn _origin_label() -> &'static str {
    ORIGIN_SLACK_CHAT
}

// Keeps install_team_id linked for the slash-command module's shared use.
#[allow(dead_code)]
fn _team_probe(v: &serde_json::Value) -> String {
    install_team_id(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_texts_render_identifier_and_title() {
        let mut res = EngineResult {
            issue_number: 41,
            ..Default::default()
        };
        assert_eq!(issue_created_text(&res), "✅ Created #41");

        res.issue_identifier = "CORD-12".into();
        assert_eq!(issue_created_text(&res), "✅ Created CORD-12");

        res.issue_title = "Fix **login** <https://x.io|docs>".into();
        assert_eq!(
            issue_created_text(&res),
            "✅ Created CORD-12 — Fix **login** &lt;https://x.io|docs>"
        );

        res.issue_duplicate = true;
        assert_eq!(
            issue_duplicate_text(&res),
            "⚠️ Not created — active issue CORD-12 already exists: Fix **login** &lt;https://x.io|docs>"
        );

        // Empty title falls back to the bare form on both paths.
        res.issue_title = "  ".into();
        res.issue_duplicate = false;
        assert_eq!(issue_created_text(&res), "✅ Created CORD-12");
        res.issue_duplicate = true;
        assert_eq!(
            issue_duplicate_text(&res),
            "⚠️ Not created — active issue CORD-12 already exists."
        );
    }

    #[test]
    fn query_escape_matches_go_url_queryescape() {
        assert_eq!(query_escape("abc123"), "abc123");
        assert_eq!(query_escape("a b+c"), "a+b%2Bc");
        assert_eq!(query_escape("a/b"), "a%2Fb");
        // Base64url alphabet passes through untouched.
        assert_eq!(query_escape("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(query_escape("é"), "%C3%A9");
    }
}
