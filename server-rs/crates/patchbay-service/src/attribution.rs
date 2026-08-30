//! Accountable-human resolution contract for agent task runs — port of
//! Human-attribution contract (PB-4302).
//!
//! Every run enqueued into agent_task_queue must be traceable to exactly one
//! accountable human, and the attribution must be EXPLAINABLE: it records not
//! just who, but at which waterfall level the human was resolved.
//!
//! This module owns the vocabulary ([`Source`], [`EvidenceKind`],
//! [`TriggerKind`]) and the PURE classification rules. The database reads stay
//! in the caller; already-fetched facts are passed into the classify functions
//! so the rules remain side-effect-free and unit-testable without a database.
//!
//! Hard invariant (PB-4302 §1.3): attribution is "on behalf of", never blame
//! and never authorization. Nothing here is consulted for permission decisions.

use uuid::Uuid;

/// The waterfall level that resolved the accountable human for a run. Stored
/// verbatim in `agent_task_queue.originator_source`. Free strings on the Go
/// side (no DB CHECK) so a new trigger path can introduce a source without a
/// schema migration — hence an open string-backed type, not a closed enum.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Source(pub String);

impl Source {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A member's own action enqueued the run. The member IS the accountable
    /// human.
    pub fn direct_human() -> Source {
        Source("direct_human".to_string())
    }
    /// An agent running on behalf of a human caused the enqueue. The parent
    /// task's accountable human is COPIED, not chained, so delegation cycles
    /// stay harmless (PB-4302 §3.2).
    pub fn delegation() -> Source {
        Source("delegation".to_string())
    }
    /// The issue's standing assignee reacted to an agent/system-authored
    /// comment; the human is resolved through comment.source_task_id
    /// (PB-4302 §3.3).
    pub fn comment_source() -> Source {
        Source("comment_source".to_string())
    }
    /// An automation schedule/webhook trigger fired; the accountable human is
    /// the member who CREATED that trigger. Preferred over [`Source::rule_owner`]
    /// (PB-4302; Bohan's refinement). originator stays NULL — authz-safe
    /// audit-only divergence.
    pub fn trigger_owner() -> Source {
        Source("trigger_owner".to_string())
    }
    /// Automation trigger whose creator is not recoverable; degrades to the
    /// publisher of the rule's active version (PB-4302 §3.4).
    pub fn rule_owner() -> Source {
        Source("rule_owner".to_string())
    }
    /// Nothing above resolved a human; degrades to the agent owner. DEGRADED,
    /// not compliance-grade (PB-4302 §3.5).
    pub fn owner_fallback() -> Source {
        Source("owner_fallback".to_string())
    }
    /// A historical row attributed after the fact by the backfill command.
    pub fn backfill() -> Source {
        Source("backfill".to_string())
    }
    /// No human resolved and no fallback applied — an explicit "we looked and
    /// found no human in the chain" marker, distinct from a pre-migration NULL.
    pub fn unattributed() -> Source {
        Source("unattributed".to_string())
    }

    /// True when src is compliance-grade (non-degraded). OWNER_FALLBACK,
    /// BACKFILL, and UNATTRIBUTED count against the attribution-coverage
    /// health metric (PB-4302 §9).
    pub fn precise(&self) -> bool {
        matches!(
            self.as_str(),
            "direct_human" | "delegation" | "comment_source" | "trigger_owner" | "rule_owner"
        )
    }
}

/// Tags the direct cause of a run so every attribution can jump to its
/// evidence row. Free strings paired with an evidence ref id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EvidenceKind(pub String);

