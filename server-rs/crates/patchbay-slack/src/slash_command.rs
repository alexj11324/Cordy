//! The Slack `/issue` SLASH COMMAND. Port of
//!
//! Deliberately separate from the message-based `/issue` (engine
//! ParseIssueCommand): on Slack a message whose first character is `/` is
//! intercepted by the client as a slash command and never delivered to the app,
//! so the message-prefix form of `/issue` cannot work here at all (PB-3908).
//! Registering `/issue` as a real slash command in the app manifest is what
//! makes it reach us — as a slash_commands envelope over the same Socket Mode
//! connection.
//!
//! The command is a QUICK-CREATE entry point: it does NOT create the issue
//! itself. It takes the invoker's natural-language description as a prompt and
//! enqueues a quick-create task against the selected workspace Agent — the
//! very same pipeline as the web "quick create" modal
//! (TaskService::enqueue_quick_create_task). The agent turns the prompt into a
//! well-formed `patchbay issue create` in the background, so the issue gets a
//! proper title + structured description instead of the raw one-liner the user
//! typed. Because creation is asynchronous, the command replies with a PRIVATE
//! (ephemeral) acknowledgement via the command's response_url — there is no
//! issue number to hand back yet — and the agent's completion surfaces to the
//! invoker as a Patchbay inbox notification through the shared quick-create
//! completion path. It starts no chat session / chat run.
//!
//! The installation routing and identity + membership checks mirror the message
//! path (resolvers.rs) so a slash-command quick-create respects the same
//! workspace boundary and account binding as every other Slack entry point;
//! they are kept local so the proven inbound pipeline is untouched.

use std::sync::Arc;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use patchbay_channel_engine::hub::PostgresHubRouter;
use patchbay_channel_engine::resolvers::{ResolvedInstallation, ResolverError};
use patchbay_db::queries::channel::get_channel_installation_by_app_id;
use patchbay_db::queries::member::get_member_by_user_and_workspace;

use crate::binding::BindingTokenService;
use crate::resolvers::installation_serves_team;
use crate::TYPE_SLACK;

const ISSUE_SLASH_COMMAND: &str = "/issue";

// User-facing ephemeral replies. Kept terse; only the invoker sees them.
const SLASH_USAGE_TEXT: &str =
    "Tell me what to file, e.g. `/issue the login button does nothing on Safari`.";
const SLASH_QUEUED_TEXT: &str =
    "✅ On it — I'm turning that into an issue. You'll get a Patchbay notification when it's ready.";
const SLASH_NOT_MEMBER_TEXT: &str =
    "You're not a member of this Patchbay workspace, so I can't file an issue for you.";
const SLASH_LINK_ACCOUNT_FALLBACK: &str =
    "Link your Slack account to Patchbay first, then try `/issue` again.";
const SLASH_INTERNAL_ERROR_TEXT: &str =
    "⚠️ Something went wrong creating the issue. Please try again.";
const SLASH_NO_AGENT_TEXT: &str =
    "No available Agent is connected to this workspace. Create or share an Agent, then try `/issue` again.";
const SLASH_DISABLED_TEXT: &str = "This Slack app isn't connected to Patchbay (or was disconnected). Ask a workspace admin to reconnect it.";

/// The narrow slice of TaskService the slash command needs to hand the
/// invoker's prompt to the agent. Implemented by
/// `patchbay_service::task_service::TaskService`; tests supply a fake. The field
/// bundle mirrors the Go interface's argument list one-to-one.
#[derive(Debug, Clone, Default)]
pub struct QuickCreateRequest {
    pub workspace_id: Uuid,
    pub requester_id: Uuid,
    pub agent_id: Uuid,
    /// None — dispatch straight to the installation agent.
    pub team_id: Option<Uuid>,
    pub prompt: String,
    /// Empty = no explicit priority.
    pub priority: String,
    /// Empty = no explicit due date.
    pub due_date: String,
    pub project_id: Option<Uuid>,
    pub parent_issue_id: Option<Uuid>,
    pub attachment_ids: Vec<Uuid>,
}

#[async_trait::async_trait]
pub trait QuickCreateEnqueuer: Send + Sync {
    async fn enqueue_quick_create_task(&self, req: QuickCreateRequest) -> anyhow::Result<()>;
}

/// The slash command payload Socket Mode delivers (see channel.rs).
#[derive(Debug, Clone, Default)]
pub struct SlashCommand {
    pub command: String,
    pub text: String,
    pub user_id: String,
    pub team_id: String,
    pub api_app_id: String,
    pub channel_id: String,
    pub trigger_id: String,
    pub response_url: String,
}

/// Handles the Slack `/issue` slash command end to end.
pub struct SlashCommandProcessor {
    pool: PgPool,
    tasks: Arc<dyn QuickCreateEnqueuer>,
    binding: Option<BindingTokenService>,
    app_url: String,
    binding_path: String,
    /// Posts an ephemeral reply to the command's response_url. Injected so
    /// tests can capture the reply without hitting Slack.
    respond: Responder,
}

