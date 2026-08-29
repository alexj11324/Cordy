//! Subscriber and activity side effects — ports of

use std::collections::HashSet;
use std::sync::OnceLock;

use patchbay_db::models::ActivityLog;
use patchbay_db::queries::{activity, agent, issue, subscriber};
use patchbay_events::{Bus, Event};
use patchbay_service::attribution::{delegated_subscriber, SubscriptionFacts};
use regex::Regex;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
struct IssueFields {
    id: Uuid,
    workspace_id: Uuid,
    creator_type: String,
    creator_id: Uuid,
    assignee_type: Option<String>,
    assignee_id: Option<Uuid>,
    reviewer_type: Option<String>,
    reviewer_id: Option<Uuid>,
    description: Option<String>,
    title: String,
    status: String,
    priority: String,
    start_date: Option<String>,
    due_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mention {
    user_type: String,
    user_id: String,
}

pub(crate) async fn handle_event(pool: &PgPool, bus: &Bus, event: &Event) {
    match event.event_type.as_str() {
        patchbay_protocol::EVENT_ISSUE_CREATED => handle_issue_created(pool, bus, event).await,
        patchbay_protocol::EVENT_ISSUE_UPDATED => handle_issue_updated(pool, bus, event).await,
        patchbay_protocol::EVENT_COMMENT_CREATED => handle_comment_created(pool, bus, event).await,
        patchbay_protocol::EVENT_TASK_COMPLETED => {
            handle_task_activity(pool, bus, event, "task_completed").await;
        }
        patchbay_protocol::EVENT_TASK_FAILED => {
            handle_task_activity(pool, bus, event, "task_failed").await;
        }
        _ => {}
    }
}

async fn handle_issue_created(pool: &PgPool, bus: &Bus, event: &Event) {
    let Some(fields) = scoped_issue(event) else {
        return;
    };

    // Go registration order: creator, direct assignee, mentions, delegated
    // human. Keeping them in one task also makes notification listeners able
    // to follow this listener later without reordering these writes.
    add_subscriber(
        pool,
        bus,
        fields.workspace_id,
        fields.id,
        &fields.creator_type,
        fields.creator_id,
        "creator",
    )
    .await;
    if let (Some(user_type), Some(user_id)) = (fields.assignee_type.as_deref(), fields.assignee_id)
    {
        if is_assignment_recipient(user_type)
            && !(user_type == fields.creator_type && user_id == fields.creator_id)
        {
            add_subscriber(
                pool,
                bus,
                fields.workspace_id,
                fields.id,
                user_type,
                user_id,
                "assignee",
            )
            .await;
        }
    }
    if let Some(description) = fields.description.as_deref() {
        for mention in parse_mentions(description) {
            let Ok(user_id) = mention.user_id.parse() else {
                return;
            };
            add_subscriber(
                pool,
                bus,
                fields.workspace_id,
                fields.id,
                &mention.user_type,
                user_id,
                "mentioned",
            )
            .await;
        }
    }
    subscribe_delegated_human(pool, bus, fields.workspace_id, fields.id).await;

    // Go records creation activity only for handler.IssueResponse, not the map
    // used by Autopilot. The handler payload uniquely carries `labels`.
    if event
        .payload
        .get("issue")
        .and_then(Value::as_object)
        .is_some_and(|issue| issue.contains_key("labels"))
    {
        create_activity(pool, bus, event, &fields, "created", json!({})).await;
    }
}

async fn handle_issue_updated(pool: &PgPool, bus: &Bus, event: &Event) {
    let Some(fields) = scoped_issue(event) else {
        return;
    };

    if flag(&event.payload, "assignee_changed") {
        if let (Some(user_type), Some(user_id)) =
            (fields.assignee_type.as_deref(), fields.assignee_id)
        {
            if is_assignment_recipient(user_type) {
                add_subscriber(
                    pool,
                    bus,
                    fields.workspace_id,
                    fields.id,
                    user_type,
                    user_id,
                    "assignee",
                )
                .await;
            }
        }
    }
    if flag(&event.payload, "description_changed") {
        let previous = event
            .payload
            .get("prev_description")
            .and_then(Value::as_str)
            .map(parse_mentions)
            .unwrap_or_default()
            .into_iter()
            .map(|mention| (mention.user_type, mention.user_id))
            .collect::<HashSet<_>>();
        if let Some(description) = fields.description.as_deref() {
            for mention in parse_mentions(description) {
                if previous.contains(&(mention.user_type.clone(), mention.user_id.clone())) {
                    continue;
                }
                let Ok(user_id) = mention.user_id.parse() else {
                    return;
                };
                add_subscriber(
                    pool,
                    bus,
                    fields.workspace_id,
                    fields.id,
                    &mention.user_type,
                    user_id,
                    "mentioned",
                )
                .await;
            }
        }
    }

    // Handler events always carry this flag, including false. Background map
    // events omit it and Go deliberately excludes them from activity/inbox.
    if event.payload.get("priority_changed").is_none() {
        return;
    }
    let review_handoff = flag(&event.payload, "review_handoff");
    if review_handoff {
        if let (Some(user_type), Some(user_id)) =
            (fields.reviewer_type.as_deref(), fields.reviewer_id)
        {
            if is_assignment_recipient(user_type) {
                add_subscriber(
                    pool,
                    bus,
                    fields.workspace_id,
                    fields.id,
                    user_type,
                    user_id,
                    "assignee",
                )
                .await;
            }
        }
        let mut details = Map::new();
        insert_optional(
            &mut details,
            "from_type",
            event.payload.get("prev_assignee_type"),
        );
        insert_optional(
            &mut details,
            "from_id",
            event.payload.get("prev_assignee_id"),
        );
        insert_optional_str(&mut details, "to_type", fields.reviewer_type.as_deref());
        if let Some(to_id) = fields.reviewer_id {
            details.insert("to_id".into(), Value::String(to_id.to_string()));
        }
        insert_optional(
            &mut details,
            "from_status",
            event.payload.get("prev_status"),
        );
        details.insert("to_status".into(), Value::String(fields.status.clone()));
        create_activity(
            pool,
            bus,
            event,
            &fields,
            "review_handoff",
            Value::Object(details),
        )
        .await;
    }
    if flag(&event.payload, "status_changed") && !review_handoff {
        create_activity(
            pool,
            bus,
            event,
            &fields,
            "status_changed",
            json!({"from": string(&event.payload, "prev_status"), "to": fields.status.as_str()}),
        )
        .await;
    }
    if flag(&event.payload, "priority_changed") {
        create_activity(
            pool,
            bus,
            event,
            &fields,
            "priority_changed",
            json!({"from": string(&event.payload, "prev_priority"), "to": fields.priority.as_str()}),
        )
        .await;
    }
    if flag(&event.payload, "assignee_changed") && !review_handoff {
        let mut details = Map::new();
        insert_optional(
            &mut details,
            "from_type",
            event.payload.get("prev_assignee_type"),
        );
        insert_optional(
            &mut details,
            "from_id",
            event.payload.get("prev_assignee_id"),
        );
        insert_optional_str(&mut details, "to_type", fields.assignee_type.as_deref());
        insert_optional_str(
            &mut details,
            "to_id",
            fields.assignee_id.as_ref().map(Uuid::to_string).as_deref(),
        );
        create_activity(
            pool,
            bus,
            event,
            &fields,
            "assignee_changed",
            Value::Object(details),
        )
        .await;
    }
    if flag(&event.payload, "start_date_changed") {
        create_activity(
            pool,
            bus,
            event,
            &fields,
            "start_date_changed",
            json!({"from": string(&event.payload, "prev_start_date"), "to": fields.start_date.as_deref().unwrap_or_default()}),
        )
        .await;
    }
    if flag(&event.payload, "due_date_changed") {
        create_activity(
            pool,
            bus,
            event,
            &fields,
            "due_date_changed",
            json!({"from": string(&event.payload, "prev_due_date"), "to": fields.due_date.as_deref().unwrap_or_default()}),
        )
        .await;
    }
    if flag(&event.payload, "title_changed") {
        create_activity(
            pool,
            bus,
            event,
            &fields,
            "title_changed",
            json!({"from": string(&event.payload, "prev_title"), "to": fields.title.as_str()}),
        )
        .await;
    }
    if flag(&event.payload, "description_changed") {
        create_activity(pool, bus, event, &fields, "description_updated", json!({})).await;
    }
}

async fn handle_comment_created(pool: &PgPool, bus: &Bus, event: &Event) {
    let Some(comment) = event.payload.get("comment") else {
        return;
    };
    let Some(issue_id) = uuid(comment, "issue_id") else {
        return;
    };
    let Some(author_id) = uuid(comment, "author_id") else {
        return;
    };
    let Some(author_type) = comment.get("author_type").and_then(Value::as_str) else {
        return;
    };
    if author_type == "system" {
        return;
    }
    let Some(workspace_id) = event_workspace(event) else {
        return;
    };
    let Ok(Some(owner)) = issue::get_issue(pool, issue_id).await else {
        return;
    };
    if owner.workspace_id != workspace_id {
        tracing::warn!(%issue_id, %workspace_id, "subscriber listener: comment workspace mismatch");
        return;
    }
    add_subscriber(
        pool,
        bus,
        workspace_id,
        issue_id,
        author_type,
        author_id,
        "commenter",
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn add_subscriber(
    pool: &PgPool,
    bus: &Bus,
    workspace_id: Uuid,
    issue_id: Uuid,
    user_type: &str,
    user_id: Uuid,
    reason: &str,
) {
    match subscriber::add_issue_subscriber(pool, issue_id, user_type, user_id, reason).await {
        Ok(0) => {}
        Ok(_) => publish_subscriber_added(bus, workspace_id, issue_id, user_type, user_id, reason),
        Err(error) => tracing::error!(
            %error, %issue_id, user_type, %user_id, reason,
            "subscriber listener: add failed"
        ),
    }
}

async fn subscribe_delegated_human(pool: &PgPool, bus: &Bus, workspace_id: Uuid, issue_id: Uuid) {
    let issue = match issue::get_issue(pool, issue_id).await {
        Ok(Some(issue)) if issue.workspace_id == workspace_id => issue,
        Ok(_) => return,
        Err(error) => {
            tracing::error!(%error, %issue_id, "delegated subscribe: load issue failed");
            return;
        }
    };
    let (Some(origin_type), Some(origin_id)) = (issue.origin_type.as_deref(), issue.origin_id)
    else {
        return;
    };
    if issue.creator_type != "agent" {
        return;
    }
    let origin = match agent::get_agent_task_in_workspace(pool, origin_id, workspace_id).await {
        Ok(Some(origin)) => origin,
        Ok(None) => return,
        Err(error) => {
            tracing::debug!(%error, %issue_id, origin_type, "delegated subscribe: origin task not resolvable");
            return;
        }
    };
    let Some((human, reason)) = delegated_subscriber(SubscriptionFacts {
        creator_type: issue.creator_type.clone(),
        origin_type: origin_type.to_string(),
        origin_originator: origin.originator_user_id,
    }) else {
        return;
    };

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, %issue_id, "delegated subscribe: begin failed");
            return;
        }
    };
    if let Err(error) = subscriber::lock_subscriber_writes(&mut *tx, workspace_id, human).await {
        tracing::error!(%error, %issue_id, "delegated subscribe: lock failed");
        return;
    }
    let affected =
        match subscriber::add_delegated_subscriber(&mut *tx, issue_id, human, reason, workspace_id)
            .await
        {
            Ok(affected) => affected,
            Err(error) => {
                tracing::error!(%error, %issue_id, "delegated subscribe: insert failed");
                return;
            }
        };
    if let Err(error) = tx.commit().await {
        tracing::error!(%error, %issue_id, "delegated subscribe: commit failed");
        return;
    }
    if affected > 0 {
        publish_subscriber_added(bus, workspace_id, issue_id, "member", human, reason);
    }
}

