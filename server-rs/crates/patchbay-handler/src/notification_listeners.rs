//! Inbox notification side effects.
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, OnceLock};

use patchbay_db::models::{InboxItem, Issue};
use patchbay_db::queries::{inbox, issue, member, notification_preference, subscriber, team};
use patchbay_events::{Bus, Event};
use regex::Regex;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
struct IssueFields {
    id: Uuid,
    workspace_id: Uuid,
    title: String,
    description: Option<String>,
    status: String,
    priority: String,
    assignee_type: Option<String>,
    assignee_id: Option<Uuid>,
    reviewer_type: Option<String>,
    reviewer_id: Option<Uuid>,
    start_date: Option<String>,
    due_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mention {
    user_type: String,
    user_id: String,
}

struct InboxSpec<'a> {
    recipient_type: &'a str,
    recipient_id: Uuid,
    issue_id: Uuid,
    issue_status: &'a str,
    notif_type: &'a str,
    severity: &'a str,
    title: &'a str,
    body: Option<&'a str>,
    details: &'a Value,
}

type ListenerFuture = std::pin::Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

pub(crate) async fn handle_event(pool: PgPool, bus: Arc<Bus>, event: Event) -> anyhow::Result<()> {
    match event.event_type.as_str() {
        patchbay_protocol::EVENT_ISSUE_CREATED => handle_issue_created(pool, bus, event).await,
        patchbay_protocol::EVENT_ISSUE_UPDATED => handle_issue_updated(pool, bus, event).await,
        patchbay_protocol::EVENT_COMMENT_CREATED => handle_comment_created(pool, bus, event).await,
        patchbay_protocol::EVENT_ISSUE_REACTION_ADDED => {
            handle_issue_reaction_added(pool, bus, event).await
        }
        patchbay_protocol::EVENT_REACTION_ADDED => handle_reaction_added(pool, bus, event).await,
        patchbay_protocol::EVENT_TASK_FAILED => handle_task_failed(pool, bus, event).await,
        _ => Ok(()),
    }
}

fn handle_issue_created(pool: PgPool, bus: Arc<Bus>, event: Event) -> ListenerFuture {
    Box::pin(async move {
        let Some(fields) = handler_issue(&event, true) else {
            return Ok(());
        };
        if issue::get_issue_in_workspace(&pool, fields.id, fields.workspace_id)
            .await?
            .is_none()
        {
            return Ok(());
        }
        let mut skip = HashSet::from_iter(event_actor(&event));
        if let (Some(recipient_type), Some(recipient_id)) =
            (fields.assignee_type.as_deref(), fields.assignee_id)
        {
            if is_assignment_recipient(recipient_type) {
                skip.insert(recipient_id);
                notify_direct(
                    &pool,
                    &bus,
                    &event,
                    InboxSpec {
                        recipient_type,
                        recipient_id,
                        issue_id: fields.id,
                        issue_status: &fields.status,
                        notif_type: "issue_assigned",
                        severity: "action_required",
                        title: &fields.title,
                        body: None,
                        details: &json!({}),
                    },
                )
                .await?;
            }
        }
        if let Some(description) = fields.description.as_deref() {
            notify_mentions(
                &pool,
                &bus,
                &event,
                parse_mentions(description),
                &fields,
                &skip,
                &json!({}),
            )
            .await?;
        }
        Ok(())
    })
}

