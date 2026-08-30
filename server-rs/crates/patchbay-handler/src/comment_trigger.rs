//! Comment agent-trigger computation.
//!
//! Implements `computeCommentAgentTriggers`, `isNoteComment`, and
//! `retriggerCancelledTaskSurvivors`
//! without the full coalesce/defer enqueue machine. The P1 contract is: wake
//! the right agents (mentions, assignee, thread parent, conversation, `/note`
//! opt-out) and restore surviving coalesced comments after cancel.

use std::collections::{HashMap, HashSet};

use axum::http::HeaderMap;
use patchbay_db::models::{Agent, AgentTaskQueue, Comment, Issue};
use patchbay_db::queries::{agent, autopilot, comment, team};
use patchbay_middleware::workspace::WorkspaceContext;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::state::HandlerState;
use patchbay_service::task_service::SideChatSeed;

const NOTE_COMMENT_PREFIX: &str = "/note";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommentTriggerSource {
    IssueAssignee,
    MentionAgent,
    MentionTeamLeader,
    ThreadParent,
    Conversation,
}

impl CommentTriggerSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::IssueAssignee => "issue_assignee",
            Self::MentionAgent => "mention_agent",
            Self::MentionTeamLeader => "mention_team_leader",
            Self::ThreadParent => "thread_parent",
            Self::Conversation => "conversation_continuation",
        }
    }

    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::IssueAssignee => "Current issue assignment will trigger this agent.",
            Self::MentionAgent => "This agent was mentioned in the comment.",
            Self::MentionTeamLeader => "A mentioned team will trigger its leader.",
            Self::ThreadParent => "This reply will trigger the parent comment's author.",
            Self::Conversation => "This follow-up will continue the recent agent conversation.",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CommentTrigger {
    pub agent: Agent,
    pub team_id: Option<Uuid>,
    pub source: CommentTriggerSource,
}

#[derive(Debug, Clone)]
pub(crate) struct BlockedMention {
    pub target_type: &'static str,
    pub target_id: Uuid,
    pub reason_code: &'static str,
}

#[derive(Debug, Default)]
pub(crate) struct CommentTriggerPlan {
    pub triggers: Vec<CommentTrigger>,
    pub blocked: Vec<BlockedMention>,
}

pub(crate) struct CommentTriggerInput<'a> {
    pub state: &'a HandlerState,
    pub issue: &'a Issue,
    pub content: &'a str,
    pub parent: Option<&'a Comment>,
    pub actor_type: &'a str,
    pub actor_id: Uuid,
    pub originator_user_id: Option<Uuid>,
    pub exclude_trigger_comment_id: Option<Uuid>,
}

/// `/note` as the first token opts the comment out of agent triggering.
pub(crate) fn is_note_comment(content: &str) -> bool {
    let trimmed = content.trim_start_matches([' ', '\t', '\r', '\n']);
    let first_token = trimmed
        .split(|character: char| character.is_whitespace())
        .next()
        .unwrap_or_default();
    first_token.eq_ignore_ascii_case(NOTE_COMMENT_PREFIX)
}

pub(crate) fn mention_ids(content: &str, kind: &str) -> Vec<Uuid> {
    let needle = format!("mention://{kind}/");
    let mut ids = Vec::new();
    let mut rest = content;
    while let Some(index) = rest.find(&needle) {
        let tail = &rest[index + needle.len()..];
        let raw = tail
            .split(|character: char| !character.is_ascii_hexdigit() && character != '-')
            .next()
            .unwrap_or_default();
        if let Ok(id) = Uuid::parse_str(raw) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        rest = &tail[raw.len()..];
    }
    ids
}

pub(crate) fn has_all_mention(content: &str) -> bool {
    content.contains("mention://all/")
}

pub(crate) async fn invoke_originator(
    state: &HandlerState,
    headers: &HeaderMap,
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
    let task_id = task_id.or_else(|| {
        headers
            .get("x-task-id")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| Uuid::parse_str(value).ok())
    })?;
    agent::get_agent_task(&state.pool, task_id)
        .await
        .ok()
        .flatten()
        .and_then(|task| task.originator_user_id)
}