fn publish_subscriber_added(
    bus: &Bus,
    workspace_id: Uuid,
    issue_id: Uuid,
    user_type: &str,
    user_id: Uuid,
    reason: &str,
) {
    bus.publish(&Event {
        event_type: patchbay_protocol::EVENT_SUBSCRIBER_ADDED.to_string(),
        workspace_id: workspace_id.to_string(),
        payload: json!({
            "issue_id": issue_id,
            "user_type": user_type,
            "user_id": user_id,
            "reason": reason,
        }),
        ..Default::default()
    });
}

async fn handle_task_activity(pool: &PgPool, bus: &Bus, event: &Event, action: &str) {
    let Some(issue_id) = uuid(&event.payload, "issue_id") else {
        return;
    };
    let Some(workspace_id) = event_workspace(event) else {
        return;
    };
    let issue = match issue::get_issue(pool, issue_id).await {
        Ok(Some(issue)) if issue.workspace_id == workspace_id => issue,
        Ok(_) => return,
        Err(error) => {
            tracing::error!(%error, %issue_id, action, "activity listener: task issue load failed");
            return;
        }
    };
    let Some(actor_id) = uuid(&event.payload, "agent_id") else {
        return;
    };
    insert_activity(
        pool,
        bus,
        event,
        issue.workspace_id,
        issue.id,
        Some("agent"),
        Some(actor_id),
        action,
        json!({}),
    )
    .await;
}