impl EvidenceKind {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn evidence_comment() -> EvidenceKind {
    EvidenceKind("comment".to_string())
}
pub fn evidence_issue_assignment() -> EvidenceKind {
    EvidenceKind("issue_assignment".to_string())
}
pub fn evidence_automation_run() -> EvidenceKind {
    EvidenceKind("automation_run".to_string())
}
pub fn evidence_rule_version() -> EvidenceKind {
    EvidenceKind("rule_version".to_string())
}
pub fn evidence_rerun() -> EvidenceKind {
    EvidenceKind("rerun".to_string())
}
/// Points at the terminal worker task that handed control back to its source
/// coordinator.
pub fn evidence_delegated_failure() -> EvidenceKind {
    EvidenceKind("delegated_failure".to_string())
}
/// Points the uniform evidence pair at the chat session that triggered the
/// run — the chat analogue of automation_run/issue_assignment (PB-4302 §2).
pub fn evidence_chat() -> EvidenceKind {
    EvidenceKind("chat".to_string())
}

/// Every path that can enqueue a run. An explicit taxonomy so adding a new
/// trigger path is a visible, deliberate change that has to declare its
/// attribution rule (no enqueue path may exist without one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriggerKind {
    MemberComment,
    MemberMention,
    MemberAssign,
    AgentMention,
    AgentComment,
    SubIssueCreate,
    StageWakeup,
    QuickCreate,
    Chat,
    AutomationSchedule,
    AutomationWebhook,
    AutomationManual,
    Retry,
    Rerun,
    DeferredFallback,
}

impl TriggerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TriggerKind::MemberComment => "member_comment",
            TriggerKind::MemberMention => "member_mention",
            TriggerKind::MemberAssign => "member_assign",
            TriggerKind::AgentMention => "agent_mention",
            TriggerKind::AgentComment => "agent_comment",
            TriggerKind::SubIssueCreate => "sub_issue_create",
            TriggerKind::StageWakeup => "stage_wakeup",
            TriggerKind::QuickCreate => "quick_create",
            TriggerKind::Chat => "chat",
            TriggerKind::AutomationSchedule => "automation_schedule",
            TriggerKind::AutomationWebhook => "automation_webhook",
            TriggerKind::AutomationManual => "automation_manual",
            TriggerKind::Retry => "retry",
            TriggerKind::Rerun => "rerun",
            TriggerKind::DeferredFallback => "deferred_fallback",
        }
    }
}

/// The attribution stamped onto a queued run.
///
/// - `user_id` is the AUTHORIZATION human written into originator_user_id;
///   legitimately None when no human authorized the run.
/// - `accountable_user_id` is the AUDIT human written into
///   accountable_user_id. One-way invariant (enforced by
///   [`finalize_attribution`]): when user_id is Some they are equal; when
///   None they may diverge — rule_owner names the rule publisher and
///   owner_fallback names the agent owner while authorization carries none.
///
/// Construct through [`classify_comment`] / [`classify_direct`] /
/// [`direct_human_run`] / [`unattributed`] / [`rule_owner`] so accountability
/// is always finalized; never stamp a hand-built literal onto the queue.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Result_ {
    pub user_id: Option<Uuid>,
    pub accountable_user_id: Option<Uuid>,
    pub source: Option<Source>,
    pub delegated_from_task_id: Option<Uuid>,
    pub rule_version_id: Option<Uuid>,
    pub retry_of_task_id: Option<Uuid>,
    pub rerun_of_task_id: Option<Uuid>,
    pub evidence_kind: Option<EvidenceKind>,
    pub evidence_ref_id: Option<Uuid>,
}

/// Enforces the one-way Phase 1 accountability invariant (PB-4302 §11):
/// `originator IS NOT NULL ⟹ accountable = originator`. When user_id is None
/// it leaves accountable exactly as the caller set it — the single divergence
/// point where rule_owner / owner_fallback name an accountable human for
/// audit while authorization correctly carries none. Every result flows
/// through here; enqueue call sites never special-case this.
fn finalize_attribution(mut r: Result_) -> Result_ {
    if r.user_id.is_some() {
        r.accountable_user_id = r.user_id;
    }
    r
}

/// Already-fetched facts about a trigger comment, gathered by the caller from
/// the DB so classification stays pure.
#[derive(Debug, Clone, Default)]
pub struct CommentFacts {
    pub comment_id: Option<Uuid>,
    /// "member" | "agent" | other
    pub author_type: String,
    pub author_id: Option<Uuid>,

    /// For agent-authored comments: the source task the comment was written
    /// from, and that task's originator_user_id (None when the source task is
    /// missing or itself unattributed).
    pub source_task_id: Option<Uuid>,
    pub parent_originator: Option<Uuid>,