pub(crate) async fn autopilot_delegation_authority(
    state: &HandlerState,
    issue: &Issue,
    actor_type: &str,
    actor_id: Uuid,
    task_id: Option<Uuid>,
) -> Option<Uuid> {
    if actor_type != "agent" {
        return None;
    }
    if issue.origin_type.as_deref() != Some("autopilot") {
        return None;
    }
    let origin_id = issue.origin_id?;
    let task_id = task_id?;
    let task = agent::get_agent_task(&state.pool, task_id)
        .await
        .ok()
        .flatten()?;
    if task.agent_id != actor_id || task.issue_id != Some(issue.id) {
        return None;
    }
    let autopilot =
        autopilot::get_autopilot_in_workspace(&state.pool, origin_id, issue.workspace_id)
            .await
            .ok()
            .flatten()?;
    if autopilot.created_by_type != "member" {
        return None;
    }
    Some(autopilot.created_by_id)
}

pub(crate) async fn effective_invoker(
    state: &HandlerState,
    issue: &Issue,
    headers: &HeaderMap,
    actor_type: &str,
    actor_id: Uuid,
    task_id: Option<Uuid>,
) -> Option<Uuid> {
    if let Some(originator) = invoke_originator(state, headers, actor_type, actor_id, task_id).await
    {
        return Some(originator);
    }
    autopilot_delegation_authority(state, issue, actor_type, actor_id, task_id).await
}

pub(crate) async fn compute_comment_agent_triggers(
    input: CommentTriggerInput<'_>,
) -> CommentTriggerPlan {
    if is_note_comment(input.content) {
        return CommentTriggerPlan::default();
    }
    if !mention_ids(input.content, "agent").is_empty()
        || !mention_ids(input.content, "team").is_empty()
    {
        return resolve_mentioned_triggers(input).await;
    }
    if has_all_mention(input.content) || !mention_ids(input.content, "member").is_empty() {
        return CommentTriggerPlan::default();
    }
    if input.actor_type != "member" {
        if input.issue.assignee_type.as_deref() == Some("team") {
            if let Some(trigger) = route_assigned_team_leader(&input).await {
                return CommentTriggerPlan {
                    triggers: vec![trigger],
                    blocked: Vec::new(),
                };
            }
        }
        return CommentTriggerPlan::default();
    }
    if let Some(parent) = input.parent {
        if parent.author_type == "agent" {
            if let Some(trigger) = route_reply_to_parent_author(&input, parent).await {
                return CommentTriggerPlan {
                    triggers: vec![trigger],
                    blocked: Vec::new(),
                };
            }
            return CommentTriggerPlan::default();
        }
        if let Some(triggers) = route_thread_root_owners(&input, parent).await {
            return CommentTriggerPlan {
                triggers,
                blocked: Vec::new(),
            };
        }
        if parent.author_type == "member" {
            return CommentTriggerPlan::default();
        }
    }
    if let Some(trigger) = route_assignee_fallback(&input).await {
        return CommentTriggerPlan {
            triggers: vec![trigger],
            blocked: Vec::new(),
        };
    }
    CommentTriggerPlan::default()
}