pub struct SlashCommandConfig {
    pub pool: PgPool,
    pub tasks: Arc<dyn QuickCreateEnqueuer>,
    /// Required for the unbound-user "link your account" reply; without them
    /// that case falls back to a plain instruction.
    pub binding: Option<BindingTokenService>,
    pub app_url: String,
    /// Default "/slack/bind".
    pub binding_path: String,
    /// Overrides the default responder (POST an ephemeral message to the
    /// command's response_url — a signed webhook, no bot token required).
    pub respond: Option<Responder>,
}

impl SlashCommandProcessor {
    pub fn new(cfg: SlashCommandConfig) -> Self {
        let mut binding_path = cfg.binding_path;
        if binding_path.is_empty() {
            binding_path = "/slack/bind".to_string();
        }
        if !binding_path.starts_with('/') {
            binding_path = format!("/{binding_path}");
        }
        let respond = cfg.respond.unwrap_or_else(default_respond);
        Self {
            pool: cfg.pool,
            tasks: cfg.tasks,
            binding: cfg.binding,
            app_url: cfg.app_url.trim_end_matches('/').to_string(),
            binding_path,
            respond,
        }
    }

    /// Processes one slash command and delivers the ephemeral reply. It is
    /// called from a detached task (the socket receive loop has already
    /// ACKed), so it never returns an error — every outcome is a user-facing
    /// message.
    pub async fn handle(&self, ctx: CancellationToken, cmd: SlashCommand) {
        // Only /issue is registered in the manifest; ignore anything else
        // defensively.
        if !cmd.command.trim().eq_ignore_ascii_case(ISSUE_SLASH_COMMAND) {
            return;
        }
        let text = self.response_text(ctx.clone(), &cmd).await;
        if text.is_empty() || cmd.response_url.is_empty() {
            return;
        }
        if let Err(err) = (self.respond)(ctx, cmd.response_url.clone(), text).await {
            tracing::warn!(
                app_id = %cmd.api_app_id,
                error = %err,
                "slack slash command: response_url reply failed"
            );
        }
    }

    /// Runs a verified HTTP slash command and returns the ephemeral response
    /// body. The managed Events API uses this instead of `response_url` so the
    /// provider is acknowledged only after the task has been durably accepted.
    pub async fn response_text(&self, ctx: CancellationToken, cmd: &SlashCommand) -> String {
        self.process(ctx, cmd).await
    }

    /// Runs the command and returns the ephemeral text to reply with.
    async fn process(&self, ctx: CancellationToken, cmd: &SlashCommand) -> String {
        let prompt = cmd.text.trim();
        if prompt.is_empty() {
            return SLASH_USAGE_TEXT.to_string();
        }

        let inst = match self
            .resolve_installation(&cmd.api_app_id, &cmd.team_id)
            .await
        {
            Ok(inst) => inst,
            Err(err) => {
                if err.downcast_ref::<ResolverError>() != Some(&ResolverError::InstallationNotFound)
                {
                    tracing::warn!(
                        app_id = %cmd.api_app_id,
                        error = %err,
                        "slack slash command: resolve installation failed"
                    );
                    return SLASH_INTERNAL_ERROR_TEXT.to_string();
                }
                return SLASH_DISABLED_TEXT.to_string();
            }
        };
        if !inst.active {
            return SLASH_DISABLED_TEXT.to_string();
        }

        let user_id = match self.resolve_user(&inst, &cmd.user_id).await {
            Ok(id) => id,
            Err(err) => {
                if err.downcast_ref::<ResolverError>() == Some(&ResolverError::SenderUnbound) {
                    return self.binding_text(ctx, &inst, &cmd.user_id).await;
                }
                if err.downcast_ref::<ResolverError>() == Some(&ResolverError::SenderNotMember) {
                    return SLASH_NOT_MEMBER_TEXT.to_string();
                }
                tracing::warn!(
                    app_id = %cmd.api_app_id,
                    error = %err,
                    "slack slash command: resolve user failed"
                );
                return SLASH_INTERNAL_ERROR_TEXT.to_string();
            }
        };

        let agent_id = if inst.agent_id.is_nil() {
            match PostgresHubRouter::new(self.pool.clone())
                .selected_or_default_agent(&inst, user_id, &cmd.channel_id)
                .await
            {
                Ok(Some(agent_id)) => agent_id,
                Ok(None) => return SLASH_NO_AGENT_TEXT.to_string(),
                Err(err) => {
                    tracing::warn!(
                        app_id = %cmd.api_app_id,
                        error = %err,
                        "slack slash command: resolve workspace Agent failed"
                    );
                    return SLASH_INTERNAL_ERROR_TEXT.to_string();
                }
            }
        } else {
            inst.agent_id
        };

        // Hand the raw natural-language prompt to the selected Agent as a
        // quick-create task; the Agent authors the well-formed issue in the
        // background and attributes it to the bound member. No project / parent
        // / attachments and no team routing — the slash command follows the
        // conversation's workspace-Hub selection when one exists.
        if let Err(err) = self
            .tasks
            .enqueue_quick_create_task(QuickCreateRequest {
                workspace_id: inst.workspace_id,
                requester_id: user_id,
                agent_id,
                // No team — dispatch straight to the selected Agent.
                team_id: None,
                prompt: prompt.to_string(),
                // No explicit priority / due date / project / parent /
                // attachments.
                priority: String::new(),
                due_date: String::new(),
                project_id: None,
                parent_issue_id: None,
                attachment_ids: vec![],
            })
            .await
        {
            tracing::warn!(
                app_id = %cmd.api_app_id,
                error = %err,
                "slack slash command: enqueue quick-create failed"
            );
            return SLASH_INTERNAL_ERROR_TEXT.to_string();
        }
        SLASH_QUEUED_TEXT.to_string()
    }

