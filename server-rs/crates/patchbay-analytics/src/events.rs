//! Event catalog and typed builders.
//!
//! This file is the source-of-truth catalog; keep the frontend analytics
//! package in sync with it.

use std::collections::HashSet;
use std::sync::LazyLock;

use serde_json::Value;

use crate::client::{Event, Props};

// --- Event names ----------------------------------------------------------

pub const EVENT_SIGNUP: &str = "signup";
pub const EVENT_WORKSPACE_CREATED: &str = "workspace_created";
pub const EVENT_RUNTIME_REGISTERED: &str = "runtime_registered";
pub const EVENT_RUNTIME_READY: &str = "runtime_ready";
pub const EVENT_RUNTIME_FAILED: &str = "runtime_failed";
pub const EVENT_RUNTIME_OFFLINE: &str = "runtime_offline";
pub const EVENT_ISSUE_EXECUTED: &str = "issue_executed";
pub const EVENT_ISSUE_CREATED: &str = "issue_created";
pub const EVENT_CHAT_MESSAGE_SENT: &str = "chat_message_sent";
pub const EVENT_AUTOPILOT_RUN_STARTED: &str = "autopilot_run_started";
pub const EVENT_AUTOPILOT_RUN_COMPLETED: &str = "autopilot_run_completed";
pub const EVENT_AUTOPILOT_RUN_FAILED: &str = "autopilot_run_failed";
pub const EVENT_TEAM_INVITE_SENT: &str = "team_invite_sent";
pub const EVENT_TEAM_INVITE_ACCEPTED: &str = "team_invite_accepted";
pub const EVENT_ONBOARDING_STARTED: &str = "onboarding_started";
pub const EVENT_ONBOARDING_QUESTIONNAIRE_SUBMIT: &str = "onboarding_questionnaire_submitted";
pub const EVENT_ONBOARDING_SOURCE_SUBMIT: &str = "onboarding_source_submitted";
pub const EVENT_AGENT_CREATED: &str = "agent_created";
pub const EVENT_ONBOARDING_COMPLETED: &str = "onboarding_completed";
pub const EVENT_CLOUD_WAITLIST_JOINED: &str = "cloud_waitlist_joined";
pub const EVENT_FEEDBACK_SUBMITTED: &str = "feedback_submitted";
pub const EVENT_CONTACT_SALES_SUBMITTED: &str = "contact_sales_submitted";
pub const EVENT_SQUAD_CREATED: &str = "squad_created";
pub const EVENT_AUTOPILOT_CREATED: &str = "autopilot_created";

pub const EVENT_SCHEMA_VERSION: i64 = 2;

/// Every server-side event recorded to Prometheus but deliberately NOT shipped
/// to PostHog. As of PB-4127 PostHog no longer receives server-side product
/// analytics (the funnel is read from the operational DB and Grafana counters),
/// so ALL server-side events are metrics-only. PostHog now only receives
/// frontend error/crash telemetry.
static METRICS_ONLY_EVENTS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        // Product-behaviour events — DB + Grafana are the source of truth.
        EVENT_SIGNUP,
        EVENT_WORKSPACE_CREATED,
        EVENT_ISSUE_CREATED,
        EVENT_ISSUE_EXECUTED,
        EVENT_CHAT_MESSAGE_SENT,
        EVENT_TEAM_INVITE_SENT,
        EVENT_TEAM_INVITE_ACCEPTED,
        EVENT_ONBOARDING_STARTED,
        EVENT_ONBOARDING_QUESTIONNAIRE_SUBMIT,
        EVENT_ONBOARDING_SOURCE_SUBMIT,
        EVENT_AGENT_CREATED,
        EVENT_ONBOARDING_COMPLETED,
        EVENT_CLOUD_WAITLIST_JOINED,
        EVENT_FEEDBACK_SUBMITTED,
        EVENT_CONTACT_SALES_SUBMITTED,
        EVENT_SQUAD_CREATED,
        EVENT_AUTOPILOT_CREATED,
        // High-volume runtime / autopilot execution-lifecycle telemetry.
        EVENT_RUNTIME_REGISTERED,
        EVENT_RUNTIME_READY,
        EVENT_RUNTIME_FAILED,
        EVENT_RUNTIME_OFFLINE,
        EVENT_AUTOPILOT_RUN_STARTED,
        EVENT_AUTOPILOT_RUN_COMPLETED,
        EVENT_AUTOPILOT_RUN_FAILED,
    ])
});

/// Reports whether an event name is recorded to Prometheus but must not be
/// sent to PostHog.
pub fn is_metrics_only(name: &str) -> bool {
    METRICS_ONLY_EVENTS.contains(name)
}

// --- Shared vocabulary ------------------------------------------------------

pub const SOURCE_ONBOARDING: &str = "onboarding";
pub const SOURCE_MANUAL: &str = "manual";
pub const SOURCE_CHAT: &str = "chat";
pub const SOURCE_AUTOPILOT: &str = "autopilot";
pub const SOURCE_API: &str = "api";

pub const ONBOARDING_PATH_FULL: &str = "full";
pub const ONBOARDING_PATH_RUNTIME_SKIPPED: &str = "runtime_skipped";
pub const ONBOARDING_PATH_CLOUD_WAITLIST: &str = "cloud_waitlist";
pub const ONBOARDING_PATH_SKIP_EXISTING: &str = "skip_existing";
pub const ONBOARDING_PATH_INVITE_ACCEPT: &str = "invite_accept";
pub const ONBOARDING_PATH_UNKNOWN: &str = "unknown";