async fn resolve_mentioned_triggers(input: CommentTriggerInput<'_>) -> CommentTriggerPlan {
    let mut plan = CommentTriggerPlan::default();
    let mut seen = HashMap::<Uuid, usize>::new();
    let mut add = |plan: &mut CommentTriggerPlan, trigger: CommentTrigger| {
        if let Some(index) = seen.get(&trigger.agent.id).copied() {
            if plan.triggers[index].source != CommentTriggerSource::MentionTeamLeader
                && trigger.source == CommentTriggerSource::MentionTeamLeader
            {
                plan.triggers[index] = trigger;
            }
            return;
        }
        seen.insert(trigger.agent.id, plan.triggers.len());
        plan.triggers.push(trigger);
    };

    for agent_id in mention_ids(input.content, "agent") {
        match load_runnable_agent(input.state, input.issue.workspace_id, agent_id).await {
            Some(agent) => {
                if !crate::issue::can_invoke_agent(
                    input.state,
                    input.actor_type,
                    input.originator_user_id,
                    input.issue.workspace_id,
                    &agent,
                )
                .await
                {
                    plan.blocked.push(BlockedMention {
                        target_type: "agent",
                        target_id: agent_id,
                        reason_code: "invocation_not_allowed",
                    });
                    continue;
                }
                add(
                    &mut plan,
                    CommentTrigger {
                        agent,
                        team_id: None,
                        source: CommentTriggerSource::MentionAgent,
                    },
                );
            }
            None => plan.blocked.push(BlockedMention {
                target_type: "agent",
                target_id: agent_id,
                reason_code: "target_unavailable",
            }),
        }
    }

    for team_id in mention_ids(input.content, "team") {
        match team::get_team_in_workspace(&input.state.pool, team_id, input.issue.workspace_id)
            .await
        {
            Ok(Some(team)) if team.archived_at.is_none() => {
                match load_runnable_agent(input.state, input.issue.workspace_id, team.leader_id)
                    .await
                {
                    Some(agent) => {
                        if !crate::issue::can_invoke_agent(
                            input.state,
                            input.actor_type,
                            input.originator_user_id,
                            input.issue.workspace_id,
                            &agent,
                        )
                        .await
                        {
                            plan.blocked.push(BlockedMention {
                                target_type: "team",
                                target_id: team_id,
                                reason_code: "target_unavailable",
                            });
                            continue;
                        }
                        add(
                            &mut plan,
                            CommentTrigger {
                                agent,
                                team_id: Some(team.id),
                                source: CommentTriggerSource::MentionTeamLeader,
                            },
                        );
                    }
                    None => plan.blocked.push(BlockedMention {
                        target_type: "team",
                        target_id: team_id,
                        reason_code: "target_unavailable",
                    }),
                }
            }
            _ => plan.blocked.push(BlockedMention {
                target_type: "team",
                target_id: team_id,
                reason_code: "target_unavailable",
            }),
        }
    }
    plan
}

async fn load_runnable_agent(
    state: &HandlerState,
    workspace_id: Uuid,
    agent_id: Uuid,
) -> Option<Agent> {
    let agent = agent::get_agent_in_workspace(&state.pool, agent_id, workspace_id)
        .await
        .ok()
        .flatten()?;
    if agent.archived_at.is_some() || agent.runtime_id.is_none() {
        return None;
    }
    Some(agent)
}

async fn agent_allowed(input: &CommentTriggerInput<'_>, agent: &Agent) -> bool {
    crate::issue::can_invoke_agent(
        input.state,
        input.actor_type,
        input.originator_user_id,
        input.issue.workspace_id,
        agent,
    )
    .await
}

async fn route_reply_to_parent_author(
    input: &CommentTriggerInput<'_>,
    parent: &Comment,
) -> Option<CommentTrigger> {
    if parent.author_type != "agent" {
        return None;
    }
    let agent =
        load_runnable_agent(input.state, input.issue.workspace_id, parent.author_id).await?;
    if !agent_allowed(input, &agent).await {
        return None;
    }
    Some(CommentTrigger {
        agent,
        team_id: None,
        source: CommentTriggerSource::ThreadParent,
    })
}

async fn route_thread_root_owners(
    input: &CommentTriggerInput<'_>,
    parent: &Comment,
) -> Option<Vec<CommentTrigger>> {
    let root = comment::get_thread_root(&input.state.pool, parent.id, input.issue.workspace_id)
        .await
        .ok()
        .flatten()?;
    if root.author_type != "member" {
        return None;
    }
    if let Some(trigger) = route_first_explicit_root_mention(input, &root).await {
        return Some(vec![trigger]);
    }
    let tasks = agent::list_tasks_by_issue(&input.state.pool, input.issue.id)
        .await
        .ok()?;
    let mut routed = HashMap::<Uuid, Option<Uuid>>::new();
    for task in tasks {
        let Some(trigger_comment_id) = task.trigger_comment_id else {
            continue;
        };
        if input
            .exclude_trigger_comment_id
            .is_some_and(|id| id == trigger_comment_id)
        {
            continue;
        }
        if trigger_comment_id != root.id {
            continue;
        }
        routed.entry(task.agent_id).or_insert(task.team_id);
    }
    if routed.is_empty() {
        return None;
    }
    let mut triggers = Vec::new();
    for (agent_id, team_id) in routed {
        if let Some(trigger) = route_conversation_agent(input, agent_id, team_id).await {
            triggers.push(trigger);
        }
    }
    Some(triggers)
}