async fn create_activity(
    pool: &PgPool,
    bus: &Bus,
    event: &Event,
    fields: &IssueFields,
    action: &str,
    details: Value,
) {
    let actor_type = (!event.actor_type.is_empty()).then_some(event.actor_type.as_str());
    let actor_id = if event.actor_id.is_empty() {
        None
    } else {
        let Ok(actor_id) = event.actor_id.parse() else {
            return;
        };
        Some(actor_id)
    };
    insert_activity(
        pool,
        bus,
        event,
        fields.workspace_id,
        fields.id,
        actor_type,
        actor_id,
        action,
        details,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn insert_activity(
    pool: &PgPool,
    bus: &Bus,
    event: &Event,
    workspace_id: Uuid,
    issue_id: Uuid,
    actor_type: Option<&str>,
    actor_id: Option<Uuid>,
    action: &str,
    details: Value,
) {
    match activity::create_activity(
        pool,
        workspace_id,
        issue_id,
        actor_type,
        actor_id,
        action,
        &details,
        patchbay_db::dbid::new_v7(),
    )
    .await
    {
        Ok(Some(row)) => publish_activity(bus, event, row),
        Ok(None) => {}
        Err(error) => {
            tracing::error!(%error, %issue_id, action, "activity listener: create failed")
        }
    }
}

fn publish_activity(bus: &Bus, original: &Event, row: ActivityLog) {
    bus.publish(&Event {
        event_type: patchbay_protocol::EVENT_ACTIVITY_CREATED.to_string(),
        workspace_id: original.workspace_id.clone(),
        actor_type: original.actor_type.clone(),
        actor_id: original.actor_id.clone(),
        payload: json!({
            "issue_id": row.issue_id.map(|id| id.to_string()).unwrap_or_default(),
            "entry": {
                "type": "activity",
                "id": row.id,
                "actor_type": row.actor_type.unwrap_or_default(),
                "actor_id": row.actor_id.map(|id| id.to_string()).unwrap_or_default(),
                "action": row.action,
                "details": row.details,
                "created_at": crate::timefmt::rfc3339(row.created_at),
            },
        }),
        ..Default::default()
    });
}

fn scoped_issue(event: &Event) -> Option<IssueFields> {
    let workspace_id = event_workspace(event)?;
    let issue = event.payload.get("issue")?;
    let issue_workspace = uuid(issue, "workspace_id")?;
    if workspace_id != issue_workspace {
        tracing::warn!(%workspace_id, %issue_workspace, "event listener: issue workspace mismatch");
        return None;
    }
    Some(IssueFields {
        id: uuid(issue, "id")?,
        workspace_id,
        creator_type: string(issue, "creator_type"),
        creator_id: uuid(issue, "creator_id")?,
        assignee_type: optional_string(issue, "assignee_type"),
        assignee_id: uuid(issue, "assignee_id"),
        reviewer_type: optional_string(issue, "reviewer_type"),
        reviewer_id: uuid(issue, "reviewer_id"),
        description: optional_string(issue, "description"),
        title: string(issue, "title"),
        status: string(issue, "status"),
        priority: string(issue, "priority"),
        start_date: optional_string(issue, "start_date"),
        due_date: optional_string(issue, "due_date"),
    })
}

fn event_workspace(event: &Event) -> Option<Uuid> {
    event.workspace_id.parse().ok()
}

fn uuid(value: &Value, key: &str) -> Option<Uuid> {
    value.get(key)?.as_str()?.parse().ok()
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool) == Some(true)
}

fn insert_optional(details: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    if let Some(value) = value.and_then(Value::as_str) {
        details.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn insert_optional_str(details: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        details.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn is_assignment_recipient(user_type: &str) -> bool {
    matches!(user_type, "member" | "agent")
}

fn parse_mentions(content: &str) -> Vec<Mention> {
    static MENTION_RE: OnceLock<Regex> = OnceLock::new();
    let re = MENTION_RE.get_or_init(|| {
        Regex::new(r"\[@?(.+?)\]\(mention://(member|agent|team|issue|all)/([0-9a-fA-F-]+|all)\)")
            .expect("mention regex is valid")
    });
    let mut seen = HashSet::new();
    re.captures_iter(content)
        .filter_map(|captures| {
            let user_type = captures.get(2)?.as_str().to_string();
            let user_id = captures.get(3)?.as_str().to_string();
            seen.insert((user_type.clone(), user_id.clone()))
                .then_some(Mention { user_type, user_id })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_preserve_first_seen_order_and_deduplicate_by_type_and_id() {
        let content = "[@A](mention://member/11111111-1111-4111-8111-111111111111) \
            [@A again](mention://member/11111111-1111-4111-8111-111111111111) \
            [Bot](mention://agent/11111111-1111-4111-8111-111111111111)";
        assert_eq!(
            parse_mentions(content),
            vec![
                Mention {
                    user_type: "member".into(),
                    user_id: "11111111-1111-4111-8111-111111111111".into(),
                },
                Mention {
                    user_type: "agent".into(),
                    user_id: "11111111-1111-4111-8111-111111111111".into(),
                },
            ]
        );
    }

    #[test]
    fn issue_scope_rejects_cross_workspace_payloads() {
        let event = Event {
            workspace_id: "11111111-1111-4111-8111-111111111111".into(),
            payload: json!({
                "issue": {
                    "id": "22222222-2222-4222-8222-222222222222",
                    "workspace_id": "33333333-3333-4333-8333-333333333333",
                }
            }),
            ..Default::default()
        };
        assert!(scoped_issue(&event).is_none());
    }

    #[test]
    fn only_direct_assignee_types_are_subscriber_recipients() {
        assert!(is_assignment_recipient("member"));
        assert!(is_assignment_recipient("agent"));
        assert!(!is_assignment_recipient("team"));
        assert!(!is_assignment_recipient("system"));
    }
}