pub const PLATFORM_SERVER: &str = "server";
pub const PLATFORM_WEB: &str = "web";
pub const PLATFORM_DESKTOP: &str = "desktop";
pub const PLATFORM_CLI: &str = "cli";

/// The shared join and segmentation fields used by the canonical events.
/// Empty values are omitted by [`with_core_properties`], except `is_demo`
/// which is always stamped so dashboards can filter demo data without
/// sparse-property edge cases.
#[derive(Debug, Clone, Default)]
pub struct CoreProperties {
    pub user_id: String,
    pub workspace_id: String,
    pub agent_id: String,
    pub task_id: String,
    pub issue_id: String,
    pub chat_session_id: String,
    pub autopilot_run_id: String,
    pub source: String,
    pub runtime_mode: String,
    pub provider: String,
    pub is_demo: bool,
}

pub type TaskContext = CoreProperties;

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

fn b(v: bool) -> Value {
    Value::Bool(v)
}

fn n(v: i64) -> Value {
    Value::from(v)
}

/// Stamps the non-empty core fields into `props`; `is_demo` is unconditional.
fn with_core_properties(mut props: Props, core: &CoreProperties) -> Props {
    if !core.user_id.is_empty() {
        props.insert("user_id".to_string(), s(&core.user_id));
    }
    if !core.agent_id.is_empty() {
        props.insert("agent_id".to_string(), s(&core.agent_id));
    }
    if !core.task_id.is_empty() {
        props.insert("task_id".to_string(), s(&core.task_id));
    }
    if !core.issue_id.is_empty() {
        props.insert("issue_id".to_string(), s(&core.issue_id));
    }
    if !core.chat_session_id.is_empty() {
        props.insert("chat_session_id".to_string(), s(&core.chat_session_id));
    }
    if !core.autopilot_run_id.is_empty() {
        props.insert("autopilot_run_id".to_string(), s(&core.autopilot_run_id));
    }
    if !core.source.is_empty() {
        props.insert("source".to_string(), s(&core.source));
    }
    if !core.runtime_mode.is_empty() {
        props.insert("runtime_mode".to_string(), s(&core.runtime_mode));
    }
    if !core.provider.is_empty() {
        props.insert("provider".to_string(), s(&core.provider));
    }
    props.insert("is_demo".to_string(), b(core.is_demo));
    props
}

/// A synthetic person id keeps unrelated daemon registrations across
/// workspaces from merging under one anonymous identity.
fn workspace_distinct(workspace_id: &str) -> String {
    format!("workspace:{workspace_id}")
}

fn non_agent_user_id(distinct: &str) -> String {
    if distinct.is_empty() || distinct.contains(':') {
        return String::new();
    }
    distinct.to_string()
}

fn feedback_length_bucket(len: i64) -> &'static str {
    match len {
        n if n < 100 => "0-100",
        n if n < 500 => "100-500",
        n if n < 2000 => "500-2000",
        _ => "2000+",
    }
}

fn email_domain(email: &str) -> String {
    match email.rfind('@') {
        Some(at) if at != email.len() - 1 => email[at + 1..].to_lowercase(),
        _ => String::new(),
    }
}

// --- Builders ---------------------------------------------------------------

/// Builds the signup event. `signup_source` comes from the frontend's stored
/// UTM/referrer cookie if present; leave empty otherwise.
pub fn signup(user_id: &str, email: &str, signup_source: &str) -> Event {
    Event {
        name: EVENT_SIGNUP.to_string(),
        distinct_id: user_id.to_string(),
        properties: Some(Props::from_iter([
            ("email_domain".to_string(), s(&email_domain(email))),
            ("signup_source".to_string(), s(signup_source)),
        ])),
        set_once: Some(Props::from_iter([
            ("email".to_string(), s(email)),
            ("signup_source".to_string(), s(signup_source)),
        ])),
        ..Default::default()
    }
}