async fn route_first_explicit_root_mention(
    input: &CommentTriggerInput<'_>,
    root: &Comment,
) -> Option<CommentTrigger> {
    if let Some(agent_id) = mention_ids(&root.content, "agent").into_iter().next() {
        return route_conversation_agent(input, agent_id, None).await;
    }
    if let Some(team_id) = mention_ids(&root.content, "team").into_iter().next() {
        let team =
            team::get_team_in_workspace(&input.state.pool, team_id, input.issue.workspace_id)
                .await
                .ok()
                .flatten()?;
        return route_conversation_agent(input, team.leader_id, Some(team.id)).await;
    }
    None
}

async fn route_conversation_agent(
    input: &CommentTriggerInput<'_>,
    agent_id: Uuid,
    team_id: Option<Uuid>,
) -> Option<CommentTrigger> {
    let agent = load_runnable_agent(input.state, input.issue.workspace_id, agent_id).await?;
    if !agent_allowed(input, &agent).await {
        return None;
    }
    Some(CommentTrigger {
        agent,
        team_id,
        source: CommentTriggerSource::Conversation,
    })
}

async fn route_assignee_fallback(input: &CommentTriggerInput<'_>) -> Option<CommentTrigger> {
    match (
        input.issue.assignee_type.as_deref(),
        input.issue.assignee_id,
    ) {
        (Some("agent"), Some(agent_id)) => {
            let agent =
                load_runnable_agent(input.state, input.issue.workspace_id, agent_id).await?;
            if !agent_allowed(input, &agent).await {
                return None;
            }
            Some(CommentTrigger {
                agent,
                team_id: None,
                source: CommentTriggerSource::IssueAssignee,
            })
        }
        (Some("team"), Some(_)) => route_assigned_team_leader(input).await,
        _ => None,
    }
}

async fn route_assigned_team_leader(input: &CommentTriggerInput<'_>) -> Option<CommentTrigger> {
    let team_id = input.issue.assignee_id?;
    let team = team::get_team_in_workspace(&input.state.pool, team_id, input.issue.workspace_id)
        .await
        .ok()
        .flatten()?;
    if input.actor_type == "agent" && input.actor_id == team.leader_id {
        return None;
    }
    let agent = load_runnable_agent(input.state, input.issue.workspace_id, team.leader_id).await?;
    if !agent_allowed(input, &agent).await {
        return None;
    }
    Some(CommentTrigger {
        agent,
        team_id: Some(team.id),
        source: CommentTriggerSource::IssueAssignee,
    })
}

pub(crate) async fn enqueue_comment_triggers(
    state: &HandlerState,
    issue: &Issue,
    trigger_comment_id: Uuid,
    plan: &CommentTriggerPlan,
    suppressed: &[Uuid],
) -> Vec<Value> {
    let mut outcomes = Vec::new();
    for blocked in &plan.blocked {
        outcomes.push(json!({
            "agent_id": blocked.target_id,
            "target_type": blocked.target_type,
            "target_id": blocked.target_id,
            "status": "blocked",
            "reason": blocked.reason_code,
            "reason_code": blocked.reason_code,
        }));
    }
    for trigger in &plan.triggers {
        if suppressed.contains(&trigger.agent.id) {
            continue;
        }
        let outcome = enqueue_one(state, issue, trigger_comment_id, trigger).await;
        outcomes.push(outcome);
    }
    outcomes
}