fn handle_issue_updated(pool: PgPool, bus: Arc<Bus>, event: Event) -> ListenerFuture {
    Box::pin(async move {
        let Some(fields) = handler_issue(&event, false) else {
            return Ok(());
        };
        if issue::get_issue_in_workspace(&pool, fields.id, fields.workspace_id)
            .await?
            .is_none()
        {
            return Ok(());
        }
        if flag(&event.payload, "assignee_changed") {
            let mut details = Map::new();
            insert_string(
                &mut details,
                "prev_assignee_type",
                event.payload.get("prev_assignee_type"),
            );
            insert_string(
                &mut details,
                "prev_assignee_id",
                event.payload.get("prev_assignee_id"),
            );
            insert_str(
                &mut details,
                "new_assignee_type",
                fields.assignee_type.as_deref(),
            );
            let new_id = fields.assignee_id.map(|id| id.to_string());
            insert_str(&mut details, "new_assignee_id", new_id.as_deref());
            let details = Value::Object(details);

            if let (Some(recipient_type), Some(recipient_id)) =
                (fields.assignee_type.as_deref(), fields.assignee_id)
            {
                if is_assignment_recipient(recipient_type) {
                    notify_direct(
                        &pool,
                        &bus,
                        &event,
                        InboxSpec {
                            recipient_type,
                            recipient_id,
                            issue_id: fields.id,
                            issue_status: &fields.status,
                            notif_type: "issue_assigned",
                            severity: "action_required",
                            title: &fields.title,
                            body: None,
                            details: &details,
                        },
                    )
                    .await?;
                }
            }
            let previous_type = optional_string(&event.payload, "prev_assignee_type");
            let previous_id = uuid(&event.payload, "prev_assignee_id");
            if previous_type.as_deref() == Some("member") {
                if let Some(recipient_id) = previous_id {
                    notify_direct(
                        &pool,
                        &bus,
                        &event,
                        InboxSpec {
                            recipient_type: "member",
                            recipient_id,
                            issue_id: fields.id,
                            issue_status: &fields.status,
                            notif_type: "unassigned",
                            severity: "info",
                            title: &fields.title,
                            body: None,
                            details: &details,
                        },
                    )
                    .await?;
                }
            }
            let exclude = previous_id.into_iter().chain(fields.assignee_id).collect();
            notify_subscribers(
                &pool,
                &bus,
                &event,
                &fields,
                &exclude,
                "assignee_changed",
                "info",
                "",
                &details,
            )
            .await?;
        }
        if flag(&event.payload, "review_handoff") || flag(&event.payload, "reviewer_changed") {
            if let (Some(recipient_type), Some(recipient_id)) =
                (fields.reviewer_type.as_deref(), fields.reviewer_id)
            {
                if is_assignment_recipient(recipient_type) {
                    let reviewer_id = recipient_id.to_string();
                    let mut details = Map::new();
                    insert_str(&mut details, "new_assignee_type", Some(recipient_type));
                    insert_str(&mut details, "new_assignee_id", Some(reviewer_id.as_str()));
                    let details = Value::Object(details);
                    notify_direct(
                        &pool,
                        &bus,
                        &event,
                        InboxSpec {
                            recipient_type,
                            recipient_id,
                            issue_id: fields.id,
                            issue_status: &fields.status,
                            notif_type: "issue_assigned",
                            severity: "action_required",
                            title: &fields.title,
                            body: None,
                            details: &details,
                        },
                    )
                    .await?;
                }
            }
        }

        if flag(&event.payload, "status_changed") {
            let details =
                json!({"from": string(&event.payload, "prev_status"), "to": fields.status});
            notify_subscribers(
                &pool,
                &bus,
                &event,
                &fields,
                &HashSet::new(),
                "status_changed",
                "info",
                "",
                &details,
            )
            .await?;
            let effective = patchbay_service::issue_status::effective(
                &pool,
                fields.workspace_id,
                &fields.status,
            )
            .await;
            if matches!(effective.as_str(), "in_review" | "done" | "cancelled") {
                archive_task_failures(&pool, &bus, fields.workspace_id, fields.id).await?;
            }
        }
        if flag(&event.payload, "priority_changed") {
            let details =
                json!({"from": string(&event.payload, "prev_priority"), "to": fields.priority});
            notify_subscribers(
                &pool,
                &bus,
                &event,
                &fields,
                &HashSet::new(),
                "priority_changed",
                "info",
                "",
                &details,
            )
            .await?;
        }
        if flag(&event.payload, "start_date_changed") {
            let details = json!({"from": string(&event.payload, "prev_start_date"), "to": fields.start_date.as_deref().unwrap_or_default()});
            notify_subscribers(
                &pool,
                &bus,
                &event,
                &fields,
                &HashSet::new(),
                "start_date_changed",
                "info",
                "",
                &details,
            )
            .await?;
        }
        if flag(&event.payload, "due_date_changed") {
            let details = json!({"from": string(&event.payload, "prev_due_date"), "to": fields.due_date.as_deref().unwrap_or_default()});
            notify_subscribers(
                &pool,
                &bus,
                &event,
                &fields,
                &HashSet::new(),
                "due_date_changed",
                "info",
                "",
                &details,
            )
            .await?;
        }
        if flag(&event.payload, "description_changed") {
            let previous = optional_string(&event.payload, "prev_description")
                .map(|value| mention_keys(&parse_mentions(&value)))
                .unwrap_or_default();
            if let Some(description) = fields.description.as_deref() {
                let added = parse_mentions(description)
                    .into_iter()
                    .filter(|mention| {
                        !previous.contains(&(mention.user_type.clone(), mention.user_id.clone()))
                    })
                    .collect();
                let skip = HashSet::from_iter(event_actor(&event));
                notify_mentions(&pool, &bus, &event, added, &fields, &skip, &json!({})).await?;
            }
        }
        Ok(())
    })
}