    /// Maps the command's api_app_id (+ event team) to its installation,
    /// applying the same team-scoping guard as inbound routing.
    async fn resolve_installation(
        &self,
        app_id: &str,
        team_id: &str,
    ) -> anyhow::Result<ResolvedInstallation> {
        let managed_key = crate::config::managed_routing_key(app_id, team_id);
        let inst =
            match get_channel_installation_by_app_id(&self.pool, TYPE_SLACK, &managed_key).await? {
                Some(value) => value,
                None => get_channel_installation_by_app_id(&self.pool, TYPE_SLACK, app_id)
                    .await?
                    .ok_or(ResolverError::InstallationNotFound)?,
            };
        if !installation_serves_team(&inst.config, team_id) {
            return Err(ResolverError::InstallationNotFound.into());
        }
        Ok(ResolvedInstallation {
            id: inst.id,
            workspace_id: inst.workspace_id,
            agent_id: inst.agent_id.unwrap_or_default(),
            route_revision: 0,
            installer_user_id: inst.installer_user_id,
            active: inst.status == "active",
            platform: std::sync::Arc::new(inst),
        })
    }

    /// Maps the Slack user id to the bound Patchbay user, re-checking workspace
    /// membership (no binding→member FK). Returns SenderUnbound or
    /// SenderNotMember for the product cases.
    async fn resolve_user(
        &self,
        inst: &ResolvedInstallation,
        slack_user_id: &str,
    ) -> anyhow::Result<Uuid> {
        let binding = patchbay_db::queries::channel::get_channel_user_binding_by_user_id(
            &self.pool,
            inst.id,
            slack_user_id,
        )
        .await?
        .ok_or(ResolverError::SenderUnbound)?;
        if get_member_by_user_and_workspace(&self.pool, binding.patchbay_user_id, inst.workspace_id)
            .await?
            .is_none()
        {
            return Err(ResolverError::SenderNotMember.into());
        }
        Ok(binding.patchbay_user_id)
    }

    /// Mints a single-use binding token and returns a "link your account"
    /// prompt, mirroring the outbound replier's NeedsBinding message. Falls
    /// back to a plain instruction when the binding service / app URL are not
    /// configured.
    async fn binding_text(
        &self,
        _ctx: CancellationToken,
        inst: &ResolvedInstallation,
        slack_user_id: &str,
    ) -> String {
        let (Some(binding), false) = (&self.binding, self.app_url.is_empty()) else {
            return SLASH_LINK_ACCOUNT_FALLBACK.to_string();
        };
        let token = match binding
            .mint(inst.workspace_id, inst.id, slack_user_id)
            .await
        {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(
                    installation_id = %inst.id,
                    error = %err,
                    "slack slash command: mint binding token failed"
                );
                return SLASH_LINK_ACCOUNT_FALLBACK.to_string();
            }
        };
        let bind_url = format!(
            "{}{}?token={}",
            self.app_url,
            self.binding_path,
            crate::replier::query_escape(&token.raw)
        );
        // Wrap the URL as an explicit Slack link so the base64url token's `_`/`-`
        // are not mangled by mrkdwn (same reasoning as the replier).
        format!(
            "👋 To file issues, link your Slack account to Patchbay: <{bind_url}|link your account>\n(This link expires in 15 minutes.)"
        )
    }
}

/// The responder signature: posts an ephemeral reply to a command's
/// response_url. A boxed future keeps the injection point object-safe.
pub type RespondFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>;
pub type Responder = Arc<dyn Fn(CancellationToken, String, String) -> RespondFuture + Send + Sync>;

/// The default responder POSTs an ephemeral message to the command's
/// response_url (a signed webhook — no bot token required).
fn default_respond() -> Responder {
    Arc::new(|_ctx, response_url, text| {
        Box::pin(async move {
            let payload = serde_json::json!({
                "response_type": "ephemeral",
                "text": text,
            });
            let client = reqwest::Client::new();
            let resp = client
                .post(&response_url)
                .json(&payload)
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("slack post webhook: {e}"))?;
            let status = resp.status();
            if !status.is_success() {
                anyhow::bail!("slack post webhook: http {status}");
            }
            Ok(())
        })
    })
}