async fn enqueue_one(
    state: &HandlerState,
    issue: &Issue,
    trigger_comment_id: Uuid,
    trigger: &CommentTrigger,
) -> Value {
    if trigger.source == CommentTriggerSource::MentionAgent {
        if let Ok(Some(active)) =
            agent::get_active_issue_agent_task(&state.pool, issue.id, trigger.agent.id).await
        {
            let root_comment_id =
                comment::get_thread_root(&state.pool, trigger_comment_id, issue.workspace_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|root| root.id)
                    .unwrap_or(trigger_comment_id);
            let result = state
                .tasks
                .enqueue_side_chat_for_mention(
                    issue,
                    trigger.agent.id,
                    trigger_comment_id,
                    SideChatSeed {
                        parent_task_id: active.id,
                        root_comment_id,
                    },
                )
                .await;
            return match result {
                Ok(task) => json!({
                    "agent_id": trigger.agent.id,
                    "target_type": "agent",
                    "target_id": trigger.agent.id,
                    "status": "side_chat",
                    "reason_code": "side_chat",
                    "task_id": task.id,
                    "parent_task_id": active.id,
                }),
                Err(error) => json!({
                    "agent_id": trigger.agent.id,
                    "target_type": "agent",
                    "target_id": trigger.agent.id,
                    "status": "blocked",
                    "reason": error.to_string(),
                    "reason_code": "enqueue_failed",
                }),
            };
        }
    }
    let result = match (trigger.source, trigger.team_id) {
        (CommentTriggerSource::MentionTeamLeader, Some(team_id)) => {
            state
                .tasks
                .enqueue_task_for_team_leader_without_owner_context(
                    issue,
                    trigger.agent.id,
                    team_id,
                    Some(trigger_comment_id),
                )
                .await
        }
        (CommentTriggerSource::IssueAssignee, Some(team_id)) => {
            state
                .tasks
                .enqueue_task_for_team_leader(
                    issue,
                    trigger.agent.id,
                    team_id,
                    Some(trigger_comment_id),
                )
                .await
        }
        (CommentTriggerSource::ThreadParent, _) => {
            state
                .tasks
                .enqueue_task_for_thread_parent(issue, trigger.agent.id, Some(trigger_comment_id))
                .await
        }
        _ => {
            state
                .tasks
                .enqueue_task_for_mention(issue, trigger.agent.id, Some(trigger_comment_id))
                .await
        }
    };
    match result {
        Ok(task) => json!({
            "agent_id": trigger.agent.id,
            "target_type": "agent",
            "target_id": trigger.agent.id,
            "status": "queued",
            "reason_code": "queued",
            "task_id": task.id,
        }),
        Err(error) => {
            json!({
                "agent_id": trigger.agent.id,
                "target_type": "agent",
                "target_id": trigger.agent.id,
                "status": "blocked",
                "reason": error.to_string(),
                "reason_code": "enqueue_failed",
            })
        }
    }
}

pub(crate) fn preview_agents(plan: &CommentTriggerPlan) -> Vec<Value> {
    plan.triggers
        .iter()
        .map(|trigger| {
            json!({
                "id": trigger.agent.id,
                "name": trigger.agent.name,
                "avatar_url": trigger.agent.avatar_url,
                "source": trigger.source.as_str(),
                "reason": trigger.source.reason(),
            })
        })
        .collect()
}

pub(crate) fn preview_blocked(plan: &CommentTriggerPlan) -> Vec<Value> {
    plan.blocked
        .iter()
        .map(|blocked| {
            json!({
                "target_type": blocked.target_type,
                "target_id": blocked.target_id,
                "reason_code": blocked.reason_code,
            })
        })
        .collect()
}

pub(crate) async fn retrigger_cancelled_task_survivors(
    state: &HandlerState,
    issue: &Issue,
    cancelled: &[AgentTaskQueue],
    excluded_comment_id: Option<Uuid>,
) {
    if cancelled.is_empty() {
        return;
    }
    let mut targets_by_comment: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();
    for task in cancelled {
        let mut planned = task.coalesced_comment_ids.clone();
        if let Some(trigger_comment_id) = task.trigger_comment_id {
            planned.push(trigger_comment_id);
        }
        for comment_id in planned {
            if excluded_comment_id == Some(comment_id) {
                continue;
            }
            targets_by_comment
                .entry(comment_id)
                .or_default()
                .insert(task.agent_id);
        }
    }
    let mut comments = Vec::new();
    for comment_id in targets_by_comment.keys().copied() {
        let Ok(Some(row)) = comment::get_comment(&state.pool, comment_id).await else {
            continue;
        };
        if row.issue_id != issue.id {
            continue;
        }
        comments.push(row);
    }
    comments.sort_by_key(|row| (row.created_at, row.id));
    for row in comments {
        if is_note_comment(&row.content) {
            continue;
        }
        let parent = match row.parent_id {
            Some(parent_id) => comment::get_comment(&state.pool, parent_id)
                .await
                .ok()
                .flatten(),
            None => None,
        };
        let originator_user_id = if row.author_type == "member" {
            Some(row.author_id)
        } else {
            invoke_originator(
                state,
                &HeaderMap::new(),
                &row.author_type,
                row.author_id,
                row.source_task_id,
            )
            .await
            .or(autopilot_delegation_authority(
                state,
                issue,
                &row.author_type,
                row.author_id,
                row.source_task_id,
            )
            .await)
        };
        let plan = compute_comment_agent_triggers(CommentTriggerInput {
            state,
            issue,
            content: &row.content,
            parent: parent.as_ref(),
            actor_type: &row.author_type,
            actor_id: row.author_id,
            originator_user_id,
            exclude_trigger_comment_id: Some(row.id),
        })
        .await;
        let Some(allowed) = targets_by_comment.get(&row.id) else {
            continue;
        };
        let scoped = CommentTriggerPlan {
            triggers: plan
                .triggers
                .into_iter()
                .filter(|trigger| allowed.contains(&trigger.agent.id))
                .collect(),
            blocked: Vec::new(),
        };
        if !scoped.triggers.is_empty() {
            let _ = enqueue_comment_triggers(state, issue, row.id, &scoped, &[]).await;
        }
    }
}