    /// The source task's accountable_user_id (PB-4302 §3.2): lets an
    /// automation-rooted chain copy its responsible human down the delegation
    /// instead of dropping the chain root to unattributed.
    pub parent_accountable: Option<Uuid>,
}

/// Resolves attribution for a comment-triggered run. `agent_authored_source`
/// selects the label used when the trigger comment is agent-authored:
/// COMMENT_SOURCE for the issue-assignee-reacting path, DELEGATION for an
/// explicit mention / thread-parent / team-leader path. The returned user_id
/// is byte-identical to the legacy originator resolution so authorization
/// behavior is unchanged.
pub fn classify_comment(f: CommentFacts, agent_authored_source: Source) -> Result_ {
    match f.author_type.as_str() {
        "member" => finalize_attribution(Result_ {
            user_id: f.author_id,
            source: Some(Source::direct_human()),
            evidence_kind: Some(evidence_comment()),
            evidence_ref_id: f.comment_id,
            ..Default::default()
        }),
        "agent" => {
            let mut r = Result_ {
                evidence_kind: Some(evidence_comment()),
                evidence_ref_id: f.comment_id,
                ..Default::default()
            };
            let Some(source_task_id) = f.source_task_id else {
                // Agent comment with no source task: cannot walk the chain.
                r.source = Some(Source::unattributed());
                return finalize_attribution(r);
            };
            r.delegated_from_task_id = Some(source_task_id);
            if f.parent_originator.is_some() {
                r.user_id = f.parent_originator;
                r.source = Some(agent_authored_source);
            } else if f.parent_accountable.is_some() {
                // The parent had no authorizing human (automation-rooted chain)
                // but IS accountable to someone. Copy that down so the chain
                // root stays stable at any depth; originator stays NULL so
                // authorization is unchanged and a fail-closed workspace does
                // not reject a fan-out with a precise responsible human.
                r.accountable_user_id = f.parent_accountable;
                r.source = Some(agent_authored_source);
            } else {
                // Source task exists but has no human at its own top of chain.
                r.source = Some(Source::unattributed());
            }
            finalize_attribution(r)
        }
        _ => finalize_attribution(Result_ {
            source: Some(Source::unattributed()),
            evidence_kind: Some(evidence_comment()),
            evidence_ref_id: f.comment_id,
            ..Default::default()
        }),
    }
}

/// Facts for a run with no trigger comment: a direct issue assignment /
/// creation, or an agent-created issue with a quick-create origin.
#[derive(Debug, Clone, Default)]
pub struct DirectFacts {
    pub issue_id: Option<Uuid>,
    pub creator_type: String,
    pub creator_id: Option<Uuid>,

    /// The member who PERFORMED the action that enqueued this run. When valid
    /// it takes precedence over the issue creator: the person who acted, not
    /// whoever happened to file the issue, is on the hook (PB-4302 §4).
    pub actor_user_id: Option<Uuid>,

    /// An agent-created issue's provenance ("quick_create" or "agent_create");
    /// empty means none.
    pub origin_type: String,
    pub origin_task_id: Option<Uuid>,
    pub origin_originator: Option<Uuid>,

    /// The origin task's accountable_user_id — the DirectFacts analogue of
    /// CommentFacts.parent_accountable (PB-4302 §3.2).
    pub origin_accountable: Option<Uuid>,
}