fn handle_comment_created(pool: PgPool, bus: Arc<Bus>, event: Event) -> ListenerFuture {
    Box::pin(async move {
        let Some(comment) = event.payload.get("comment") else {
            return Ok(());
        };
        if string(comment, "author_type") == "system" {
            return Ok(());
        }
        let Some(issue_id) = uuid(comment, "issue_id") else {
            return Ok(());
        };
        let Some(issue) = scoped_db_issue(&pool, &event, issue_id).await? else {
            return Ok(());
        };
        let comment_id = optional_string(comment, "id");
        let content = string(comment, "content");
        let details = comment_id.map_or_else(|| json!({}), |id| json!({"comment_id": id}));
        let fields = fields_from_issue(issue);
        notify_subscribers(
            &pool,
            &bus,
            &event,
            &fields,
            &HashSet::new(),
            "new_comment",
            "info",
            &content,
            &details,
        )
        .await?;
        let skip = HashSet::from_iter(event_actor(&event));
        notify_mentions(
            &pool,
            &bus,
            &event,
            parse_mentions(&content),
            &fields,
            &skip,
            &details,
        )
        .await?;
        Ok(())
    })
}

fn handle_issue_reaction_added(pool: PgPool, bus: Arc<Bus>, event: Event) -> ListenerFuture {
    Box::pin(async move {
        let Some(issue_id) = uuid(&event.payload, "issue_id") else {
            return Ok(());
        };
        let Some(issue) = scoped_db_issue(&pool, &event, issue_id).await? else {
            return Ok(());
        };
        let Some(recipient_id) = uuid(&event.payload, "creator_id") else {
            return Ok(());
        };
        let recipient_type = string(&event.payload, "creator_type");
        if recipient_type.is_empty() {
            return Ok(());
        }
        let emoji = event
            .payload
            .get("reaction")
            .map(|r| string(r, "emoji"))
            .unwrap_or_default();
        let details = json!({"emoji": emoji});
        notify_direct(
            &pool,
            &bus,
            &event,
            InboxSpec {
                recipient_type: &recipient_type,
                recipient_id,
                issue_id,
                issue_status: &issue.status,
                notif_type: "reaction_added",
                severity: "info",
                title: &issue.title,
                body: None,
                details: &details,
            },
        )
        .await?;
        Ok(())
    })
}

fn handle_reaction_added(pool: PgPool, bus: Arc<Bus>, event: Event) -> ListenerFuture {
    Box::pin(async move {
        let Some(issue_id) = uuid(&event.payload, "issue_id") else {
            return Ok(());
        };
        let Some(issue) = scoped_db_issue(&pool, &event, issue_id).await? else {
            return Ok(());
        };
        let Some(recipient_id) = uuid(&event.payload, "comment_author_id") else {
            return Ok(());
        };
        let recipient_type = string(&event.payload, "comment_author_type");
        if recipient_type.is_empty() {
            return Ok(());
        }
        let emoji = event
            .payload
            .get("reaction")
            .map(|r| string(r, "emoji"))
            .unwrap_or_default();
        let mut details = Map::from_iter([("emoji".into(), Value::String(emoji))]);
        insert_string(&mut details, "comment_id", event.payload.get("comment_id"));
        let details = Value::Object(details);
        notify_direct(
            &pool,
            &bus,
            &event,
            InboxSpec {
                recipient_type: &recipient_type,
                recipient_id,
                issue_id,
                issue_status: &issue.status,
                notif_type: "reaction_added",
                severity: "info",
                title: &issue.title,
                body: None,
                details: &details,
            },
        )
        .await?;
        Ok(())
    })
}