pub(crate) async fn load_parent_comment(
    state: &HandlerState,
    issue: &Issue,
    parent_id: Option<Uuid>,
) -> Option<Comment> {
    let parent_id = parent_id?;
    comment::get_comment_in_workspace(&state.pool, parent_id, issue.workspace_id)
        .await
        .ok()
        .flatten()
        .filter(|parent| parent.issue_id == issue.id)
}

/// Shared actor + originator resolution for comment trigger paths.
pub(crate) async fn trigger_actor(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    issue: &Issue,
) -> (String, Uuid, Option<Uuid>, Option<Uuid>) {
    let (actor_type, actor_id, task_id) =
        crate::issue::mutation_actor(state, context, headers).await;
    let originator = effective_invoker(state, issue, headers, &actor_type, actor_id, task_id).await;
    (actor_type, actor_id, task_id, originator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_prefix_matches_first_token_only() {
        assert!(is_note_comment("/note"));
        assert!(is_note_comment("  /NOTE check expiry"));
        assert!(is_note_comment("/note\nbody"));
        assert!(!is_note_comment("/notes"));
        assert!(!is_note_comment("/ note"));
        assert!(!is_note_comment("see foo/note"));
        assert!(!is_note_comment(
            "[@bot](mention://agent/018f946a-1234-7890-abcd-1234567890ab)"
        ));
    }

    #[test]
    fn mention_ids_dedupe_and_ignore_other_kinds() {
        let first = Uuid::parse_str("018f946a-1234-7890-abcd-1234567890ab").unwrap();
        let second = Uuid::parse_str("018f946a-2234-7890-abcd-1234567890ab").unwrap();
        let content = format!(
            "a\0 [@one](mention://agent/{first}) duplicate mention://agent/{first} and mention://agent/{second})"
        );
        assert_eq!(mention_ids(&content, "agent"), vec![first, second]);
        assert!(mention_ids(&content, "team").is_empty());
        assert!(mention_ids(&content, "member").is_empty());
    }

    #[test]
    fn all_and_member_mentions_are_detectable() {
        let member = Uuid::parse_str("018f946a-3234-7890-abcd-1234567890ab").unwrap();
        assert!(has_all_mention("[@all](mention://all/all) heads up"));
        assert!(!has_all_mention(
            "[@bot](mention://agent/018f946a-1234-7890-abcd-1234567890ab)"
        ));
        assert_eq!(
            mention_ids(&format!("[@Member](mention://member/{member})"), "member"),
            vec![member]
        );
    }

    #[test]
    fn trigger_source_wire_names_match_go() {
        assert_eq!(
            CommentTriggerSource::IssueAssignee.as_str(),
            "issue_assignee"
        );
        assert_eq!(CommentTriggerSource::MentionAgent.as_str(), "mention_agent");
        assert_eq!(
            CommentTriggerSource::MentionTeamLeader.as_str(),
            "mention_team_leader"
        );
        assert_eq!(CommentTriggerSource::ThreadParent.as_str(), "thread_parent");
        assert_eq!(
            CommentTriggerSource::Conversation.as_str(),
            "conversation_continuation"
        );
    }
}
