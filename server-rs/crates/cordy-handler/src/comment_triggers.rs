//! Comment-trigger routing.
//!
//! This is the handler-side port of the explicit `@agent` / `@squad` branch and
//! the implicit reply/conversation/assignee branches in
//! `server/internal/handler/comment.go`. It deliberately keeps target
//! resolution separate from execution: two named targets may resolve to one
//! agent, while the API still returns one outcome for every named target.

use crate::state::HandlerState;
use chrono::{Duration, Utc};
use cordy_db::models::{Agent, AgentTaskQueue, Comment, Issue};
use cordy_db::queries::{agent, agent_invocation_target, comment, member, squad, workspace};
use cordy_service::agent_ready::agent_readiness;
use cordy_service::dispatch_reason::ReasonCode;
use cordy_service::task_service::{pending_slot_taken_err, TaskServiceError};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DispatchStatus {
    Queued,
    Coalesced,
    Deferred,
    Blocked,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CommentTriggerOutcome {
    pub target_type: String,
    pub target_id: String,
    pub status: DispatchStatus,
    pub reason_code: ReasonCode,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PreviewAgent {
    pub id: Uuid,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    pub source: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PreviewResponse {
    pub agents: Vec<PreviewAgent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<CommentTriggerOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Mention {
    kind: String,
    id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TriggerSource {
    IssueAssignee,
    MentionAgent,
    MentionSquadLeader,
    ThreadParent,
    Conversation,
}

impl TriggerSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::IssueAssignee => "issue_assignee",
            Self::MentionAgent => "mention_agent",
            Self::MentionSquadLeader => "mention_squad_leader",
            Self::ThreadParent => "thread_parent",
            Self::Conversation => "conversation_continuation",
        }
    }

    fn uses_delegation_attribution(self) -> bool {
        !matches!(self, Self::IssueAssignee)
    }

    fn schedules_escalation(self) -> bool {
        matches!(self, Self::ThreadParent | Self::Conversation)
    }
}

#[derive(Debug, Clone)]
struct EscalationFallback {
    agent: Agent,
    squad_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
struct Trigger {
    agent: Agent,
    squad_id: Option<Uuid>,
    is_leader: bool,
    source: TriggerSource,
    already_pending: bool,
    escalation_fallback: Option<EscalationFallback>,
}

#[derive(Debug, Clone)]
struct Target {
    target_type: String,
    target_id: String,
    agent_id: Option<Uuid>,
    terminal: Option<(DispatchStatus, ReasonCode)>,
}

#[derive(Debug, Clone, Copy)]
struct EnqueueResult {
    status: DispatchStatus,
    reason: ReasonCode,
    execution_squad_id: Option<Uuid>,
    task_id: Option<Uuid>,
}

fn parse_mentions(content: &str) -> Vec<Mention> {
    // Keep this expression byte-for-byte compatible with util.MentionRe. In
    // particular, the label is intentionally non-greedy so labels containing
    // square brackets remain valid mention anchors.
    let expression =
        Regex::new(r"\[@?(.+?)\]\(mention://(member|agent|squad|issue|all)/([0-9a-fA-F-]+|all)\)")
            .expect("comment mention regex is valid");
    let mut seen = HashSet::new();
    let mut mentions = Vec::new();
    for captures in expression.captures_iter(content) {
        let Some(kind) = captures.get(2) else {
            continue;
        };
        let Some(id) = captures.get(3) else {
            continue;
        };
        let mention = Mention {
            kind: kind.as_str().to_string(),
            id: id.as_str().to_string(),
        };
        if seen.insert(mention.clone()) {
            mentions.push(mention);
        }
    }
    mentions
}

fn is_note_comment(content: &str) -> bool {
    content
        .split_whitespace()
        .next()
        .is_some_and(|token| token.eq_ignore_ascii_case("/note"))
}

/// Resolve the human principal used by the A2A invocation gate. A member is
/// their own principal; an agent may only borrow the originator recorded on
/// the trusted speaking task.
pub(crate) async fn invocation_originator(
    state: &HandlerState,
    actor_type: &str,
    actor_id: Uuid,
    task_id: Option<Uuid>,
) -> Option<Uuid> {
    if actor_type == "member" {
        return Some(actor_id);
    }
    if actor_type != "agent" {
        return None;
    }
    let task_id = task_id?;
    agent::get_agent_task(&state.pool, task_id)
        .await
        .ok()
        .flatten()
        .and_then(|task| task.originator_user_id)
}

async fn can_invoke_agent(
    state: &HandlerState,
    target: &Agent,
    actor_type: &str,
    actor_id: Uuid,
    originator_user_id: Option<Uuid>,
    workspace_id: Uuid,
) -> bool {
    // Members invoke as themselves. Agent/system actors are judged by the
    // top-of-chain human, never by the immediate agent principal.
    let effective_user = if actor_type == "member" {
        Some(actor_id)
    } else {
        originator_user_id
    };

    if effective_user.is_some() && target.owner_id == effective_user {
        return true;
    }
    if target.permission_mode != "public_to" {
        return false;
    }

    let targets = match agent_invocation_target::list_agent_invocation_targets(
        &state.pool,
        target.id,
    )
    .await
    {
        Ok(targets) => targets,
        Err(_) => return false,
    };
    let workspace_broad = matches!(actor_type, "agent" | "system");
    let workspace_member = match effective_user {
        Some(user_id) => {
            member::get_member_by_user_and_workspace(&state.pool, user_id, workspace_id)
                .await
                .is_ok_and(|row| row.is_some())
        }
        None => false,
    };

    targets
        .iter()
        .any(|entry| match entry.target_type.as_str() {
            "workspace" => workspace_broad || workspace_member,
            "member" => effective_user == Some(entry.target_id),
            // Team targets are intentionally inert until team membership exists.
            "team" => false,
            _ => false,
        })
}

fn blocked(target_type: &str, target_id: &str, reason: ReasonCode) -> Target {
    Target {
        target_type: target_type.to_string(),
        target_id: target_id.to_string(),
        agent_id: None,
        terminal: Some((DispatchStatus::Blocked, reason)),
    }
}

fn add_trigger(triggers: &mut Vec<Trigger>, seen: &mut HashMap<Uuid, usize>, trigger: Trigger) {
    if let Some(index) = seen.get(&trigger.agent.id).copied() {
        // A squad mention upgrades a plain agent trigger: the leader briefing
        // is a strict superset of the generic run, independent of mention order.
        if trigger.is_leader && !triggers[index].is_leader {
            triggers[index] = trigger;
        }
        return;
    }
    seen.insert(trigger.agent.id, triggers.len());
    triggers.push(trigger);
}

async fn resolve_explicit(
    state: &HandlerState,
    issue: &Issue,
    content: &str,
    actor_type: &str,
    actor_id: Uuid,
    originator_user_id: Option<Uuid>,
    suppressed_agent_ids: &HashSet<Uuid>,
) -> (Vec<Trigger>, Vec<Target>) {
    if is_note_comment(content) {
        return (Vec::new(), Vec::new());
    }

    let mut triggers = Vec::new();
    let mut seen_agents = HashMap::new();
    let mut targets = Vec::new();
    let mut seen_targets = HashSet::new();

    for mention in parse_mentions(content) {
        if mention.kind != "agent" && mention.kind != "squad" {
            // @all, member, and issue mentions do not themselves enqueue an
            // agent. @all is handled by the implicit-routing contract; explicit
            // targets still win over it.
            continue;
        }
        let target_key = format!("{}:{}", mention.kind, mention.id);
        if !seen_targets.insert(target_key) {
            continue;
        }

        if mention.kind == "agent" {
            let Ok(agent_id) = Uuid::parse_str(&mention.id) else {
                targets.push(blocked("agent", &mention.id, ReasonCode::TargetUnavailable));
                continue;
            };
            let agent = match agent::get_agent_in_workspace(
                &state.pool,
                agent_id,
                issue.workspace_id,
            )
            .await
            {
                Ok(Some(agent)) => agent,
                // A well-formed but unresolved id must not reveal whether a
                // private target exists in another workspace.
                Ok(None) | Err(_) => {
                    targets.push(blocked(
                        "agent",
                        &mention.id,
                        ReasonCode::InvocationNotAllowed,
                    ));
                    continue;
                }
            };
            if !can_invoke_agent(
                state,
                &agent,
                actor_type,
                actor_id,
                originator_user_id,
                issue.workspace_id,
            )
            .await
            {
                targets.push(blocked(
                    "agent",
                    &mention.id,
                    ReasonCode::InvocationNotAllowed,
                ));
                continue;
            }
            if agent.archived_at.is_some() {
                targets.push(blocked("agent", &mention.id, ReasonCode::TargetUnavailable));
                continue;
            }
            if let Ok(verdict) = agent_readiness(&state.pool, &agent).await {
                if verdict.blocked() {
                    targets.push(blocked("agent", &mention.id, verdict.reason));
                    continue;
                }
            }
            if suppressed_agent_ids.contains(&agent.id) {
                // Suppression is the explicit composer opt-out: no outcome is
                // returned for a target the user unchecked.
                continue;
            }
            add_trigger(
                &mut triggers,
                &mut seen_agents,
                Trigger {
                    agent,
                    squad_id: None,
                    is_leader: false,
                    source: TriggerSource::MentionAgent,
                    already_pending: false,
                    escalation_fallback: None,
                },
            );
            targets.push(Target {
                target_type: "agent".to_string(),
                target_id: mention.id,
                agent_id: Some(agent_id),
                terminal: None,
            });
            continue;
        }

        let Ok(squad_id) = Uuid::parse_str(&mention.id) else {
            targets.push(blocked("squad", &mention.id, ReasonCode::TargetUnavailable));
            continue;
        };
        let squad =
            match squad::get_squad_in_workspace(&state.pool, squad_id, issue.workspace_id).await {
                Ok(Some(squad)) => squad,
                Ok(None) | Err(_) => {
                    targets.push(blocked("squad", &mention.id, ReasonCode::TargetUnavailable));
                    continue;
                }
            };
        if squad.archived_at.is_some() {
            targets.push(blocked("squad", &mention.id, ReasonCode::TargetUnavailable));
            continue;
        }
        let leader =
            match agent::get_agent_in_workspace(&state.pool, squad.leader_id, issue.workspace_id)
                .await
            {
                Ok(Some(agent)) => agent,
                Ok(None) | Err(_) => {
                    targets.push(blocked("squad", &mention.id, ReasonCode::TargetUnavailable));
                    continue;
                }
            };
        if !can_invoke_agent(
            state,
            &leader,
            actor_type,
            actor_id,
            originator_user_id,
            issue.workspace_id,
        )
        .await
        {
            targets.push(blocked(
                "squad",
                &mention.id,
                ReasonCode::InvocationNotAllowed,
            ));
            continue;
        }
        if leader.archived_at.is_some() {
            targets.push(blocked("squad", &mention.id, ReasonCode::TargetUnavailable));
            continue;
        }
        if let Ok(verdict) = agent_readiness(&state.pool, &leader).await {
            if verdict.blocked() {
                targets.push(blocked("squad", &mention.id, verdict.reason));
                continue;
            }
        }
        if suppressed_agent_ids.contains(&leader.id) {
            continue;
        }
        add_trigger(
            &mut triggers,
            &mut seen_agents,
            Trigger {
                agent: leader,
                squad_id: Some(squad.id),
                is_leader: true,
                source: TriggerSource::MentionSquadLeader,
                already_pending: false,
                escalation_fallback: None,
            },
        );
        targets.push(Target {
            target_type: "squad".to_string(),
            target_id: mention.id,
            agent_id: Some(squad.leader_id),
            terminal: None,
        });
    }

    (triggers, targets)
}

async fn has_pending_task(
    state: &HandlerState,
    issue: &Issue,
    agent_id: Uuid,
    exclude_trigger_comment_id: Option<Uuid>,
) -> Result<bool, ReasonCode> {
    let head = state.tasks.resolve_issue_review_sha(issue.id).await;
    let head = (!head.is_empty()).then_some(head);
    let value = match exclude_trigger_comment_id {
        Some(comment_id) => {
            agent::has_pending_task_for_issue_and_agent_excluding_trigger_comment(
                &state.pool,
                issue.id,
                agent_id,
                comment_id,
                head.as_deref(),
            )
            .await
        }
        None => {
            agent::has_pending_task_for_issue_and_agent(
                &state.pool,
                issue.id,
                agent_id,
                head.as_deref(),
            )
            .await
        }
    }
    .map_err(|_| ReasonCode::InternalError)?;
    Ok(value.unwrap_or(false))
}

async fn route_agent(
    state: &HandlerState,
    issue: &Issue,
    agent_id: Uuid,
    source: TriggerSource,
    squad_id: Option<Uuid>,
    actor_type: &str,
    actor_id: Uuid,
    originator_user_id: Option<Uuid>,
    exclude_trigger_comment_id: Option<Uuid>,
) -> Option<Trigger> {
    let agent = agent::get_agent_in_workspace(&state.pool, agent_id, issue.workspace_id)
        .await
        .ok()??;
    if agent.archived_at.is_some() || agent.runtime_id.is_none() {
        return None;
    }
    if !can_invoke_agent(
        state,
        &agent,
        actor_type,
        actor_id,
        originator_user_id,
        issue.workspace_id,
    )
    .await
    {
        return None;
    }
    let already_pending = has_pending_task(state, issue, agent.id, exclude_trigger_comment_id)
        .await
        .ok()?;
    Some(Trigger {
        agent,
        squad_id,
        is_leader: matches!(source, TriggerSource::MentionSquadLeader)
            || (matches!(
                source,
                TriggerSource::IssueAssignee | TriggerSource::Conversation
            ) && squad_id.is_some()),
        source,
        already_pending,
        escalation_fallback: None,
    })
}

async fn should_suppress_squad_leader_self_trigger(
    state: &HandlerState,
    issue_id: Uuid,
    leader_id: Uuid,
    squad_id: Uuid,
) -> bool {
    let latest =
        agent::get_latest_task_role_for_issue_and_agent(&state.pool, issue_id, leader_id).await;
    let Ok(Some(latest)) = latest else {
        // Go treats a role-query failure as an unavailable suppression proof and
        // lets the normal enqueue path decide what to do.
        return false;
    };
    if latest.is_leader_task {
        return true;
    }
    latest.squad_id != Some(squad_id)
}

async fn route_assignee_fallback(
    state: &HandlerState,
    issue: &Issue,
    actor_type: &str,
    actor_id: Uuid,
    originator_user_id: Option<Uuid>,
    exclude_trigger_comment_id: Option<Uuid>,
) -> Option<Trigger> {
    let (assignee_type, assignee_id) = (issue.assignee_type.as_deref()?, issue.assignee_id?);
    match assignee_type {
        "agent" => {
            route_agent(
                state,
                issue,
                assignee_id,
                TriggerSource::IssueAssignee,
                None,
                actor_type,
                actor_id,
                originator_user_id,
                exclude_trigger_comment_id,
            )
            .await
        }
        "squad" => {
            let assigned_squad =
                squad::get_squad_in_workspace(&state.pool, assignee_id, issue.workspace_id)
                    .await
                    .ok()??;
            if actor_type == "agent"
                && actor_id == assigned_squad.leader_id
                && should_suppress_squad_leader_self_trigger(
                    state,
                    issue.id,
                    assigned_squad.leader_id,
                    assigned_squad.id,
                )
                .await
            {
                return None;
            }
            route_agent(
                state,
                issue,
                assigned_squad.leader_id,
                TriggerSource::IssueAssignee,
                Some(assigned_squad.id),
                actor_type,
                actor_id,
                originator_user_id,
                exclude_trigger_comment_id,
            )
            .await
        }
        _ => None,
    }
}

async fn route_conversation_owner(
    state: &HandlerState,
    issue: &Issue,
    agent_id: Uuid,
    squad_id: Option<Uuid>,
    member_id: Uuid,
    exclude_trigger_comment_id: Option<Uuid>,
) -> Option<Trigger> {
    let squad_id = if let Some(candidate) = squad_id {
        if squad::get_squad_in_workspace(&state.pool, candidate, issue.workspace_id)
            .await
            .ok()
            .flatten()
            .is_some()
        {
            Some(candidate)
        } else {
            // A stale historical squad id does not prevent the conversation
            // from continuing with the agent, matching Go's best-effort
            // squad lookup.
            None
        }
    } else {
        None
    };
    route_agent(
        state,
        issue,
        agent_id,
        TriggerSource::Conversation,
        squad_id,
        "member",
        member_id,
        Some(member_id),
        exclude_trigger_comment_id,
    )
    .await
}

async fn route_first_explicit_root_owner(
    state: &HandlerState,
    issue: &Issue,
    root: &Comment,
    member_id: Uuid,
    exclude_trigger_comment_id: Option<Uuid>,
) -> (bool, Option<Trigger>) {
    for mention in parse_mentions(&root.content) {
        match mention.kind.as_str() {
            "agent" => {
                let Ok(agent_id) = Uuid::parse_str(&mention.id) else {
                    return (true, None);
                };
                return (
                    true,
                    route_conversation_owner(
                        state,
                        issue,
                        agent_id,
                        None,
                        member_id,
                        exclude_trigger_comment_id,
                    )
                    .await,
                );
            }
            "squad" => {
                let Ok(squad_id) = Uuid::parse_str(&mention.id) else {
                    return (true, None);
                };
                let Ok(Some(assigned_squad)) =
                    squad::get_squad_in_workspace(&state.pool, squad_id, issue.workspace_id).await
                else {
                    return (true, None);
                };
                return (
                    true,
                    route_conversation_owner(
                        state,
                        issue,
                        assigned_squad.leader_id,
                        Some(assigned_squad.id),
                        member_id,
                        exclude_trigger_comment_id,
                    )
                    .await,
                );
            }
            _ => {}
        }
    }
    (false, None)
}

async fn route_conversation_owners(
    state: &HandlerState,
    issue: &Issue,
    root: &Comment,
    member_id: Uuid,
    exclude_trigger_comment_id: Option<Uuid>,
) -> (Vec<Trigger>, bool) {
    if root.author_type != "member" || root.issue_id != issue.id {
        return (Vec::new(), false);
    }
    let (has_explicit_owner, owner) =
        route_first_explicit_root_owner(state, issue, root, member_id, exclude_trigger_comment_id)
            .await;
    if has_explicit_owner {
        return (owner.into_iter().collect(), true);
    }

    let tasks = match agent::list_tasks_by_issue(&state.pool, issue.id).await {
        Ok(tasks) => tasks,
        Err(_) => return (Vec::new(), false),
    };
    let mut owner_indexes = HashMap::<Uuid, usize>::new();
    let mut owners = Vec::<(Uuid, Option<Uuid>)>::new();
    for task in tasks {
        if task.trigger_comment_id != Some(root.id)
            || exclude_trigger_comment_id == task.trigger_comment_id
        {
            continue;
        }
        if let Some(index) = owner_indexes.get(&task.agent_id).copied() {
            if owners[index].1.is_none() {
                owners[index].1 = task.squad_id;
            }
        } else {
            owner_indexes.insert(task.agent_id, owners.len());
            owners.push((task.agent_id, task.squad_id));
        }
    }
    if owners.is_empty() {
        return (Vec::new(), false);
    }

    let mut triggers = Vec::with_capacity(owners.len());
    for (agent_id, squad_id) in owners {
        if let Some(trigger) = route_conversation_owner(
            state,
            issue,
            agent_id,
            squad_id,
            member_id,
            exclude_trigger_comment_id,
        )
        .await
        {
            triggers.push(trigger);
        }
    }
    (triggers, true)
}

async fn route_implicit(
    state: &HandlerState,
    issue: &Issue,
    parent: Option<&Comment>,
    actor_type: &str,
    actor_id: Uuid,
    originator_user_id: Option<Uuid>,
    exclude_trigger_comment_id: Option<Uuid>,
) -> Vec<Trigger> {
    if actor_type != "member" {
        if issue.assignee_type.as_deref() == Some("squad") {
            return route_assignee_fallback(
                state,
                issue,
                actor_type,
                actor_id,
                originator_user_id,
                exclude_trigger_comment_id,
            )
            .await
            .into_iter()
            .collect();
        }
        return Vec::new();
    }

    if let Some(parent) = parent {
        if parent.author_type == "agent" {
            let Some(mut trigger) = route_agent(
                state,
                issue,
                parent.author_id,
                TriggerSource::ThreadParent,
                None,
                actor_type,
                actor_id,
                originator_user_id,
                exclude_trigger_comment_id,
            )
            .await
            else {
                return Vec::new();
            };
            if let Some(fallback) = route_assignee_fallback(
                state,
                issue,
                actor_type,
                actor_id,
                originator_user_id,
                exclude_trigger_comment_id,
            )
            .await
            .filter(|fallback| fallback.agent.id != trigger.agent.id)
            {
                trigger.escalation_fallback = Some(EscalationFallback {
                    agent: fallback.agent,
                    squad_id: fallback.squad_id,
                });
            }
            return vec![trigger];
        }

        if let Ok(Some(root)) =
            comment::get_thread_root(&state.pool, parent.id, issue.workspace_id).await
        {
            let (triggers, handled) = route_conversation_owners(
                state,
                issue,
                &root,
                actor_id,
                exclude_trigger_comment_id,
            )
            .await;
            if handled {
                if triggers.len() == 1 {
                    let mut triggers = triggers;
                    if let Some(fallback) = route_assignee_fallback(
                        state,
                        issue,
                        actor_type,
                        actor_id,
                        originator_user_id,
                        exclude_trigger_comment_id,
                    )
                    .await
                    .filter(|fallback| fallback.agent.id != triggers[0].agent.id)
                    {
                        triggers[0].escalation_fallback = Some(EscalationFallback {
                            agent: fallback.agent,
                            squad_id: fallback.squad_id,
                        });
                    }
                    return triggers;
                }
                return triggers;
            }
        }

        // A member-to-member reply is a human discussion, not an assignee
        // fallback. A missing/invalid root is conservatively treated the same.
        if parent.author_type == "member" {
            return Vec::new();
        }
    }

    route_assignee_fallback(
        state,
        issue,
        actor_type,
        actor_id,
        originator_user_id,
        exclude_trigger_comment_id,
    )
    .await
    .into_iter()
    .collect()
}

async fn resolve_comment_triggers(
    state: &HandlerState,
    issue: &Issue,
    content: &str,
    parent: Option<&Comment>,
    actor_type: &str,
    actor_id: Uuid,
    originator_user_id: Option<Uuid>,
    exclude_trigger_comment_id: Option<Uuid>,
    suppressed_agent_ids: &HashSet<Uuid>,
) -> (Vec<Trigger>, Vec<Target>) {
    if is_note_comment(content) {
        return (Vec::new(), Vec::new());
    }
    let mentions = parse_mentions(content);
    let has_explicit = mentions
        .iter()
        .any(|mention| matches!(mention.kind.as_str(), "agent" | "squad"));
    let (mut triggers, targets) = if has_explicit {
        resolve_explicit(
            state,
            issue,
            content,
            actor_type,
            actor_id,
            originator_user_id,
            suppressed_agent_ids,
        )
        .await
    } else if mentions
        .iter()
        .any(|mention| matches!(mention.kind.as_str(), "all" | "member"))
    {
        (Vec::new(), Vec::new())
    } else {
        (
            route_implicit(
                state,
                issue,
                parent,
                actor_type,
                actor_id,
                originator_user_id,
                exclude_trigger_comment_id,
            )
            .await,
            Vec::new(),
        )
    };
    triggers.retain(|trigger| !suppressed_agent_ids.contains(&trigger.agent.id));
    (triggers, targets)
}

async fn comment_routing_escalation_delay(state: &HandlerState, workspace_id: Uuid) -> Duration {
    const DEFAULT_SECONDS: i64 = 5 * 60;
    let default = Duration::seconds(DEFAULT_SECONDS);
    let Ok(Some(workspace)) = workspace::get_workspace(&state.pool, workspace_id).await else {
        return default;
    };
    let Some(seconds) = workspace
        .settings
        .get("comment_routing")
        .and_then(|value| value.get("escalation_seconds"))
        .and_then(Value::as_i64)
    else {
        return default;
    };
    if seconds <= 0 {
        Duration::zero()
    } else {
        Duration::seconds(seconds)
    }
}

fn attribution_blocked(error: &TaskServiceError) -> bool {
    matches!(
        error,
        TaskServiceError::FailClosedPolicyUnavailable(_)
            | TaskServiceError::FailClosedPolicyRead(_, _)
            | TaskServiceError::FailClosed(_)
            | TaskServiceError::FailClosedNoOwner(_)
    )
}

fn enqueue_failure_reason(error: &TaskServiceError) -> ReasonCode {
    match error {
        TaskServiceError::AgentArchived => ReasonCode::TargetUnavailable,
        TaskServiceError::AgentNoRuntime => ReasonCode::AgentRuntimeRequired,
        error if attribution_blocked(error) => ReasonCode::AttributionBlocked,
        _ => ReasonCode::InternalError,
    }
}

async fn merge_into_pending(
    state: &HandlerState,
    issue: &Issue,
    trigger: &Trigger,
    comment_id: Uuid,
) -> Result<bool, ReasonCode> {
    let attr = state
        .tasks
        .attribution_for_merged_comment(
            issue.workspace_id,
            Some(comment_id),
            trigger.source.uses_delegation_attribution(),
            &trigger.agent,
        )
        .await
        .map_err(|error| {
            if attribution_blocked(&error) {
                ReasonCode::AttributionBlocked
            } else {
                ReasonCode::InternalError
            }
        })?;

    let (overlay, connected_apps) = match attr.user_id {
        Some(originator) => {
            state
                .tasks
                .build_runtime_mcp_overlay_for_merge(originator, &trigger.agent)
                .await
        }
        None => (None, None),
    };
    let summary = state
        .tasks
        .build_comment_trigger_summary(issue.workspace_id, Some(comment_id))
        .await
        .unwrap_or(None);
    let head_sha = state.tasks.resolve_issue_review_sha(issue.id).await;
    let attr_source = attr
        .source
        .as_ref()
        .map(|source| source.as_str().to_string());
    let delegated_from = attr.delegated_from_task_id.unwrap_or_else(Uuid::nil);
    let rule_version = attr.rule_version_id.unwrap_or_else(Uuid::nil);
    let evidence_kind = attr
        .evidence_kind
        .as_ref()
        .filter(|kind| !kind.as_str().is_empty())
        .map(|kind| kind.as_str().to_string());
    let evidence_ref = attr.evidence_ref_id.unwrap_or_else(Uuid::nil);
    let merged = cordy_db::queries::agent::merge_comment_into_pending_task(
        &state.pool,
        comment_id,
        summary.as_deref(),
        attr.user_id.unwrap_or_else(Uuid::nil),
        attr.accountable_user_id.unwrap_or_else(Uuid::nil),
        attr_source.as_deref(),
        delegated_from,
        rule_version,
        evidence_kind.as_deref(),
        evidence_ref,
        &overlay.unwrap_or(Value::Null),
        &connected_apps.unwrap_or(Value::Null),
        issue.id,
        trigger.agent.id,
        (!head_sha.is_empty()).then_some(head_sha.as_str()),
    )
    .await
    .map_err(|_| ReasonCode::InternalError)?;
    Ok(merged.is_some())
}

async fn register_planned(
    state: &HandlerState,
    issue: &Issue,
    trigger: &Trigger,
    comment_id: Uuid,
) -> Result<bool, ReasonCode> {
    let head_sha = state.tasks.resolve_issue_review_sha(issue.id).await;
    cordy_db::queries::agent::register_planned_comment_for_active_task(
        &state.pool,
        comment_id,
        issue.id,
        trigger.agent.id,
        (!head_sha.is_empty()).then_some(head_sha.as_str()),
    )
    .await
    .map(|row| row.is_some())
    .map_err(|_| ReasonCode::InternalError)
}

async fn enqueue_fresh_trigger(
    state: &HandlerState,
    issue: &Issue,
    trigger: &Trigger,
    comment_id: Uuid,
) -> Result<AgentTaskQueue, TaskServiceError> {
    match trigger.source {
        TriggerSource::IssueAssignee => {
            if let Some(squad_id) = trigger.squad_id {
                state
                    .tasks
                    .enqueue_task_for_squad_leader(
                        issue,
                        trigger.agent.id,
                        squad_id,
                        Some(comment_id),
                    )
                    .await
            } else {
                state
                    .tasks
                    .enqueue_task_for_issue(issue, Some(comment_id))
                    .await
            }
        }
        TriggerSource::MentionAgent => {
            state
                .tasks
                .enqueue_task_for_mention(issue, trigger.agent.id, Some(comment_id))
                .await
        }
        TriggerSource::MentionSquadLeader => {
            state
                .tasks
                .enqueue_task_for_squad_leader(
                    issue,
                    trigger.agent.id,
                    trigger
                        .squad_id
                        .expect("squad mention trigger always carries a squad"),
                    Some(comment_id),
                )
                .await
        }
        TriggerSource::ThreadParent => {
            state
                .tasks
                .enqueue_task_for_thread_parent(issue, trigger.agent.id, Some(comment_id))
                .await
        }
        TriggerSource::Conversation => {
            if let Some(squad_id) = trigger.squad_id {
                state
                    .tasks
                    .enqueue_task_for_squad_leader(
                        issue,
                        trigger.agent.id,
                        squad_id,
                        Some(comment_id),
                    )
                    .await
            } else {
                state
                    .tasks
                    .enqueue_task_for_thread_parent(issue, trigger.agent.id, Some(comment_id))
                    .await
            }
        }
    }
}

async fn enqueue_trigger(
    state: &HandlerState,
    issue: &Issue,
    trigger: &Trigger,
    comment_id: Uuid,
) -> EnqueueResult {
    // A duplicate insert is resolved by an atomic same-head merge or a
    // claim-receipt planned id. Never report success for a merge that did not
    // complete; bounded retries make sustained concurrent churn visible.
    let mut pending = trigger.already_pending;
    let mut lost_race = false;
    for _ in 0..4 {
        if pending {
            match merge_into_pending(state, issue, trigger, comment_id).await {
                Ok(true) => {
                    return EnqueueResult {
                        status: DispatchStatus::Coalesced,
                        reason: ReasonCode::Coalesced,
                        execution_squad_id: trigger.squad_id,
                        task_id: None,
                    };
                }
                Err(reason) => {
                    return EnqueueResult {
                        status: DispatchStatus::Blocked,
                        reason,
                        execution_squad_id: trigger.squad_id,
                        task_id: None,
                    };
                }
                Ok(false) if lost_race => {
                    match register_planned(state, issue, trigger, comment_id).await {
                        Ok(true) => {
                            return EnqueueResult {
                                status: DispatchStatus::Deferred,
                                reason: ReasonCode::Deferred,
                                execution_squad_id: trigger.squad_id,
                                task_id: None,
                            };
                        }
                        Err(reason) => {
                            return EnqueueResult {
                                status: DispatchStatus::Blocked,
                                reason,
                                execution_squad_id: trigger.squad_id,
                                task_id: None,
                            };
                        }
                        Ok(false) => {}
                    }
                }
                Ok(false) => {
                    match agent::has_active_task_for_issue_and_agent(
                        &state.pool,
                        issue.id,
                        trigger.agent.id,
                    )
                    .await
                    {
                        Ok(Some(true)) => {
                            return EnqueueResult {
                                status: DispatchStatus::Deferred,
                                reason: ReasonCode::Deferred,
                                execution_squad_id: trigger.squad_id,
                                task_id: None,
                            };
                        }
                        Ok(Some(false)) | Ok(None) => {}
                        Err(_) => {
                            return EnqueueResult {
                                status: DispatchStatus::Blocked,
                                reason: ReasonCode::InternalError,
                                execution_squad_id: trigger.squad_id,
                                task_id: None,
                            };
                        }
                    }
                }
            }
        }

        match enqueue_fresh_trigger(state, issue, trigger, comment_id).await {
            Ok(task) => {
                return EnqueueResult {
                    status: DispatchStatus::Queued,
                    reason: ReasonCode::Queued,
                    execution_squad_id: trigger.squad_id,
                    task_id: Some(task.id),
                };
            }
            Err(error) if pending_slot_taken_err(&error) => {
                pending = true;
                lost_race = true;
            }
            Err(error) => {
                return EnqueueResult {
                    status: DispatchStatus::Blocked,
                    reason: enqueue_failure_reason(&error),
                    execution_squad_id: trigger.squad_id,
                    task_id: None,
                };
            }
        }
    }
    EnqueueResult {
        status: DispatchStatus::Blocked,
        reason: ReasonCode::InternalError,
        execution_squad_id: trigger.squad_id,
        task_id: None,
    }
}

async fn schedule_escalation(
    state: &HandlerState,
    issue: &Issue,
    trigger: &Trigger,
    comment_id: Uuid,
    result: &EnqueueResult,
) {
    if !trigger.source.schedules_escalation()
        || result.status != DispatchStatus::Queued
        || result.task_id.is_none()
    {
        return;
    }
    let Some(fallback) = trigger.escalation_fallback.as_ref() else {
        return;
    };
    let delay = comment_routing_escalation_delay(state, issue.workspace_id).await;
    if delay <= Duration::zero() {
        return;
    }
    let Some(fire_at) = Utc::now().checked_add_signed(delay) else {
        tracing::warn!(
            issue_id = %issue.id,
            agent_id = %trigger.agent.id,
            "comment routing escalation delay overflowed"
        );
        return;
    };
    if let Err(error) = state
        .tasks
        .enqueue_deferred_assignee_fallback(
            issue,
            fallback.agent.id,
            fallback.squad_id,
            result.task_id.expect("checked above"),
            Some(comment_id),
            fire_at,
        )
        .await
    {
        // The primary route is already queued. Escalation is deliberately
        // best-effort and must not turn a successful primary dispatch into a
        // false failure, matching the Go handler contract.
        tracing::warn!(
            %error,
            issue_id = %issue.id,
            primary_agent_id = %trigger.agent.id,
            fallback_agent_id = %fallback.agent.id,
            "failed to enqueue deferred comment routing fallback"
        );
    }
}

async fn dispatch_triggers(
    state: &HandlerState,
    issue: &Issue,
    comment_id: Uuid,
    triggers: &[Trigger],
) -> HashMap<Uuid, EnqueueResult> {
    let mut results = HashMap::with_capacity(triggers.len());
    for trigger in triggers {
        let result = enqueue_trigger(state, issue, trigger, comment_id).await;
        schedule_escalation(state, issue, trigger, comment_id, &result).await;
        results.insert(trigger.agent.id, result);
    }
    results
}

fn outcomes_for_targets(
    targets: Vec<Target>,
    results: &HashMap<Uuid, EnqueueResult>,
) -> Vec<CommentTriggerOutcome> {
    targets
        .into_iter()
        .filter_map(|target| {
            let (status, reason) = match target.terminal {
                Some(result) => result,
                None => {
                    let result = results.get(&target.agent_id?)?;
                    let mut status = result.status;
                    let mut reason = result.reason;
                    // A shared leader task carries one squad's briefing. A
                    // second squad naming that same leader is still handled,
                    // but only by coalescing into the shared run.
                    if target.target_type == "squad"
                        && result.execution_squad_id.is_some()
                        && result.execution_squad_id.map(|id| id.to_string())
                            != Some(target.target_id.clone())
                        && status == DispatchStatus::Queued
                    {
                        status = DispatchStatus::Coalesced;
                        reason = ReasonCode::Coalesced;
                    }
                    (status, reason)
                }
            };
            Some(CommentTriggerOutcome {
                target_type: target.target_type,
                target_id: target.target_id,
                status,
                reason_code: reason,
            })
        })
        .collect()
}

fn trigger_reason(source: TriggerSource) -> &'static str {
    match source {
        TriggerSource::IssueAssignee => "Current issue assignment will trigger this agent.",
        TriggerSource::MentionAgent => "This agent was mentioned in the comment.",
        TriggerSource::MentionSquadLeader => "A mentioned squad will trigger its leader.",
        TriggerSource::ThreadParent => "This reply will trigger the parent comment's author.",
        TriggerSource::Conversation => {
            "This follow-up will continue the recent agent conversation."
        }
    }
}

pub(crate) async fn trigger_comment(
    state: &HandlerState,
    issue: &Issue,
    comment: &Comment,
    parent: Option<&Comment>,
    actor_type: &str,
    actor_id: Uuid,
    originator_user_id: Option<Uuid>,
    suppressed_agent_ids: &[Uuid],
) -> Vec<CommentTriggerOutcome> {
    let suppressed = suppressed_agent_ids.iter().copied().collect();
    let (triggers, targets) = resolve_comment_triggers(
        state,
        issue,
        &comment.content,
        parent,
        actor_type,
        actor_id,
        originator_user_id,
        Some(comment.id),
        &suppressed,
    )
    .await;
    let results = dispatch_triggers(state, issue, comment.id, &triggers).await;
    outcomes_for_targets(targets, &results)
}

pub(crate) async fn preview_comment_triggers(
    state: &HandlerState,
    issue: &Issue,
    content: &str,
    parent: Option<&Comment>,
    actor_type: &str,
    actor_id: Uuid,
    originator_user_id: Option<Uuid>,
    exclude_trigger_comment_id: Option<Uuid>,
) -> PreviewResponse {
    let (triggers, targets) = resolve_comment_triggers(
        state,
        issue,
        content,
        parent,
        actor_type,
        actor_id,
        originator_user_id,
        exclude_trigger_comment_id,
        &HashSet::new(),
    )
    .await;
    let agents = triggers
        .into_iter()
        .map(|trigger| PreviewAgent {
            id: trigger.agent.id,
            name: trigger.agent.name,
            avatar_url: trigger.agent.avatar_url,
            source: trigger.source.as_str().to_string(),
            reason: trigger_reason(trigger.source).to_string(),
        })
        .collect();
    let blocked = targets
        .into_iter()
        .filter_map(|target| {
            let (status, reason) = target.terminal?;
            (status == DispatchStatus::Blocked).then_some(CommentTriggerOutcome {
                target_type: target.target_type,
                target_id: target.target_id,
                status,
                reason_code: reason,
            })
        })
        .collect();
    PreviewResponse { agents, blocked }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_matches_go_shape_and_deduplicates_targets() {
        let first = "018f946a-1234-7890-abcd-1234567890ab";
        let content = format!(
            "[@Agent [TF]](mention://agent/{first}) duplicate [@Agent](mention://agent/{first}) [@all](mention://all/all) [MUL-1](mention://issue/{first})"
        );
        let mentions = parse_mentions(&content);
        assert_eq!(mentions.len(), 3);
        assert_eq!(mentions[0].kind, "agent");
        assert_eq!(mentions[1].kind, "all");
        assert_eq!(mentions[2].kind, "issue");
    }

    #[test]
    fn note_comments_never_trigger() {
        assert!(is_note_comment(
            "/note [@Agent](mention://agent/018f946a-1234-7890-abcd-1234567890ab)"
        ));
        assert!(!is_note_comment(
            "please [@Agent](mention://agent/018f946a-1234-7890-abcd-1234567890ab)"
        ));
    }

    #[test]
    fn status_and_reason_use_stable_wire_values() {
        let outcome = CommentTriggerOutcome {
            target_type: "agent".to_string(),
            target_id: "a".to_string(),
            status: DispatchStatus::Blocked,
            reason_code: ReasonCode::InvocationNotAllowed,
        };
        let value = serde_json::to_value(outcome).unwrap();
        assert_eq!(value["status"], "blocked");
        assert_eq!(value["reason_code"], "invocation_not_allowed");
    }

    #[test]
    fn implicit_sources_match_go_contract() {
        assert_eq!(TriggerSource::IssueAssignee.as_str(), "issue_assignee");
        assert_eq!(TriggerSource::ThreadParent.as_str(), "thread_parent");
        assert_eq!(
            TriggerSource::Conversation.as_str(),
            "conversation_continuation"
        );
        assert_eq!(
            trigger_reason(TriggerSource::ThreadParent),
            "This reply will trigger the parent comment's author."
        );
    }

    #[test]
    fn only_routed_sources_schedule_escalation() {
        assert!(!TriggerSource::IssueAssignee.schedules_escalation());
        assert!(!TriggerSource::MentionAgent.schedules_escalation());
        assert!(TriggerSource::ThreadParent.schedules_escalation());
        assert!(TriggerSource::Conversation.schedules_escalation());
    }
}
