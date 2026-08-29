//! Workspace-scoped Agent routing for channel installations.
//!
//! A connected IM platform is a Hub: the installation is owned by the
//! workspace, while each conversation remembers the Agent selected from the
//! channel itself (`/agents`). This keeps the settings page from making a
//! channel connection on behalf of one arbitrary Agent.

use async_trait::async_trait;
use patchbay_channel::InboundMessage;
use patchbay_db::models::Agent;
use patchbay_db::queries::agent::list_agents;
use patchbay_db::queries::agent_invocation_target::list_agent_invocation_targets;
use patchbay_db::queries::channel::{
    get_channel_chat_session_binding, merge_channel_chat_session_binding_config,
};
use patchbay_db::queries::chat::switch_chat_session_agent;
use patchbay_db::queries::member::get_member_by_user_and_workspace;
use sqlx::PgPool;
use uuid::Uuid;

use crate::resolvers::{ResolvedIdentity, ResolvedInstallation};

const HUB_AGENT_CONFIG_KEY: &str = "hub_agent_id";

/// The result of resolving one message against the channel hub.
#[derive(Debug, Clone, Default)]
pub struct HubResolution {
    pub agent_id: Option<Uuid>,
    pub reply_text: Option<String>,
    /// A control command is delivered as a reply and must not become an Agent
    /// turn. `/agents <selector>` still creates the conversation binding so
    /// the next ordinary message continues with the selected Agent.
    pub handled: bool,
    pub ensure_session: bool,
}

#[async_trait]
pub trait HubRouter: Send + Sync {
    async fn resolve(
        &self,
        installation: &ResolvedInstallation,
        identity: &ResolvedIdentity,
        message: &InboundMessage,
        binding_key: &str,
    ) -> anyhow::Result<HubResolution>;

    async fn persist_route(
        &self,
        installation_id: Uuid,
        workspace_id: Uuid,
        binding_key: &str,
        chat_session_id: Uuid,
        agent_id: Uuid,
    ) -> anyhow::Result<()>;
}

/// PostgreSQL implementation shared by every channel adapter.
pub struct PostgresHubRouter {
    pool: PgPool,
}

impl PostgresHubRouter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Returns the Agent selected for a conversation, falling back to the
    /// first Agent visible to the sender. This is used by Slack's dedicated
    /// `/issue` slash command, which does not travel through the normal
    /// inbound-message router but should still honor `/agents` state.
    pub async fn selected_or_default_agent(
        &self,
        installation: &ResolvedInstallation,
        user_id: Uuid,
        binding_key: &str,
    ) -> anyhow::Result<Option<Uuid>> {
        let agents = self
            .available_agents(installation.workspace_id, user_id)
            .await?;
        let current_id = self.current_agent_id(installation.id, binding_key).await?;
        Ok(current_id
            .filter(|id| agents.iter().any(|agent| agent.id == *id))
            .or_else(|| agents.first().map(|agent| agent.id)))
    }

    async fn available_agents(
        &self,
        workspace_id: Uuid,
        user_id: Uuid,
    ) -> anyhow::Result<Vec<Agent>> {
        let agents = list_agents(&self.pool, workspace_id).await?;
        let is_workspace_admin =
            get_member_by_user_and_workspace(&self.pool, user_id, workspace_id)
                .await?
                .is_some_and(|member| matches!(member.role.as_str(), "owner" | "admin"));
        if is_workspace_admin {
            return Ok(agents);
        }
        let mut available = Vec::with_capacity(agents.len());
        for agent in agents {
            if agent.owner_id == Some(user_id) {
                available.push(agent);
                continue;
            }
            if agent.permission_mode != "public_to" {
                continue;
            }
            let targets = list_agent_invocation_targets(&self.pool, agent.id).await?;
            if targets.iter().any(|target| {
                (target.target_type == "workspace")
                    || (target.target_type == "member" && target.target_id == user_id)
            }) {
                available.push(agent);
            }
        }
        Ok(available)
    }

    async fn current_agent_id(
        &self,
        installation_id: Uuid,
        binding_key: &str,
    ) -> anyhow::Result<Option<Uuid>> {
        let Some(binding) =
            get_channel_chat_session_binding(&self.pool, installation_id, binding_key).await?
        else {
            return Ok(None);
        };
        Ok(binding
            .config
            .get(HUB_AGENT_CONFIG_KEY)
            .and_then(serde_json::Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok()))
    }
}