/// Resolves attribution for a run with no trigger comment.
pub fn classify_direct(f: DirectFacts) -> Result_ {
    // A member who directly assigned/promoted the issue is the accountable
    // human, ahead of the issue's creator (PB-4302 §4).
    if f.actor_user_id.is_some() {
        return finalize_attribution(Result_ {
            user_id: f.actor_user_id,
            source: Some(Source::direct_human()),
            evidence_kind: Some(evidence_issue_assignment()),
            evidence_ref_id: f.issue_id,
            ..Default::default()
        });
    }
    if f.creator_type == "member" && f.creator_id.is_some() {
        return finalize_attribution(Result_ {
            user_id: f.creator_id,
            source: Some(Source::direct_human()),
            evidence_kind: Some(evidence_issue_assignment()),
            evidence_ref_id: f.issue_id,
            ..Default::default()
        });
    }
    match f.origin_type.as_str() {
        "quick_create" | "agent_create" => {
            let mut r = Result_ {
                delegated_from_task_id: f.origin_task_id,
                evidence_kind: Some(evidence_issue_assignment()),
                evidence_ref_id: f.issue_id,
                ..Default::default()
            };
            if f.origin_originator.is_some() {
                r.user_id = f.origin_originator;
                r.source = Some(Source::delegation());
            } else if f.origin_accountable.is_some() {
                // Automation-rooted origin task: copy accountable down; the
                // chain root stays stable, originator stays NULL (§3.2).
                r.accountable_user_id = f.origin_accountable;
                r.source = Some(Source::delegation());
            } else {
                r.source = Some(Source::unattributed());
            }
            finalize_attribution(r)
        }
        _ => finalize_attribution(Result_ {
            source: Some(Source::unattributed()),
            evidence_kind: Some(evidence_issue_assignment()),
            evidence_ref_id: f.issue_id,
            ..Default::default()
        }),
    }
}

/// Builds attribution for a run a member triggered directly through a path
/// that carries no issue and no trigger comment — a chat message or a
/// quick-create request. An invalid userID yields an explicit unattributed
/// result rather than a NULL-source bypass.
pub fn direct_human_run(
    user_id: Option<Uuid>,
    evidence_kind: EvidenceKind,
    evidence_ref_id: Option<Uuid>,
) -> Result_ {
    if user_id.is_none() {
        return finalize_attribution(Result_ {
            source: Some(Source::unattributed()),
            evidence_kind: Some(evidence_kind),
            evidence_ref_id,
            ..Default::default()
        });
    }
    finalize_attribution(Result_ {
        user_id,
        source: Some(Source::direct_human()),
        evidence_kind: Some(evidence_kind),
        evidence_ref_id,
        ..Default::default()
    })
}

/// Builds an explicit "no human resolved" result for an enqueue path that
/// currently carries no accountable human. Stamping UNATTRIBUTED with real
/// evidence keeps the row off the NULL-source bypass while leaving
/// originator/accountable None so authorization still says "no human".
pub fn unattributed(evidence_kind: EvidenceKind, evidence_ref_id: Option<Uuid>) -> Result_ {
    finalize_attribution(Result_ {
        source: Some(Source::unattributed()),
        evidence_kind: Some(evidence_kind),
        evidence_ref_id,
        ..Default::default()
    })
}

/// Builds attribution for an automation-triggered run keyed to the publisher
/// of the active rule version (PB-4302 §3.4). No human authorized the run:
/// user_id stays None; accountable is publisher_user_id — THE divergence the
/// two-column split exists for. A missing publisher degrades to unattributed
/// so we never fabricate a human.
pub fn rule_owner(
    publisher_user_id: Option<Uuid>,
    rule_version_id: Option<Uuid>,
    evidence_kind: EvidenceKind,
    evidence_ref_id: Option<Uuid>,
) -> Result_ {
    let mut r = Result_ {
        rule_version_id,
        evidence_kind: Some(evidence_kind),
        evidence_ref_id,
        ..Default::default()
    };
    if publisher_user_id.is_some() {
        r.source = Some(Source::rule_owner());
        r.accountable_user_id = publisher_user_id;
    } else {
        r.source = Some(Source::unattributed());
    }
    finalize_attribution(r)
}

/// Builds attribution for an automation schedule/webhook run keyed to the
/// human who created the firing trigger (Bohan's refinement). Like
/// [`rule_owner`], only the audit-accountable side is set. An invalid creator
/// degrades to unattributed so callers fall back to rule_owner rather than
/// fabricating a human.
pub fn trigger_owner(
    creator_user_id: Option<Uuid>,
    evidence_kind: EvidenceKind,
    evidence_ref_id: Option<Uuid>,
) -> Result_ {
    let mut r = Result_ {
        evidence_kind: Some(evidence_kind),
        evidence_ref_id,
        ..Default::default()
    };
    if creator_user_id.is_some() {
        r.source = Some(Source::trigger_owner());
        r.accountable_user_id = creator_user_id;
    } else {
        r.source = Some(Source::unattributed());
    }
    finalize_attribution(r)
}

