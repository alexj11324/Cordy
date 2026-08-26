//! Explicit comment-trigger routing.
//!
//! This is the handler-side port of the explicit `@agent` / `@squad` branch in
//! `server/internal/handler/comment.go` (MUL-4525).  It deliberately keeps
//! target resolution separate from execution: two named targets may resolve to
//! one agent, while the API still returns one outcome for every named target.

use crate::state::HandlerState;
use cordy_db::models::{Agent, Issue};
use cordy_db::queries::{agent, agent_invocation_target, member, squad};
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

#[derive(Debug, Clone)]
struct Trigger {
    agent: Agent,
    squad_id: Option<Uuid>,
    is_leader: bool,
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
        .attribution_for_merged_comment(issue.workspace_id, Some(comment_id), true, &trigger.agent)
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

async fn enqueue_trigger(
    state: &HandlerState,
    issue: &Issue,
    trigger: &Trigger,
    comment_id: Uuid,
) -> EnqueueResult {
    // A duplicate insert is resolved by an atomic same-head merge or a
    // claim-receipt planned id. Never report success for a merge that did not
    // complete; bounded retries make sustained concurrent churn visible.
    let mut lost_race = false;
    for _ in 0..4 {
        match state
            .tasks
            .enqueue_mention_task(
                issue,
                trigger.agent.id,
                Some(comment_id),
                Vec::new(),
                trigger.is_leader,
                trigger.squad_id,
                false,
                "",
                None,
                None,
            )
            .await
        {
            Ok(_) => {
                return EnqueueResult {
                    status: DispatchStatus::Queued,
                    reason: ReasonCode::Queued,
                    execution_squad_id: trigger.squad_id,
                };
            }
            Err(error) if pending_slot_taken_err(&error) => {
                match merge_into_pending(state, issue, trigger, comment_id).await {
                    Ok(true) => {
                        return EnqueueResult {
                            status: DispatchStatus::Coalesced,
                            reason: ReasonCode::Coalesced,
                            execution_squad_id: trigger.squad_id,
                        };
                    }
                    Err(reason) => {
                        return EnqueueResult {
                            status: DispatchStatus::Blocked,
                            reason,
                            execution_squad_id: trigger.squad_id,
                        };
                    }
                    Ok(false) if lost_race => {
                        match register_planned(state, issue, trigger, comment_id).await {
                            Ok(true) => {
                                return EnqueueResult {
                                    status: DispatchStatus::Deferred,
                                    reason: ReasonCode::Deferred,
                                    execution_squad_id: trigger.squad_id,
                                };
                            }
                            Err(reason) => {
                                return EnqueueResult {
                                    status: DispatchStatus::Blocked,
                                    reason,
                                    execution_squad_id: trigger.squad_id,
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
                                };
                            }
                            Ok(Some(false)) | Ok(None) => {}
                            Err(_) => {
                                return EnqueueResult {
                                    status: DispatchStatus::Blocked,
                                    reason: ReasonCode::InternalError,
                                    execution_squad_id: trigger.squad_id,
                                };
                            }
                        }
                    }
                }
                lost_race = true;
            }
            Err(error) => {
                return EnqueueResult {
                    status: DispatchStatus::Blocked,
                    reason: enqueue_failure_reason(&error),
                    execution_squad_id: trigger.squad_id,
                };
            }
        }
    }
    EnqueueResult {
        status: DispatchStatus::Blocked,
        reason: ReasonCode::InternalError,
        execution_squad_id: trigger.squad_id,
    }
}

pub(crate) async fn trigger_explicit_mentions(
    state: &HandlerState,
    issue: &Issue,
    content: &str,
    comment_id: Uuid,
    actor_type: &str,
    actor_id: Uuid,
    originator_user_id: Option<Uuid>,
    suppressed_agent_ids: &[Uuid],
) -> Vec<CommentTriggerOutcome> {
    let suppressed = suppressed_agent_ids.iter().copied().collect();
    let (triggers, targets) = resolve_explicit(
        state,
        issue,
        content,
        actor_type,
        actor_id,
        originator_user_id,
        &suppressed,
    )
    .await;
    let mut results = HashMap::new();
    for trigger in &triggers {
        results.insert(
            trigger.agent.id,
            enqueue_trigger(state, issue, trigger, comment_id).await,
        );
    }

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

pub(crate) async fn preview_explicit_mentions(
    state: &HandlerState,
    issue: &Issue,
    content: &str,
    actor_type: &str,
    actor_id: Uuid,
    originator_user_id: Option<Uuid>,
) -> PreviewResponse {
    let (triggers, targets) = resolve_explicit(
        state,
        issue,
        content,
        actor_type,
        actor_id,
        originator_user_id,
        &HashSet::new(),
    )
    .await;
    let agents = triggers
        .into_iter()
        .map(|trigger| PreviewAgent {
            id: trigger.agent.id,
            name: trigger.agent.name,
            avatar_url: trigger.agent.avatar_url,
            source: if trigger.is_leader {
                "mention_squad_leader".to_string()
            } else {
                "mention_agent".to_string()
            },
            reason: if trigger.is_leader {
                "A mentioned squad will trigger its leader.".to_string()
            } else {
                "This agent was mentioned in the comment.".to_string()
            },
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
}