/// "Is this the user's first workspace?" is deliberately not stamped here —
/// it's derived downstream by checking whether the user has a prior event.
pub fn workspace_created(user_id: &str, workspace_id: &str) -> Event {
    Event {
        name: EVENT_WORKSPACE_CREATED.to_string(),
        distinct_id: user_id.to_string(),
        workspace_id: workspace_id.to_string(),
        properties: Some(with_core_properties(
            Props::new(),
            &CoreProperties {
                user_id: user_id.to_string(),
                workspace_id: workspace_id.to_string(),
                source: SOURCE_MANUAL.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

/// Fires the first time a (workspace, daemon, provider) triple is upserted;
/// heartbeats and repeat registrations never emit this. `owner_id` may be
/// empty for daemon-token auth — funnels needing per-user attribution fall
/// back to workspace_id as the grouping key.
#[allow(clippy::too_many_arguments)]
pub fn runtime_registered(
    owner_id: &str,
    workspace_id: &str,
    runtime_id: &str,
    daemon_id: &str,
    provider: &str,
    runtime_version: &str,
    cli_version: &str,
) -> Event {
    let distinct = if owner_id.is_empty() {
        workspace_distinct(workspace_id)
    } else {
        owner_id.to_string()
    };
    Event {
        name: EVENT_RUNTIME_REGISTERED.to_string(),
        distinct_id: distinct,
        workspace_id: workspace_id.to_string(),
        properties: Some(with_core_properties(
            Props::from_iter([
                ("runtime_id".to_string(), s(runtime_id)),
                ("daemon_id".to_string(), s(daemon_id)),
                ("provider".to_string(), s(provider)),
                ("runtime_mode".to_string(), s("local")),
                ("runtime_version".to_string(), s(runtime_version)),
                ("cli_version".to_string(), s(cli_version)),
            ]),
            &CoreProperties {
                user_id: owner_id.to_string(),
                workspace_id: workspace_id.to_string(),
                source: SOURCE_MANUAL.to_string(),
                runtime_mode: "local".to_string(),
                provider: provider.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

pub fn runtime_ready(
    owner_id: &str,
    workspace_id: &str,
    runtime_id: &str,
    daemon_id: &str,
    provider: &str,
    ready_duration_ms: i64,
) -> Event {
    let distinct = if owner_id.is_empty() {
        workspace_distinct(workspace_id)
    } else {
        owner_id.to_string()
    };
    let mut props = Props::from_iter([
        ("runtime_id".to_string(), s(runtime_id)),
        ("daemon_id".to_string(), s(daemon_id)),
    ]);
    if ready_duration_ms > 0 {
        props.insert("ready_duration_ms".to_string(), n(ready_duration_ms));
    }
    Event {
        name: EVENT_RUNTIME_READY.to_string(),
        distinct_id: distinct,
        workspace_id: workspace_id.to_string(),
        properties: Some(with_core_properties(
            props,
            &CoreProperties {
                user_id: owner_id.to_string(),
                workspace_id: workspace_id.to_string(),
                source: SOURCE_MANUAL.to_string(),
                runtime_mode: "local".to_string(),
                provider: provider.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

pub fn runtime_failed(
    owner_id: &str,
    workspace_id: &str,
    daemon_id: &str,
    provider: &str,
    failure_reason: &str,
    error_type: &str,
    recoverable: bool,
) -> Event {
    let distinct = if owner_id.is_empty() && !workspace_id.is_empty() {
        workspace_distinct(workspace_id)
    } else {
        owner_id.to_string()
    };
    Event {
        name: EVENT_RUNTIME_FAILED.to_string(),
        distinct_id: distinct,
        workspace_id: workspace_id.to_string(),
        properties: Some(with_core_properties(
            Props::from_iter([
                ("daemon_id".to_string(), s(daemon_id)),
                ("failure_reason".to_string(), s(failure_reason)),
                ("error_type".to_string(), s(error_type)),
                ("recoverable".to_string(), b(recoverable)),
            ]),
            &CoreProperties {
                user_id: owner_id.to_string(),
                workspace_id: workspace_id.to_string(),
                source: SOURCE_MANUAL.to_string(),
                runtime_mode: "local".to_string(),
                provider: provider.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

pub fn runtime_offline(
    owner_id: &str,
    workspace_id: &str,
    runtime_id: &str,
    daemon_id: &str,
    provider: &str,
) -> Event {
    let distinct = if owner_id.is_empty() {
        workspace_distinct(workspace_id)
    } else {
        owner_id.to_string()
    };
    Event {
        name: EVENT_RUNTIME_OFFLINE.to_string(),
        distinct_id: distinct,
        workspace_id: workspace_id.to_string(),
        properties: Some(with_core_properties(
            Props::from_iter([
                ("runtime_id".to_string(), s(runtime_id)),
                ("daemon_id".to_string(), s(daemon_id)),
            ]),
            &CoreProperties {
                user_id: owner_id.to_string(),
                workspace_id: workspace_id.to_string(),
                source: SOURCE_MANUAL.to_string(),
                runtime_mode: "local".to_string(),
                provider: provider.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

/// Fires at most once per issue lifetime — on the first task completion that
/// flips `issues.first_executed_at` from NULL atomically. Retries,
/// re-assignments, and comment-triggered follow-ups never re-emit.
///
/// Deliberately not stamped: the workspace's Nth-issue ordinal — computing it
/// at emit time is not atomic, and downstream derives the same number exactly
/// at query time.
#[allow(clippy::too_many_arguments)]
pub fn issue_executed(
    actor_id: &str,
    workspace_id: &str,
    issue_id: &str,
    task_id: &str,
    agent_id: &str,
    source: &str,
    runtime_mode: &str,
    provider: &str,
    task_duration_ms: i64,
) -> Event {
    Event {
        name: EVENT_ISSUE_EXECUTED.to_string(),
        distinct_id: actor_id.to_string(),
        workspace_id: workspace_id.to_string(),
        properties: Some(with_core_properties(
            Props::from_iter([
                ("issue_id".to_string(), s(issue_id)),
                ("task_id".to_string(), s(task_id)),
                ("agent_id".to_string(), s(agent_id)),
                ("task_duration_ms".to_string(), n(task_duration_ms)),
                ("duration_ms".to_string(), n(task_duration_ms)),
            ]),
            &CoreProperties {
                user_id: non_agent_user_id(actor_id),
                workspace_id: workspace_id.to_string(),
                agent_id: agent_id.to_string(),
                task_id: task_id.to_string(),
                issue_id: issue_id.to_string(),
                source: source.to_string(),
                runtime_mode: runtime_mode.to_string(),
                provider: provider.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn issue_created(
    actor_id: &str,
    workspace_id: &str,
    issue_id: &str,
    agent_id: &str,
    task_id: &str,
    autopilot_run_id: &str,
    source: &str,
    platform: &str,
) -> Event {
    let mut props = Props::new();
    if !platform.is_empty() {
        props.insert("platform".to_string(), s(platform));
    }
    Event {
        name: EVENT_ISSUE_CREATED.to_string(),
        distinct_id: actor_id.to_string(),
        workspace_id: workspace_id.to_string(),
        properties: Some(with_core_properties(
            props,
            &CoreProperties {
                user_id: non_agent_user_id(actor_id),
                workspace_id: workspace_id.to_string(),
                agent_id: agent_id.to_string(),
                task_id: task_id.to_string(),
                issue_id: issue_id.to_string(),
                autopilot_run_id: autopilot_run_id.to_string(),
                source: source.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn chat_message_sent(
    user_id: &str,
    workspace_id: &str,
    chat_session_id: &str,
    task_id: &str,
    agent_id: &str,
    runtime_mode: &str,
    provider: &str,
    platform: &str,
) -> Event {
    let mut props = Props::new();
    if !platform.is_empty() {
        props.insert("platform".to_string(), s(platform));
    }
    Event {
        name: EVENT_CHAT_MESSAGE_SENT.to_string(),
        distinct_id: user_id.to_string(),
        workspace_id: workspace_id.to_string(),
        properties: Some(with_core_properties(
            props,
            &CoreProperties {
                user_id: user_id.to_string(),
                workspace_id: workspace_id.to_string(),
                agent_id: agent_id.to_string(),
                task_id: task_id.to_string(),
                chat_session_id: chat_session_id.to_string(),
                source: SOURCE_CHAT.to_string(),
                runtime_mode: runtime_mode.to_string(),
                provider: provider.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

/// Describes the autopilot's configured target. `agent_id` is always the agent
/// that will actually execute the work (the squad leader for squad autopilots);
/// `assignee_type`/`squad_id` record the original configuration so reports can
/// tell a solo-agent autopilot apart from a squad one.
#[derive(Debug, Clone, Default)]
pub struct AutopilotAssignee {
    pub agent_id: String,
    /// "agent" or "squad".
    pub assignee_type: String,
    /// Empty when assignee_type != "squad".
    pub squad_id: String,
}

pub fn autopilot_run_started(
    actor_id: &str,
    workspace_id: &str,
    autopilot_id: &str,
    run_id: &str,
    cadence: &str,
    assignee: &AutopilotAssignee,
    trigger_source: &str,
) -> Event {
    autopilot_run_event(
        EVENT_AUTOPILOT_RUN_STARTED,
        actor_id,
        workspace_id,
        autopilot_id,
        run_id,
        cadence,
        assignee,
        trigger_source,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn autopilot_run_completed(
    actor_id: &str,
    workspace_id: &str,
    autopilot_id: &str,
    run_id: &str,
    cadence: &str,
    assignee: &AutopilotAssignee,
    trigger_source: &str,
    duration_ms: i64,
) -> Event {
    autopilot_run_event(
        EVENT_AUTOPILOT_RUN_COMPLETED,
        actor_id,
        workspace_id,
        autopilot_id,
        run_id,
        cadence,
        assignee,
        trigger_source,
        Some(Props::from_iter([(
            "duration_ms".to_string(),
            n(duration_ms),
        )])),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn autopilot_run_failed(
    actor_id: &str,
    workspace_id: &str,
    autopilot_id: &str,
    run_id: &str,
    cadence: &str,
    assignee: &AutopilotAssignee,
    trigger_source: &str,
    failure_reason: &str,
    error_type: &str,
    will_retry: bool,
    duration_ms: i64,
) -> Event {
    autopilot_run_event(
        EVENT_AUTOPILOT_RUN_FAILED,
        actor_id,
        workspace_id,
        autopilot_id,
        run_id,
        cadence,
        assignee,
        trigger_source,
        Some(Props::from_iter([
            ("duration_ms".to_string(), n(duration_ms)),
            ("failure_reason".to_string(), s(failure_reason)),
            ("error_type".to_string(), s(error_type)),
            ("will_retry".to_string(), b(will_retry)),
        ])),
    )
}

/// Fires when a workspace admin creates an invitation. `invite_method` is
/// "email" for now; future non-email flows pass their own value.
pub fn team_invite_sent(
    inviter_id: &str,
    workspace_id: &str,
    invited_email: &str,
    invite_method: &str,
) -> Event {
    Event {
        name: EVENT_TEAM_INVITE_SENT.to_string(),
        distinct_id: inviter_id.to_string(),
        workspace_id: workspace_id.to_string(),
        properties: Some(Props::from_iter([
            (
                "invited_email_domain".to_string(),
                s(&email_domain(invited_email)),
            ),
            ("invite_method".to_string(), s(invite_method)),
        ])),
        ..Default::default()
    }
}

/// Fires when the invitee accepts and joins; `days_since_invite` segments
/// fast-acceptance from long-tail acceptance.
pub fn team_invite_accepted(invitee_id: &str, workspace_id: &str, days_since_invite: i64) -> Event {
    Event {
        name: EVENT_TEAM_INVITE_ACCEPTED.to_string(),
        distinct_id: invitee_id.to_string(),
        workspace_id: workspace_id.to_string(),
        properties: Some(Props::from_iter([(
            "days_since_invite".to_string(),
            n(days_since_invite),
        )])),
        ..Default::default()
    }
}

/// Fires the first time a user's onboarding state transitions from untouched
/// to any non-empty patch. `platform` is the X-Client-Platform header value at
/// the time of the first interaction.
pub fn onboarding_started(user_id: &str, platform: &str) -> Event {
    let mut props = Props::new();
    if !platform.is_empty() {
        props.insert("platform".to_string(), s(platform));
    }
    Event {
        name: EVENT_ONBOARDING_STARTED.to_string(),
        distinct_id: user_id.to_string(),
        properties: Some(with_core_properties(
            props,
            &CoreProperties {
                user_id: user_id.to_string(),
                source: SOURCE_ONBOARDING.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

/// Fires the first time every questionnaire slot resolves to an answer or a
/// skip marker. `source`/`use_case` stay slices for v2 back-compat; answers
/// mirror into person properties via `$set` so cohorting works across every
/// event on the same user.
#[allow(clippy::too_many_arguments)]
pub fn onboarding_questionnaire_submitted(
    user_id: &str,
    source: Vec<String>,
    role: &str,
    use_case: Vec<String>,
    source_skipped: bool,
    role_skipped: bool,
    use_case_skipped: bool,
    source_has_other: bool,
    role_has_other: bool,
    use_case_has_other: bool,
) -> Event {
    let source_arr = Value::Array(source.iter().map(|v| s(v)).collect());
    let use_case_arr = Value::Array(use_case.iter().map(|v| s(v)).collect());
    Event {
        name: EVENT_ONBOARDING_QUESTIONNAIRE_SUBMIT.to_string(),
        distinct_id: user_id.to_string(),
        properties: Some(with_core_properties(
            Props::from_iter([
                ("source".to_string(), source_arr.clone()),
                ("role".to_string(), s(role)),
                ("use_case".to_string(), use_case_arr.clone()),
                ("source_skipped".to_string(), b(source_skipped)),
                ("role_skipped".to_string(), b(role_skipped)),
                ("use_case_skipped".to_string(), b(use_case_skipped)),
                ("source_has_other".to_string(), b(source_has_other)),
                ("role_has_other".to_string(), b(role_has_other)),
                ("use_case_has_other".to_string(), b(use_case_has_other)),
            ]),
            &CoreProperties {
                user_id: user_id.to_string(),
                source: SOURCE_ONBOARDING.to_string(),
                ..Default::default()
            },
        )),
        set: Some(Props::from_iter([
            ("source".to_string(), source_arr),
            ("role".to_string(), s(role)),
            ("use_case".to_string(), use_case_arr),
        ])),
        ..Default::default()
    }
}

/// Fires when the user's acquisition source transitions from unresolved to
/// resolved — answered or explicitly declined. Asked by the workspace backfill
/// prompt after agents have completed work (PB-5159). The property key is
/// `acquisition_source`, not `source`: core properties stamp the event-source
/// dimension into props["source"] and the acquisition answer must not fight it.
pub fn onboarding_source_submitted(
    user_id: &str,
    source: Vec<String>,
    skipped: bool,
    has_other: bool,
) -> Event {
    let source_arr = Value::Array(source.iter().map(|v| s(v)).collect());
    let mut ev = Event {
        name: EVENT_ONBOARDING_SOURCE_SUBMIT.to_string(),
        distinct_id: user_id.to_string(),
        properties: Some(with_core_properties(
            Props::from_iter([
                ("acquisition_source".to_string(), source_arr.clone()),
                ("source_skipped".to_string(), b(skipped)),
                ("source_has_other".to_string(), b(has_other)),
            ]),
            &CoreProperties {
                user_id: user_id.to_string(),
                source: SOURCE_ONBOARDING.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    if !source.is_empty() {
        ev.set = Some(Props::from_iter([("source".to_string(), source_arr)]));
    }
    ev
}

/// Fires whenever a new agent is added to a workspace — not just inside
/// onboarding. `template` is the creation-source attribution (e.g.
/// "agent_builder"); empty identifies a manually authored agent.
pub fn agent_created(
    actor_id: &str,
    workspace_id: &str,
    agent_id: &str,
    provider: &str,
    runtime_mode: &str,
    template: &str,
    is_first_agent_in_workspace: bool,
) -> Event {
    Event {
        name: EVENT_AGENT_CREATED.to_string(),
        distinct_id: actor_id.to_string(),
        workspace_id: workspace_id.to_string(),
        properties: Some(with_core_properties(
            Props::from_iter([
                ("agent_id".to_string(), s(agent_id)),
                ("provider".to_string(), s(provider)),
                ("runtime_mode".to_string(), s(runtime_mode)),
                ("template".to_string(), s(template)),
                (
                    "is_first_agent_in_workspace".to_string(),
                    b(is_first_agent_in_workspace),
                ),
            ]),
            &CoreProperties {
                user_id: actor_id.to_string(),
                workspace_id: workspace_id.to_string(),
                agent_id: agent_id.to_string(),
                source: SOURCE_MANUAL.to_string(),
                runtime_mode: runtime_mode.to_string(),
                provider: provider.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

/// Fires from CompleteOnboarding. `completion_path` derives server-side from
/// the state the user arrived in; `joined_cloud_waitlist` is orthogonal to it.
/// `onboarded_at` (RFC3339) sets $set_once on the person so date cohorts are
/// queryable without re-emitting per-event.
pub fn onboarding_completed(
    user_id: &str,
    workspace_id: &str,
    completion_path: &str,
    onboarded_at: &str,
    joined_cloud_waitlist: bool,
) -> Event {
    Event {
        name: EVENT_ONBOARDING_COMPLETED.to_string(),
        distinct_id: user_id.to_string(),
        workspace_id: workspace_id.to_string(),
        properties: Some(with_core_properties(
            Props::from_iter([
                ("completion_path".to_string(), s(completion_path)),
                (
                    "joined_cloud_waitlist".to_string(),
                    b(joined_cloud_waitlist),
                ),
            ]),
            &CoreProperties {
                user_id: user_id.to_string(),
                workspace_id: workspace_id.to_string(),
                source: SOURCE_ONBOARDING.to_string(),
                ..Default::default()
            },
        )),
        set_once: Some(Props::from_iter([(
            "onboarded_at".to_string(),
            s(onboarded_at),
        )])),
        ..Default::default()
    }
}

/// Fires when a user submits the Step 3 cloud waitlist form. `has_reason` is a
/// presence bool — the free-text reason stays in the DB.
pub fn cloud_waitlist_joined(user_id: &str, has_reason: bool) -> Event {
    Event {
        name: EVENT_CLOUD_WAITLIST_JOINED.to_string(),
        distinct_id: user_id.to_string(),
        properties: Some(with_core_properties(
            Props::from_iter([("has_reason".to_string(), b(has_reason))]),
            &CoreProperties {
                user_id: user_id.to_string(),
                source: SOURCE_ONBOARDING.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

/// Fires after a feedback row is successfully inserted. Only a coarse length
/// bucket, an image-presence flag, the kind picker selection, and client
/// platform / version ship — never the message content.
pub fn feedback_submitted(
    user_id: &str,
    workspace_id: &str,
    kind: &str,
    message_len: i64,
    has_images: bool,
    platform: &str,
    app_version: &str,
) -> Event {
    let mut props = Props::from_iter([
        (
            "message_length_bucket".to_string(),
            s(feedback_length_bucket(message_len)),
        ),
        ("has_images".to_string(), b(has_images)),
    ]);
    if !kind.is_empty() {
        props.insert("kind".to_string(), s(kind));
    }
    if !platform.is_empty() {
        props.insert("platform".to_string(), s(platform));
    }
    if !app_version.is_empty() {
        props.insert("app_version".to_string(), s(app_version));
    }
    Event {
        name: EVENT_FEEDBACK_SUBMITTED.to_string(),
        distinct_id: user_id.to_string(),
        workspace_id: workspace_id.to_string(),
        properties: Some(with_core_properties(
            props,
            &CoreProperties {
                user_id: user_id.to_string(),
                workspace_id: workspace_id.to_string(),
                source: "ops_feedback".to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

/// Fires after a contact-sales inquiry is recorded. The form is public and
/// unauthenticated, so DistinctID is the inquiry id (anonymous). Core Source
/// stays "marketing_contact_sales" so dashboards keep the funnel join; the
/// Prometheus side reads form_source via the allow-list normalizer.
pub fn contact_sales_submitted(
    inquiry_id: &str,
    company_size: &str,
    country_region: &str,
    use_case: &str,
    form_source: &str,
    has_goals: bool,
) -> Event {
    let mut props = Props::from_iter([
        ("inquiry_id".to_string(), s(inquiry_id)),
        ("company_size".to_string(), s(company_size)),
        ("country_region".to_string(), s(country_region)),
        ("use_case".to_string(), s(use_case)),
        ("has_goals".to_string(), b(has_goals)),
    ]);
    if !form_source.is_empty() {
        props.insert("form_source".to_string(), s(form_source));
    }
    Event {
        name: EVENT_CONTACT_SALES_SUBMITTED.to_string(),
        distinct_id: inquiry_id.to_string(),
        properties: Some(with_core_properties(
            props,
            &CoreProperties {
                source: "marketing_contact_sales".to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

/// Fires when a workspace member or admin creates a new squad.
/// `member_count` is the seed size at creation time.
pub fn squad_created(
    actor_id: &str,
    workspace_id: &str,
    squad_id: &str,
    member_count: i64,
) -> Event {
    Event {
        name: EVENT_SQUAD_CREATED.to_string(),
        distinct_id: actor_id.to_string(),
        workspace_id: workspace_id.to_string(),
        properties: Some(with_core_properties(
            Props::from_iter([
                ("squad_id".to_string(), s(squad_id)),
                ("member_count".to_string(), n(member_count)),
            ]),
            &CoreProperties {
                user_id: non_agent_user_id(actor_id),
                workspace_id: workspace_id.to_string(),
                source: SOURCE_MANUAL.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

/// Fires when a workspace member creates a new autopilot. `trigger_kind` is
/// the initial trigger type — when both schedule and webhook triggers are
/// seeded, schedule wins upstream.
pub fn autopilot_created(
    actor_id: &str,
    workspace_id: &str,
    autopilot_id: &str,
    cadence: &str,
    trigger_kind: &str,
) -> Event {
    Event {
        name: EVENT_AUTOPILOT_CREATED.to_string(),
        distinct_id: actor_id.to_string(),
        workspace_id: workspace_id.to_string(),
        properties: Some(with_core_properties(
            Props::from_iter([
                ("autopilot_id".to_string(), s(autopilot_id)),
                ("cadence".to_string(), s(cadence)),
                ("trigger_kind".to_string(), s(trigger_kind)),
            ]),
            &CoreProperties {
                user_id: non_agent_user_id(actor_id),
                workspace_id: workspace_id.to_string(),
                source: SOURCE_MANUAL.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

#[allow(clippy::too_many_arguments)]
fn autopilot_run_event(
    name: &str,
    actor_id: &str,
    workspace_id: &str,
    autopilot_id: &str,
    run_id: &str,
    cadence: &str,
    assignee: &AutopilotAssignee,
    trigger_source: &str,
    extra: Option<Props>,
) -> Event {
    let mut props = extra.unwrap_or_default();
    props.insert("trigger_source".to_string(), s(trigger_source));
    props.insert("trigger_kind".to_string(), s(trigger_source));
    if !cadence.is_empty() {
        props.insert("cadence".to_string(), s(cadence));
    }
    let mut props = with_core_properties(
        props,
        &CoreProperties {
            user_id: non_agent_user_id(actor_id),
            workspace_id: workspace_id.to_string(),
            agent_id: assignee.agent_id.clone(),
            autopilot_run_id: run_id.to_string(),
            source: SOURCE_AUTOPILOT.to_string(),
            ..Default::default()
        },
    );
    props.insert("autopilot_id".to_string(), s(autopilot_id));
    if !assignee.assignee_type.is_empty() {
        props.insert("assignee_type".to_string(), s(&assignee.assignee_type));
    }
    if !assignee.squad_id.is_empty() {
        props.insert("squad_id".to_string(), s(&assignee.squad_id));
    }
    Event {
        name: name.to_string(),
        distinct_id: actor_id.to_string(),
        workspace_id: workspace_id.to_string(),
        properties: Some(props),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_catalog_event_is_metrics_only() {
        for name in [
            EVENT_SIGNUP,
            EVENT_WORKSPACE_CREATED,
            EVENT_RUNTIME_REGISTERED,
            EVENT_RUNTIME_READY,
            EVENT_RUNTIME_FAILED,
            EVENT_RUNTIME_OFFLINE,
            EVENT_ISSUE_EXECUTED,
            EVENT_ISSUE_CREATED,
            EVENT_CHAT_MESSAGE_SENT,
            EVENT_AUTOPILOT_RUN_STARTED,
            EVENT_AUTOPILOT_RUN_COMPLETED,
            EVENT_AUTOPILOT_RUN_FAILED,
            EVENT_TEAM_INVITE_SENT,
            EVENT_TEAM_INVITE_ACCEPTED,
            EVENT_ONBOARDING_STARTED,
            EVENT_ONBOARDING_QUESTIONNAIRE_SUBMIT,
            EVENT_ONBOARDING_SOURCE_SUBMIT,
            EVENT_AGENT_CREATED,
            EVENT_ONBOARDING_COMPLETED,
            EVENT_CLOUD_WAITLIST_JOINED,
            EVENT_FEEDBACK_SUBMITTED,
            EVENT_CONTACT_SALES_SUBMITTED,
            EVENT_SQUAD_CREATED,
            EVENT_AUTOPILOT_CREATED,
        ] {
            assert!(is_metrics_only(name), "{name}");
        }
        assert!(!is_metrics_only("client_crash"));
    }

    #[test]
    fn email_domain_edges() {
        assert_eq!(email_domain("User@Example.COM"), "example.com");
        assert_eq!(email_domain("no-at-sign"), "");
        assert_eq!(email_domain("trailing@"), "");
        assert_eq!(email_domain("a@b@c.co"), "c.co");
    }

    #[test]
    fn feedback_buckets_boundaries() {
        assert_eq!(feedback_length_bucket(0), "0-100");
        assert_eq!(feedback_length_bucket(99), "0-100");
        assert_eq!(feedback_length_bucket(100), "100-500");
        assert_eq!(feedback_length_bucket(499), "100-500");
        assert_eq!(feedback_length_bucket(500), "500-2000");
        assert_eq!(feedback_length_bucket(1999), "500-2000");
        assert_eq!(feedback_length_bucket(2000), "2000+");
    }

    #[test]
    fn non_agent_user_id_filters_synthetic_scopes() {
        assert_eq!(non_agent_user_id(""), "");
        assert_eq!(non_agent_user_id("workspace:ws-1"), "");
        assert_eq!(non_agent_user_id("user-42"), "user-42");
    }

    #[test]
    fn core_props_stamp_is_demo_always_and_omit_empties() {
        let out = with_core_properties(Props::new(), &CoreProperties::default());
        assert_eq!(out.len(), 1);
        // Zero-value CoreProperties has IsDemo=false, but the key is ALWAYS
        // stamped so dashboards never hit sparse-property edge cases.
        assert_eq!(out["is_demo"], json!(false));
    }

    #[test]
    fn core_props_is_demo_reflects_flag_and_fields_conditional() {
        let out = with_core_properties(
            Props::new(),
            &CoreProperties {
                user_id: "u".into(),
                source: SOURCE_MANUAL.into(),
                is_demo: true,
                ..Default::default()
            },
        );
        assert_eq!(out["user_id"], json!("u"));
        assert_eq!(out["source"], json!("manual"));
        assert_eq!(out["is_demo"], json!(true));
        assert!(!out.contains_key("task_id"));
    }

    #[test]
    fn signup_carries_set_once_and_email_domain() {
        let e = signup("u1", "Dev@Example.TEST", "x");
        assert_eq!(e.name, "signup");
        assert_eq!(
            e.properties.as_ref().unwrap()["email_domain"],
            json!("example.test")
        );
        assert_eq!(e.properties.as_ref().unwrap()["signup_source"], json!("x"));
        assert_eq!(
            e.set_once.as_ref().unwrap()["email"],
            json!("Dev@Example.TEST")
        );
    }

    #[test]
    fn runtime_registered_falls_back_to_workspace_scope() {
        let e = runtime_registered("", "ws-1", "rt", "dm", "claude", "1.0", "2.0");
        assert_eq!(e.distinct_id, "workspace:ws-1");
        let p = e.properties.unwrap();
        assert_eq!(p["runtime_mode"], json!("local"));
        assert!(!p.contains_key("user_id"), "empty owner is omitted like Go");
        assert_eq!(p["is_demo"], json!(false));

        let e = runtime_registered("u9", "ws-1", "rt", "dm", "claude", "1.0", "2.0");
        assert_eq!(e.distinct_id, "u9");
    }

    #[test]
    fn runtime_failed_keeps_empty_distinct_when_no_workspace() {
        let e = runtime_failed("", "", "dm", "claude", "timeout", "TimeoutError", true);
        assert_eq!(e.distinct_id, "");
        let p = e.properties.unwrap();
        assert_eq!(p["recoverable"], json!(true));
    }

    #[test]
    fn runtime_ready_duration_only_when_positive() {
        let e = runtime_ready("u", "ws", "rt", "dm", "codex", 0);
        assert!(!e
            .properties
            .as_ref()
            .unwrap()
            .contains_key("ready_duration_ms"));
        let e = runtime_ready("u", "ws", "rt", "dm", "codex", 4500);
        assert_eq!(
            e.properties.as_ref().unwrap()["ready_duration_ms"],
            json!(4500)
        );
    }

    #[test]
    fn issue_executed_stamps_both_duration_keys() {
        let e = issue_executed("u", "ws", "i", "t", "a", "issue", "local", "claude", 1234);
        let p = e.properties.unwrap();
        assert_eq!(p["task_duration_ms"], json!(1234));
        assert_eq!(p["duration_ms"], json!(1234));
        assert_eq!(p["user_id"], json!("u"));
    }

    #[test]
    fn autopilot_run_events_shape() {
        let assignee = AutopilotAssignee {
            agent_id: "ag".into(),
            assignee_type: "squad".into(),
            squad_id: "sq".into(),
        };
        let started = autopilot_run_started("u", "ws", "ap", "run", "daily", &assignee, "schedule");
        let p = started.properties.as_ref().unwrap();
        assert_eq!(p["trigger_source"], json!("schedule"));
        assert_eq!(p["trigger_kind"], json!("schedule"));
        assert_eq!(p["cadence"], json!("daily"));
        assert_eq!(p["autopilot_id"], json!("ap"));
        assert_eq!(p["assignee_type"], json!("squad"));
        assert_eq!(p["squad_id"], json!("sq"));
        assert_eq!(p["source"], json!("autopilot"));
        assert!(!p.contains_key("duration_ms"));

        let failed = autopilot_run_failed(
            "u",
            "ws",
            "ap",
            "run",
            "",
            &assignee,
            "webhook",
            "timeout",
            "TimeoutError",
            true,
            88,
        );
        let p = failed.properties.as_ref().unwrap();
        assert!(!p.contains_key("cadence"), "empty cadence omitted");
        assert_eq!(p["failure_reason"], json!("timeout"));
        assert_eq!(p["will_retry"], json!(true));
        assert_eq!(p["duration_ms"], json!(88));
    }

    #[test]
    fn onboarding_source_uses_acquisition_key_and_conditional_set() {
        let empty = onboarding_source_submitted("u", vec![], true, false);
        assert_eq!(
            empty.properties.as_ref().unwrap()["acquisition_source"],
            json!([])
        );
        assert!(empty.set.is_none());

        let answered = onboarding_source_submitted("u", vec!["github".into()], false, false);
        assert_eq!(
            answered.properties.as_ref().unwrap()["acquisition_source"],
            json!(["github"])
        );
        assert_eq!(answered.set.as_ref().unwrap()["source"], json!(["github"]));
    }

    #[test]
    fn questionnaire_mirrors_answers_into_set() {
        let e = onboarding_questionnaire_submitted(
            "u",
            vec!["google".into()],
            "eng",
            vec!["coding".into()],
            false,
            false,
            false,
            false,
            false,
            false,
        );
        assert_eq!(e.set.as_ref().unwrap()["role"], json!("eng"));
        assert_eq!(
            e.properties.as_ref().unwrap()["use_case"],
            json!(["coding"])
        );
    }

    #[test]
    fn contact_sales_is_anonymous_with_marketing_source() {
        let e = contact_sales_submitted("inq-1", "11-50", "US", "agents", "page", true);
        assert_eq!(e.distinct_id, "inq-1");
        let p = e.properties.unwrap();
        assert_eq!(p["form_source"], json!("page"));
        assert_eq!(p["source"], json!("marketing_contact_sales"));
        assert_eq!(p["is_demo"], json!(false));

        let e = contact_sales_submitted("inq-2", "1-10", "DE", "x", "", false);
        assert!(!e.properties.as_ref().unwrap().contains_key("form_source"));
    }

    #[test]
    fn feedback_optional_fields_conditionally_stamped() {
        let e = feedback_submitted("u", "ws", "", 250, true, "", "");
        let p = e.properties.unwrap();
        assert_eq!(p["message_length_bucket"], json!("100-500"));
        assert!(!p.contains_key("kind"));
        assert!(!p.contains_key("platform"));
        assert!(!p.contains_key("app_version"));
        assert_eq!(p["source"], json!("ops_feedback"));
    }
}