/// Already-fetched facts about an agent-created issue, deciding who inherits
/// VISIBILITY of it (PB-5483).
#[derive(Debug, Clone, Default)]
pub struct SubscriptionFacts {
    /// Only agent-created issues can carry a delegated subscription.
    pub creator_type: String,
    pub origin_type: String,
    pub origin_originator: Option<Uuid>,
}

/// Resolves the human who should be auto-subscribed to an agent-created
/// issue, plus the reason label. Deliberately mirrors classify_direct's
/// origin branch rather than inventing a second notion of "whose behalf is
/// this" (the defect PB-5483 fixed was attribution and notification
/// disagreeing about exactly that).
///
/// - OriginOriginator valid → subscribe; resolves the ORIGINAL human at any
///   depth because attribution COPIES across hops rather than chaining. No
///   depth cap: depth is where a lost signal hurts most.
/// - quick_create → 'creator' (direct intent, full notifications);
///   agent_create → 'delegated' (agent's own decision under a broader
///   mandate, reduced tier).
/// - Anything else → none. origin_type='automation' excluded: an automation has
///   its own configured subscriber template. Degraded attribution excluded:
///   we do not fabricate a human to notify.
pub fn delegated_subscriber(f: SubscriptionFacts) -> Option<(Uuid, &'static str)> {
    if f.creator_type != "agent" {
        return None;
    }
    let originator = f.origin_originator?;
    match f.origin_type.as_str() {
        "quick_create" => Some((originator, "creator")),
        "agent_create" => Some((originator, "delegated")),
        _ => None,
    }
}