fn handle_task_failed(pool: PgPool, bus: Arc<Bus>, mut event: Event) -> ListenerFuture {
    Box::pin(async move {
        let Some(issue_id) = uuid(&event.payload, "issue_id") else {
            return Ok(());
        };
        let Some(issue) = scoped_db_issue(&pool, &event, issue_id).await? else {
            return Ok(());
        };
        let Some(agent_id) = uuid(&event.payload, "agent_id") else {
            return Ok(());
        };
        event.actor_type = "agent".into();
        event.actor_id = agent_id.to_string();
        let fields = fields_from_issue(issue);
        notify_subscribers(
            &pool,
            &bus,
            &event,
            &fields,
            &HashSet::from([agent_id]),
            "task_failed",
            "action_required",
            "",
            &json!({}),
        )
        .await?;
        Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
async fn notify_subscribers(
    pool: &PgPool,
    bus: &Bus,
    event: &Event,
    fields: &IssueFields,
    exclude: &HashSet<Uuid>,
    notif_type: &str,
    severity: &str,
    body: &str,
    details: &Value,
) -> anyhow::Result<()> {
    let (notified, suppressed) = notify_issue_subscribers(
        pool, bus, event, fields, fields.id, fields.id, exclude, notif_type, severity, body,
        details,
    )
    .await?;
    if notif_type != "status_changed" {
        return Ok(());
    }
    let Some(child) = issue::get_issue(pool, fields.id).await? else {
        return Ok(());
    };
    if child.workspace_id != fields.workspace_id {
        return Ok(());
    }
    let Some(parent_id) = child.parent_issue_id else {
        return Ok(());
    };
    if issue::get_issue_in_workspace(pool, parent_id, fields.workspace_id)
        .await?
        .is_none()
    {
        return Ok(());
    }
    let mut parent_exclude = exclude.clone();
    parent_exclude.extend(notified);
    parent_exclude.extend(suppressed);
    let _ = notify_issue_subscribers(
        pool,
        bus,
        event,
        fields,
        parent_id,
        fields.id,
        &parent_exclude,
        notif_type,
        severity,
        body,
        details,
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn notify_issue_subscribers(
    pool: &PgPool,
    bus: &Bus,
    event: &Event,
    fields: &IssueFields,
    subscriber_issue_id: Uuid,
    target_issue_id: Uuid,
    exclude: &HashSet<Uuid>,
    notif_type: &str,
    severity: &str,
    body: &str,
    details: &Value,
) -> anyhow::Result<(HashSet<Uuid>, HashSet<Uuid>)> {
    let mut notified = HashSet::new();
    let mut suppressed = HashSet::new();
    let effective =
        patchbay_service::issue_status::effective(pool, fields.workspace_id, &fields.status).await;
    let subs = subscriber::list_issue_subscribers(pool, subscriber_issue_id).await?;
    let member_ids = subs
        .iter()
        .filter(|sub| sub.user_type == "member")
        .map(|sub| sub.user_id)
        .collect::<Vec<_>>();
    let prefs = load_preferences(pool, fields.workspace_id, member_ids).await?;
    let actor_id = event_actor(event);
    for sub in subs {
        if sub.user_type != "member"
            || actor_id == Some(sub.user_id)
            || exclude.contains(&sub.user_id)
        {
            continue;
        }
        if prefs
            .get(&sub.user_id)
            .is_some_and(|p| is_muted(p, notif_type))
        {
            continue;
        }
        if !deliver_to_subscriber(&sub.reason, notif_type, &effective) {
            suppressed.insert(sub.user_id);
            continue;
        }
        let spec = InboxSpec {
            recipient_type: "member",
            recipient_id: sub.user_id,
            issue_id: target_issue_id,
            issue_status: &effective,
            notif_type,
            severity,
            title: &fields.title,
            body: (!body.is_empty()).then_some(body),
            details,
        };
        if create_and_publish(pool, bus, event, spec).await? {
            notified.insert(sub.user_id);
        }
    }
    Ok((notified, suppressed))
}

async fn notify_direct(
    pool: &PgPool,
    bus: &Bus,
    event: &Event,
    spec: InboxSpec<'_>,
) -> anyhow::Result<()> {
    if event_actor(event) == Some(spec.recipient_id) {
        return Ok(());
    }
    let Some(workspace_id) = event_workspace(event) else {
        return Ok(());
    };
    if spec.recipient_type == "member" {
        let prefs = load_preferences(pool, workspace_id, vec![spec.recipient_id]).await?;
        if prefs
            .get(&spec.recipient_id)
            .is_some_and(|p| is_muted(p, spec.notif_type))
        {
            return Ok(());
        }
    }
    create_and_publish(pool, bus, event, spec).await?;
    Ok(())
}

async fn create_and_publish(
    pool: &PgPool,
    bus: &Bus,
    event: &Event,
    spec: InboxSpec<'_>,
) -> anyhow::Result<bool> {
    let Some(workspace_id) = event_workspace(event) else {
        return Ok(false);
    };
    let actor_type = (!event.actor_type.is_empty()).then_some(event.actor_type.as_str());
    let actor_id = if event.actor_id.is_empty() {
        None
    } else {
        event.actor_id.parse().ok()
    };
    let item = match inbox::create_inbox_item(
        pool,
        workspace_id,
        spec.recipient_type,
        spec.recipient_id,
        spec.notif_type,
        spec.severity,
        Some(spec.issue_id),
        spec.title,
        spec.body,
        actor_type,
        actor_id,
        spec.details,
        durable_coordination_inbox_id(event, &spec),
    )
    .await?
    {
        Some(item) => item,
        None => return Ok(false),
    };
    publish_inbox(bus, event, item, spec.issue_status);
    Ok(true)
}

async fn notify_mentions(
    pool: &PgPool,
    bus: &Bus,
    event: &Event,
    mentions: Vec<Mention>,
    fields: &IssueFields,
    skip: &HashSet<Uuid>,
    details: &Value,
) -> anyhow::Result<()> {
    let Some(workspace_id) = event_workspace(event) else {
        return Ok(());
    };
    let mut recipients = HashSet::new();
    let mut all = false;
    for mention in mentions {
        match mention.user_type.as_str() {
            "member" => {
                if let Ok(id) = mention.user_id.parse() {
                    recipients.insert(id);
                }
            }
            "all" => all = true,
            "team" => {
                let Ok(id) = mention.user_id.parse() else {
                    continue;
                };
                if team::get_team_in_workspace(pool, id, workspace_id)
                    .await?
                    .is_none()
                {
                    continue;
                }
                let members = team::list_team_members(pool, id).await?;
                recipients.extend(
                    members
                        .into_iter()
                        .filter(|m| m.member_type == "member")
                        .map(|m| m.member_id),
                );
            }
            _ => {}
        }
    }
    if all {
        let members = member::list_members(pool, workspace_id).await?;
        recipients.extend(members.into_iter().map(|m| m.user_id));
    }
    let actor = event_actor(event);
    let candidates = recipients
        .iter()
        .copied()
        .filter(|id| Some(*id) != actor && !skip.contains(id))
        .collect();
    let prefs = load_preferences(pool, workspace_id, candidates).await?;
    for recipient_id in recipients {
        if Some(recipient_id) == actor || skip.contains(&recipient_id) {
            continue;
        }
        if prefs
            .get(&recipient_id)
            .is_some_and(|p| is_muted(p, "mentioned"))
        {
            continue;
        }
        create_and_publish(
            pool,
            bus,
            event,
            InboxSpec {
                recipient_type: "member",
                recipient_id,
                issue_id: fields.id,
                issue_status: &fields.status,
                notif_type: "mentioned",
                severity: "info",
                title: &fields.title,
                body: None,
                details,
            },
        )
        .await?;
    }
    Ok(())
}

async fn archive_task_failures(
    pool: &PgPool,
    bus: &Bus,
    workspace_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<()> {
    let rows =
        inbox::archive_inbox_by_issue_and_type(pool, workspace_id, issue_id, "task_failed").await?;
    let mut counts = HashMap::<Uuid, usize>::new();
    for row in rows {
        if row.recipient_type == "member" {
            if let Some(id) = row.recipient_id {
                *counts.entry(id).or_default() += 1;
            }
        }
    }
    for (recipient_id, count) in counts {
        bus.publish(&Event {
            event_type: patchbay_protocol::EVENT_INBOX_BATCH_ARCHIVED.into(),
            workspace_id: workspace_id.to_string(),
            payload: json!({"recipient_id": recipient_id, "count": count as i64,
                "issue_id": issue_id, "reason": "issue_status_terminal"}),
            ..Default::default()
        });
    }
    Ok(())
}

async fn load_preferences(
    pool: &PgPool,
    workspace_id: Uuid,
    user_ids: Vec<Uuid>,
) -> anyhow::Result<HashMap<Uuid, HashMap<String, String>>> {
    if user_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = notification_preference::list_notification_preferences_by_users(
        pool,
        workspace_id,
        user_ids,
    )
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            serde_json::from_value(row.preferences)
                .ok()
                .map(|prefs| (row.user_id, prefs))
        })
        .collect())
}

fn publish_inbox(bus: &Bus, original: &Event, item: InboxItem, issue_status: &str) {
    bus.publish(&Event {
        event_type: patchbay_protocol::EVENT_INBOX_NEW.into(),
        workspace_id: original.workspace_id.clone(),
        actor_type: original.actor_type.clone(),
        actor_id: original.actor_id.clone(),
        payload: json!({"item": {
            "id": item.id, "workspace_id": item.workspace_id,
            "recipient_type": item.recipient_type, "recipient_id": item.recipient_id,
            "type": item.type_, "severity": item.severity,
            "issue_id": item.issue_id, "title": item.title, "body": item.body,
            "read": item.read, "archived": item.archived,
            "created_at": crate::timefmt::rfc3339(item.created_at),
            "actor_type": item.actor_type, "actor_id": item.actor_id,
            "details": item.details.unwrap_or_else(|| json!({})),
            "issue_status": issue_status,
        }}),
        ..Default::default()
    });
}

fn handler_issue(event: &Event, created: bool) -> Option<IssueFields> {
    let value = event.payload.get("issue")?;
    let object = value.as_object()?;
    if created && !object.contains_key("labels") {
        return None;
    }
    if !created && !event.payload.as_object()?.contains_key("priority_changed") {
        return None;
    }
    let workspace_id = event_workspace(event)?;
    if uuid(value, "workspace_id")? != workspace_id {
        return None;
    }
    Some(IssueFields {
        id: uuid(value, "id")?,
        workspace_id,
        title: string(value, "title"),
        description: optional_string(value, "description"),
        status: string(value, "status"),
        priority: string(value, "priority"),
        assignee_type: optional_string(value, "assignee_type"),
        assignee_id: uuid(value, "assignee_id"),
        reviewer_type: optional_string(value, "reviewer_type"),
        reviewer_id: uuid(value, "reviewer_id"),
        start_date: optional_string(value, "start_date"),
        due_date: optional_string(value, "due_date"),
    })
}

async fn scoped_db_issue(pool: &PgPool, event: &Event, id: Uuid) -> anyhow::Result<Option<Issue>> {
    let Some(workspace_id) = event_workspace(event) else {
        return Ok(None);
    };
    issue::get_issue_in_workspace(pool, id, workspace_id).await
}

fn fields_from_issue(issue: Issue) -> IssueFields {
    IssueFields {
        id: issue.id,
        workspace_id: issue.workspace_id,
        title: issue.title,
        description: issue.description,
        status: issue.status,
        priority: issue.priority,
        assignee_type: issue.assignee_type,
        assignee_id: issue.assignee_id,
        reviewer_type: issue.reviewer_type,
        reviewer_id: issue.reviewer_id,
        start_date: issue.start_date.map(|d| d.to_string()),
        due_date: issue.due_date.map(|d| d.to_string()),
    }
}

fn event_workspace(event: &Event) -> Option<Uuid> {
    event.workspace_id.parse().ok()
}
fn event_actor(event: &Event) -> Option<Uuid> {
    event.actor_id.parse().ok()
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
fn insert_string(map: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    if let Some(value) = value.and_then(Value::as_str) {
        map.insert(key.into(), value.into());
    }
}
fn insert_str(map: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        map.insert(key.into(), value.into());
    }
}
fn is_assignment_recipient(value: &str) -> bool {
    matches!(value, "member" | "agent")
}

fn durable_coordination_inbox_id(event: &Event, spec: &InboxSpec<'_>) -> Uuid {
    let Some(event_id) = event
        .payload
        .get("coordination_event_id")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<Uuid>().ok())
    else {
        return patchbay_db::dbid::new_v7();
    };
    let publication = coordination_publication(event);
    let transition = coordination_reviewer_transition(event, publication);
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!(
            "patchbay:coordination:inbox:{event_id}:{publication}{transition}:{}:{}:{}:{}",
            spec.recipient_type, spec.recipient_id, spec.notif_type, spec.issue_id
        )
        .as_bytes(),
    )
}

fn coordination_publication(event: &Event) -> &str {
    event
        .payload
        .get("coordination_publication")
        .and_then(Value::as_str)
        .unwrap_or(if flag(&event.payload, "review_handoff") {
            "review_handoff"
        } else if flag(&event.payload, "reviewer_changed") {
            "reviewer_replacement"
        } else {
            "coordination"
        })
}

fn coordination_reviewer_transition(event: &Event, publication: &str) -> String {
    if !matches!(publication, "review_handoff" | "reviewer_replacement") {
        return String::new();
    }
    let (previous_type_key, previous_id_key) = if publication == "review_handoff" {
        ("prev_assignee_type", "prev_assignee_id")
    } else {
        ("prev_reviewer_type", "prev_reviewer_id")
    };
    let previous_type = event
        .payload
        .get(previous_type_key)
        .and_then(Value::as_str)
        .unwrap_or_default();
    let previous_id = event
        .payload
        .get(previous_id_key)
        .and_then(Value::as_str)
        .unwrap_or_default();
    let issue = event.payload.get("issue");
    let reviewer_type = issue
        .and_then(|issue| issue.get("reviewer_type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let reviewer_id = issue
        .and_then(|issue| issue.get("reviewer_id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!(":{previous_type}:{previous_id}->{reviewer_type}:{reviewer_id}")
}

fn is_muted(prefs: &HashMap<String, String>, notif_type: &str) -> bool {
    let group = match notif_type {
        "issue_assigned" | "unassigned" | "assignee_changed" => "assignments",
        "status_changed" => "status_changes",
        "new_comment" => "comments",
        "mentioned" => "mentions",
        "priority_changed" | "start_date_changed" | "due_date_changed" => "updates",
        "task_completed" | "task_failed" | "agent_blocked" | "agent_completed" => "agent_activity",
        _ => return false,
    };
    prefs.get(group).is_some_and(|value| value == "muted")
}

fn deliver_to_subscriber(reason: &str, notif_type: &str, status: &str) -> bool {
    if reason != "delegated" {
        return true;
    }
    if matches!(notif_type, "mentioned" | "task_failed" | "agent_blocked") {
        return true;
    }
    notif_type == "status_changed"
        && matches!(status, "in_review" | "done" | "cancelled" | "blocked")
}

fn parse_mentions(content: &str) -> Vec<Mention> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"\[@?(.+?)\]\(mention://(member|agent|team|issue|all)/([0-9a-fA-F-]+|all)\)")
            .expect("mention regex is valid")
    });
    let mut seen = HashSet::new();
    re.captures_iter(content)
        .filter_map(|capture| {
            let user_type = capture.get(2)?.as_str().to_string();
            let user_id = capture.get(3)?.as_str().to_string();
            seen.insert((user_type.clone(), user_id.clone()))
                .then_some(Mention { user_type, user_id })
        })
        .collect()
}

fn mention_keys(mentions: &[Mention]) -> HashSet<(String, String)> {
    mentions
        .iter()
        .map(|m| (m.user_type.clone(), m.user_id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegated_delivery_matches_go_tier() {
        for notif_type in ["mentioned", "task_failed", "agent_blocked"] {
            assert!(deliver_to_subscriber("delegated", notif_type, "todo"));
        }
        for status in ["in_review", "done", "cancelled", "blocked"] {
            assert!(deliver_to_subscriber("delegated", "status_changed", status));
        }
        assert!(!deliver_to_subscriber(
            "delegated",
            "new_comment",
            "in_review"
        ));
        assert!(!deliver_to_subscriber(
            "delegated",
            "status_changed",
            "in_progress"
        ));
        assert!(deliver_to_subscriber("creator", "new_comment", "todo"));
    }

    #[test]
    fn preferences_only_mute_mapped_groups() {
        let prefs = HashMap::from([
            ("comments".into(), "muted".into()),
            ("mentions".into(), "all".into()),
        ]);
        assert!(is_muted(&prefs, "new_comment"));
        assert!(!is_muted(&prefs, "mentioned"));
        assert!(!is_muted(&prefs, "reaction_added"));
    }

    #[test]
    fn mentions_deduplicate_by_type_and_id() {
        let id = "11111111-1111-4111-8111-111111111111";
        assert_eq!(
            parse_mentions(&format!(
                "[@A](mention://member/{id}) [@A](mention://member/{id}) [@S](mention://team/{id})"
            ))
            .len(),
            2
        );
    }

    #[test]
    fn reviewer_replacement_notifications_are_idempotent_per_recipient() {
        let previous_reviewer_id = "55555555-5555-4555-8555-555555555555";
        let next_reviewer_id = "66666666-6666-4666-8666-666666666666";
        let event = Event {
            task_id: "11111111-1111-4111-8111-111111111111".into(),
            payload: json!({
                "reviewer_changed": true,
                "coordination_publication": "reviewer_replacement",
                "coordination_event_id": "11111111-1111-4111-8111-111111111111",
                "prev_reviewer_type": "agent",
                "prev_reviewer_id": previous_reviewer_id,
                "issue": {
                    "reviewer_type": "agent",
                    "reviewer_id": next_reviewer_id,
                },
            }),
            ..Default::default()
        };
        let reviewer_id = Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap();
        let issue_id = Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap();
        let details = json!({});
        let spec = InboxSpec {
            recipient_type: "agent",
            recipient_id: reviewer_id,
            issue_id,
            issue_status: "in_review",
            notif_type: "issue_assigned",
            severity: "action_required",
            title: "Issue",
            body: None,
            details: &details,
        };
        assert_eq!(
            durable_coordination_inbox_id(&event, &spec),
            durable_coordination_inbox_id(&event, &spec)
        );
        let other = InboxSpec {
            recipient_type: spec.recipient_type,
            recipient_id: Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap(),
            issue_id: spec.issue_id,
            issue_status: spec.issue_status,
            notif_type: spec.notif_type,
            severity: spec.severity,
            title: spec.title,
            body: spec.body,
            details: spec.details,
        };
        assert_ne!(
            durable_coordination_inbox_id(&event, &spec),
            durable_coordination_inbox_id(&event, &other)
        );
        let replacement_after_that = Event {
            payload: json!({
                "reviewer_changed": true,
                "coordination_publication": "reviewer_replacement",
                "coordination_event_id": "11111111-1111-4111-8111-111111111111",
                "prev_reviewer_type": "agent",
                "prev_reviewer_id": next_reviewer_id,
                "issue": {
                    "reviewer_type": "agent",
                    "reviewer_id": "77777777-7777-4777-8777-777777777777",
                },
            }),
            ..Default::default()
        };
        assert_ne!(
            durable_coordination_inbox_id(&event, &spec),
            durable_coordination_inbox_id(&replacement_after_that, &spec)
        );
    }
}