#[async_trait]
impl HubRouter for PostgresHubRouter {
    async fn resolve(
        &self,
        installation: &ResolvedInstallation,
        identity: &ResolvedIdentity,
        message: &InboundMessage,
        binding_key: &str,
    ) -> anyhow::Result<HubResolution> {
        let agents = self
            .available_agents(installation.workspace_id, identity.user_id)
            .await?;
        let current_id = self.current_agent_id(installation.id, binding_key).await?;
        let current = current_id.and_then(|id| agents.iter().find(|agent| agent.id == id));
        let default_agent = current.or_else(|| agents.first());
        let command = parse_agents_command(&message.command_text);

        if let Some(selector) = command {
            if selector.is_empty() {
                return Ok(HubResolution {
                    agent_id: default_agent.map(|agent| agent.id),
                    reply_text: Some(render_agent_list(
                        &agents,
                        default_agent.map(|agent| agent.id),
                    )),
                    handled: true,
                    ensure_session: false,
                });
            }

            let selected = select_agent(&agents, &selector);
            let Some(selected) = selected else {
                return Ok(HubResolution {
                    agent_id: default_agent.map(|agent| agent.id),
                    reply_text: Some(format!(
                        "I couldn't find an Agent matching `{selector}`.\n\n{}",
                        render_agent_list(&agents, default_agent.map(|agent| agent.id))
                    )),
                    handled: true,
                    ensure_session: false,
                });
            };
            return Ok(HubResolution {
                agent_id: Some(selected.id),
                reply_text: Some(format!(
                    "Switched to Agent **{}**. Send your next message to continue here.",
                    selected.name
                )),
                handled: true,
                ensure_session: true,
            });
        }

        let Some(agent) = default_agent else {
            return Ok(HubResolution {
                reply_text: Some(
                    "No available Agents are connected to your Patchbay account in this workspace."
                        .to_string(),
                ),
                handled: true,
                ensure_session: false,
                ..Default::default()
            });
        };

        Ok(HubResolution {
            agent_id: Some(agent.id),
            ..Default::default()
        })
    }

    async fn persist_route(
        &self,
        installation_id: Uuid,
        workspace_id: Uuid,
        binding_key: &str,
        chat_session_id: Uuid,
        agent_id: Uuid,
    ) -> anyhow::Result<()> {
        let config = serde_json::json!({HUB_AGENT_CONFIG_KEY: agent_id.to_string()});
        merge_channel_chat_session_binding_config(
            &self.pool,
            installation_id,
            binding_key,
            &config,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("channel hub binding not found after session creation"))?;
        if !switch_chat_session_agent(&self.pool, workspace_id, chat_session_id, agent_id).await? {
            anyhow::bail!("selected Agent is no longer available");
        }
        Ok(())
    }
}

fn parse_agents_command(command_text: &str) -> Option<String> {
    let mut words = command_text.split_whitespace();
    let first = words.next()?.trim().to_ascii_lowercase();
    let command = first
        .strip_prefix('/')
        .and_then(|value| value.split('@').next())
        .is_some_and(|value| value == "agents" || value == "agent");
    if !command {
        return None;
    }
    Some(words.collect::<Vec<_>>().join(" "))
}

fn select_agent<'a>(agents: &'a [Agent], selector: &str) -> Option<&'a Agent> {
    if let Ok(index) = selector.parse::<usize>() {
        return index.checked_sub(1).and_then(|index| agents.get(index));
    }
    if let Ok(id) = Uuid::parse_str(selector) {
        return agents.iter().find(|agent| agent.id == id);
    }
    let wanted = selector.to_ascii_lowercase();
    agents
        .iter()
        .find(|agent| agent.name.to_ascii_lowercase() == wanted)
}

fn render_agent_list(agents: &[Agent], current_id: Option<Uuid>) -> String {
    if agents.is_empty() {
        return "No available Agents were found.".to_string();
    }
    let mut text = String::from("Available Agents:\n");
    for (index, agent) in agents.iter().enumerate() {
        let marker = (Some(agent.id) == current_id)
            .then_some(" (current)")
            .unwrap_or("");
        text.push_str(&format!("{}. {}{}\n", index + 1, agent.name, marker));
    }
    text.push_str("\nSend `/agents <number>` or `/agents <name>` to switch.");
    text
}

#[cfg(test)]
mod tests {
    use super::parse_agents_command;

    #[test]
    fn parses_agent_commands_and_bot_suffixes() {
        assert_eq!(parse_agents_command("/agents"), Some(String::new()));
        assert_eq!(parse_agents_command("/agents 2"), Some("2".to_string()));
        assert_eq!(
            parse_agents_command("/agents@patchbay  Reviewer  "),
            Some("Reviewer".to_string())
        );
        assert_eq!(
            parse_agents_command("/agent reviewer"),
            Some("reviewer".to_string())
        );
    }

    #[test]
    fn ignores_unrelated_commands() {
        assert_eq!(parse_agents_command("hello"), None);
        assert_eq!(parse_agents_command("/issue fix login"), None);
        assert_eq!(parse_agents_command("agents"), None);
    }
}