/// Degrades an UNATTRIBUTED result to OWNER_FALLBACK (PB-4302 §3.5): the
/// agent owner becomes the accountable human so no run is left without one,
/// but this is a DEGRADED label surfaced distinctly in reporting. Audit-only
/// — user_id stays None. Applied ONLY when the resolved source is
/// unattributed and the workspace has not opted into fail-closed; anything
/// else passes through unchanged so a human is never fabricated.
pub fn owner_fallback(r: Result_, owner_user_id: Option<Uuid>) -> Result_ {
    if r.source.as_ref().map(|s| s.as_str()) != Some("unattributed") || owner_user_id.is_none() {
        return r;
    }
    let mut r = r;
    r.source = Some(Source::owner_fallback());
    r.accountable_user_id = owner_user_id;
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn wire_values_match_go_constants() {
        assert_eq!(Source::direct_human().as_str(), "direct_human");
        assert_eq!(Source::delegation().as_str(), "delegation");
        assert_eq!(Source::comment_source().as_str(), "comment_source");
        assert_eq!(Source::trigger_owner().as_str(), "trigger_owner");
        assert_eq!(Source::rule_owner().as_str(), "rule_owner");
        assert_eq!(Source::owner_fallback().as_str(), "owner_fallback");
        assert_eq!(Source::backfill().as_str(), "backfill");
        assert_eq!(Source::unattributed().as_str(), "unattributed");
        assert_eq!(evidence_chat().as_str(), "chat");
        assert_eq!(evidence_delegated_failure().as_str(), "delegated_failure");
        assert_eq!(TriggerKind::AutomationWebhook.as_str(), "automation_webhook");
    }

    #[test]
    fn precise_excludes_degraded_sources() {
        for s in [
            Source::direct_human(),
            Source::delegation(),
            Source::comment_source(),
            Source::trigger_owner(),
            Source::rule_owner(),
        ] {
            assert!(s.precise(), "{}", s.as_str());
        }
        for s in [
            Source::owner_fallback(),
            Source::backfill(),
            Source::unattributed(),
        ] {
            assert!(!s.precise(), "{}", s.as_str());
        }
    }

    #[test]
    fn member_comment_is_direct_human_with_evidence() {
        let out = classify_comment(
            CommentFacts {
                comment_id: Some(id(1)),
                author_type: "member".into(),
                author_id: Some(id(2)),
                ..Default::default()
            },
            Source::delegation(),
        );
        assert_eq!(out.user_id, Some(id(2)));
        assert_eq!(out.accountable_user_id, Some(id(2)));
        assert_eq!(out.source, Some(Source::direct_human()));
        assert_eq!(out.evidence_ref_id, Some(id(1)));
    }

    #[test]
    fn agent_comment_walks_chain_preferring_originator_then_accountable() {
        // Parent originator present → copied as user, labeled by caller.
        let out = classify_comment(
            CommentFacts {
                comment_id: Some(id(1)),
                author_type: "agent".into(),
                source_task_id: Some(id(10)),
                parent_originator: Some(id(20)),
                parent_accountable: Some(id(21)),
                ..Default::default()
            },
            Source::delegation(),
        );
        assert_eq!(out.user_id, Some(id(20)));
        assert_eq!(out.accountable_user_id, Some(id(20)));
        assert_eq!(out.delegated_from_task_id, Some(id(10)));
        assert_eq!(out.source, Some(Source::delegation()));

        // Automation-rooted parent: no originator but accountable set →
        // accountable copied down, user stays None (authz unchanged).
        let out = classify_comment(
            CommentFacts {
                comment_id: Some(id(1)),
                author_type: "agent".into(),
                source_task_id: Some(id(10)),
                parent_originator: None,
                parent_accountable: Some(id(21)),
                ..Default::default()
            },
            Source::comment_source(),
        );
        assert_eq!(out.user_id, None);
        assert_eq!(out.accountable_user_id, Some(id(21)));
        assert_eq!(out.source, Some(Source::comment_source()));

        // Source task exists but no human anywhere up its chain.
        let out = classify_comment(
            CommentFacts {
                comment_id: Some(id(1)),
                author_type: "agent".into(),
                source_task_id: Some(id(10)),
                ..Default::default()
            },
            Source::delegation(),
        );
        assert_eq!(out.source, Some(Source::unattributed()));
        assert_eq!(out.user_id, None);

        // No source task at all.
        let out = classify_comment(
            CommentFacts {
                comment_id: Some(id(1)),
                author_type: "agent".into(),
                ..Default::default()
            },
            Source::delegation(),
        );
        assert_eq!(out.source, Some(Source::unattributed()));
    }

    #[test]
    fn unknown_author_type_lands_unattributed() {
        let out = classify_comment(
            CommentFacts {
                comment_id: Some(id(1)),
                author_type: "system".into(),
                ..Default::default()
            },
            Source::delegation(),
        );
        assert_eq!(out.source, Some(Source::unattributed()));
    }

    #[test]
    fn direct_actor_beats_creator() {
        let out = classify_direct(DirectFacts {
            issue_id: Some(id(100)),
            creator_type: "member".into(),
            creator_id: Some(id(2)),
            actor_user_id: Some(id(3)),
            ..Default::default()
        });
        assert_eq!(
            out.user_id,
            Some(id(3)),
            "actor outranks creator (PB-4302 §4)"
        );
        assert_eq!(out.evidence_ref_id, Some(id(100)));

        // No actor → member creator resolves.
        let out = classify_direct(DirectFacts {
            issue_id: Some(id(100)),
            creator_type: "member".into(),
            creator_id: Some(id(2)),
            ..Default::default()
        });
        assert_eq!(out.user_id, Some(id(2)));
    }

    #[test]
    fn direct_origin_branch_mirrors_comment_chain() {
        let out = classify_direct(DirectFacts {
            issue_id: Some(id(100)),
            origin_type: "quick_create".into(),
            origin_task_id: Some(id(10)),
            origin_originator: Some(id(30)),
            ..Default::default()
        });
        assert_eq!(out.user_id, Some(id(30)));
        assert_eq!(out.source, Some(Source::delegation()));

        let out = classify_direct(DirectFacts {
            issue_id: Some(id(100)),
            origin_type: "agent_create".into(),
            origin_task_id: Some(id(10)),
            origin_accountable: Some(id(31)),
            ..Default::default()
        });
        assert_eq!(out.user_id, None);
        assert_eq!(out.accountable_user_id, Some(id(31)));
        assert_eq!(out.source, Some(Source::delegation()));

        let out = classify_direct(DirectFacts {
            issue_id: Some(id(100)),
            origin_type: "automation".into(),
            ..Default::default()
        });
        assert_eq!(out.source, Some(Source::unattributed()));
    }

    #[test]
    fn rule_owner_diverges_accountable_from_authorization() {
        let out = rule_owner(
            Some(id(40)),
            Some(id(41)),
            evidence_automation_run(),
            Some(id(42)),
        );
        assert_eq!(out.user_id, None, "no human authorized the run");
        assert_eq!(out.accountable_user_id, Some(id(40)));
        assert_eq!(out.rule_version_id, Some(id(41)));
        assert_eq!(out.source, Some(Source::rule_owner()));

        // Missing publisher degrades to unattributed, never fabricated.
        let out = rule_owner(None, None, evidence_automation_run(), Some(id(42)));
        assert_eq!(out.source, Some(Source::unattributed()));
        assert_eq!(out.accountable_user_id, None);
    }

    #[test]
    fn trigger_owner_and_invalid_creator_fallback() {
        let out = trigger_owner(Some(id(50)), evidence_automation_run(), Some(id(51)));
        assert_eq!(out.user_id, None);
        assert_eq!(out.accountable_user_id, Some(id(50)));
        assert_eq!(out.source, Some(Source::trigger_owner()));

        let out = trigger_owner(None, evidence_automation_run(), Some(id(51)));
        assert_eq!(out.source, Some(Source::unattributed()));
    }

    #[test]
    fn direct_human_run_none_yields_explicit_unattributed() {
        let out = direct_human_run(None, evidence_chat(), Some(id(60)));
        assert_eq!(out.source, Some(Source::unattributed()));
        assert_eq!(out.evidence_ref_id, Some(id(60)));

        let out = direct_human_run(Some(id(61)), evidence_chat(), Some(id(60)));
        assert_eq!(out.source, Some(Source::direct_human()));
        assert_eq!(out.accountable_user_id, Some(id(61)));
    }

    #[test]
    fn owner_fallback_only_from_unattributed_with_valid_owner() {
        let base = unattributed(evidence_comment(), Some(id(1)));
        let out = owner_fallback(base, Some(id(70)));
        assert_eq!(out.source, Some(Source::owner_fallback()));
        assert_eq!(out.accountable_user_id, Some(id(70)));
        assert_eq!(out.user_id, None, "audit-only: authorization untouched");

        // Precise results pass through unchanged.
        let precise = direct_human_run(Some(id(2)), evidence_chat(), Some(id(1)));
        let out = owner_fallback(precise.clone(), Some(id(70)));
        assert_eq!(out, precise);

        // No owner to fall back to → unchanged.
        let base = unattributed(evidence_comment(), Some(id(1)));
        let out = owner_fallback(base.clone(), None);
        assert_eq!(out, base);
    }

    #[test]
    fn delegated_subscriber_waterfall() {
        assert_eq!(
            delegated_subscriber(SubscriptionFacts {
                creator_type: "agent".into(),
                origin_type: "quick_create".into(),
                origin_originator: Some(id(80)),
            }),
            Some((id(80), "creator"))
        );
        assert_eq!(
            delegated_subscriber(SubscriptionFacts {
                creator_type: "agent".into(),
                origin_type: "agent_create".into(),
                origin_originator: Some(id(80)),
            }),
            Some((id(80), "delegated"))
        );
        // Member-created issues subscribe through the ordinary creator rule.
        assert_eq!(
            delegated_subscriber(SubscriptionFacts {
                creator_type: "member".into(),
                origin_type: "quick_create".into(),
                origin_originator: Some(id(80)),
            }),
            None
        );
        // Automation origin excluded: it has its own subscriber template.
        assert_eq!(
            delegated_subscriber(SubscriptionFacts {
                creator_type: "agent".into(),
                origin_type: "automation".into(),
                origin_originator: Some(id(80)),
            }),
            None
        );
        // Degraded chain (no resolvable originator) excluded.
        assert_eq!(
            delegated_subscriber(SubscriptionFacts {
                creator_type: "agent".into(),
                origin_type: "quick_create".into(),
                origin_originator: None,
            }),
            None
        );
    }
}
