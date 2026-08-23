//! Ordered event-bus to WebSocket fanout.
//!
//! The Go event bus invokes broadcaster methods synchronously, so publication
//! order is also wire order. Rust broadcasters are async; a single worker
//! preserves that contract without spawning a reorderable task per event.

use std::sync::Arc;

use cordy_events::{Bus, Event};
use cordy_protocol::events::{
    EVENT_INBOX_ARCHIVED, EVENT_INBOX_BATCH_ARCHIVED, EVENT_INBOX_BATCH_READ, EVENT_INBOX_NEW,
    EVENT_INBOX_READ, EVENT_INBOX_UNARCHIVED, EVENT_INVITATION_CREATED, EVENT_INVITATION_REVOKED,
    EVENT_ISSUE_UPDATED, EVENT_MEMBER_ADDED, EVENT_TASK_FAILED,
};
use cordy_realtime::{Broadcaster, M};
use serde_json::{json, Value};
use tokio::sync::mpsc;

const PERSONAL_EVENTS: &[&str] = &[
    EVENT_INBOX_NEW,
    EVENT_INBOX_READ,
    EVENT_INBOX_ARCHIVED,
    EVENT_INBOX_UNARCHIVED,
    EVENT_INBOX_BATCH_READ,
    EVENT_INBOX_BATCH_ARCHIVED,
    EVENT_INVITATION_CREATED,
    EVENT_INVITATION_REVOKED,
];

enum Command {
    User {
        event: Event,
        user_id: String,
        exclude_workspace: Option<String>,
    },
    Global(Event),
    Shutdown,
}

/// Owns the ordered fanout worker. `shutdown` drains every event already
/// published by the synchronous bus before joining the worker.
pub struct RealtimeForwarder {
    sender: mpsc::UnboundedSender<Command>,
    worker: tokio::task::JoinHandle<()>,
}

impl RealtimeForwarder {
    pub fn start(bus: &Bus, broadcaster: Arc<dyn Broadcaster>) -> Self {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        register_personal_listeners(bus, &sender);

        let global_sender = sender.clone();
        bus.subscribe_all(move |event| {
            if !PERSONAL_EVENTS.contains(&event.event_type.as_str()) {
                let _ = global_sender.send(Command::Global(event.clone()));
            }
        });

        let worker = tokio::spawn(async move {
            while let Some(command) = receiver.recv().await {
                match command {
                    Command::User {
                        event,
                        user_id,
                        exclude_workspace,
                    } => {
                        if let Some(frame) = event_frame(&event) {
                            M.record_event(&event.event_type);
                            broadcaster
                                .send_to_user(&user_id, &frame, exclude_workspace.as_deref())
                                .await;
                        }
                    }
                    Command::Global(event) => {
                        if let Some(frame) = event_frame(&event) {
                            if !event.workspace_id.is_empty() {
                                M.record_event(&event.event_type);
                                broadcaster
                                    .broadcast_to_workspace(&event.workspace_id, &frame)
                                    .await;
                            } else if event.event_type.starts_with("daemon:") {
                                M.record_event(&event.event_type);
                                broadcaster.broadcast(&frame).await;
                            }
                        }
                    }
                    Command::Shutdown => break,
                }
            }
        });

        Self { sender, worker }
    }

    pub async fn shutdown(self) {
        let _ = self.sender.send(Command::Shutdown);
        let _ = self.worker.await;
    }
}

fn register_personal_listeners(bus: &Bus, sender: &mpsc::UnboundedSender<Command>) {
    let tx = sender.clone();
    bus.subscribe(EVENT_INBOX_NEW, move |event| {
        if let Some(user_id) = event
            .payload
            .get("item")
            .and_then(|item| item.get("recipient_id"))
            .and_then(Value::as_str)
        {
            enqueue_user(&tx, event, user_id, None);
        }
    });

    for event_type in [
        EVENT_INBOX_READ,
        EVENT_INBOX_ARCHIVED,
        EVENT_INBOX_UNARCHIVED,
        EVENT_INBOX_BATCH_READ,
        EVENT_INBOX_BATCH_ARCHIVED,
    ] {
        let tx = sender.clone();
        bus.subscribe(event_type, move |event| {
            if let Some(user_id) = event.payload.get("recipient_id").and_then(Value::as_str) {
                enqueue_user(&tx, event, user_id, None);
            }
        });
    }

    let tx = sender.clone();
    bus.subscribe(EVENT_INVITATION_CREATED, move |event| {
        if let Some(user_id) = event
            .payload
            .get("invitation")
            .and_then(|invitation| invitation.get("invitee_user_id"))
            .and_then(Value::as_str)
        {
            enqueue_user(&tx, event, user_id, None);
        }
    });

    let tx = sender.clone();
    bus.subscribe(EVENT_INVITATION_REVOKED, move |event| {
        if let Some(user_id) = event.payload.get("invitee_user_id").and_then(Value::as_str) {
            enqueue_user(&tx, event, user_id, None);
        }
    });

    let tx = sender.clone();
    bus.subscribe(EVENT_MEMBER_ADDED, move |event| {
        if let Some(user_id) = event
            .payload
            .get("member")
            .and_then(|member| member.get("user_id"))
            .and_then(Value::as_str)
        {
            enqueue_user(&tx, event, user_id, Some(event.workspace_id.clone()));
        }
    });
}

fn enqueue_user(
    sender: &mpsc::UnboundedSender<Command>,
    event: &Event,
    user_id: &str,
    exclude_workspace: Option<String>,
) {
    if user_id.is_empty() {
        return;
    }
    let _ = sender.send(Command::User {
        event: event.clone(),
        user_id: user_id.to_string(),
        exclude_workspace,
    });
}

fn event_frame(event: &Event) -> Option<Vec<u8>> {
    serde_json::to_vec(&json!({
        "type": event.event_type,
        "payload": project_outbound(&event.event_type, &event.payload),
        "actor_id": event.actor_id,
        "actor_type": event.actor_type,
    }))
    .ok()
}

fn project_outbound(event_type: &str, payload: &Value) -> Value {
    let keys: &[&str] = match event_type {
        EVENT_ISSUE_UPDATED => &["prev_description", "prev_title"],
        EVENT_TASK_FAILED => &["error"],
        _ => return payload.clone(),
    };
    let mut projected = payload.clone();
    if let Some(object) = projected.as_object_mut() {
        for key in keys {
            object.remove(*key);
        }
    }
    projected
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordy_realtime::hub::Hub;
    use serde_json::json;

    fn event(event_type: &str, workspace_id: &str, payload: Value) -> Event {
        Event {
            event_type: event_type.into(),
            workspace_id: workspace_id.into(),
            actor_id: "actor-1".into(),
            actor_type: "member".into(),
            payload,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn workspace_and_personal_events_route_without_leaking() {
        let bus = Bus::new();
        let hub = Arc::new(Hub::new());
        let mut target = hub.register("u-target", "ws-1").1;
        let mut outsider = hub.register("u-other", "ws-1").1;
        let forwarder = RealtimeForwarder::start(&bus, hub);

        bus.publish(&event(
            EVENT_INBOX_READ,
            "ws-1",
            json!({"recipient_id": "u-target"}),
        ));
        bus.publish(&event("issue:created", "ws-1", json!({"id": "i-1"})));
        forwarder.shutdown().await;

        let personal: Value = serde_json::from_slice(&target.recv().await.unwrap()).unwrap();
        let workspace: Value = serde_json::from_slice(&target.recv().await.unwrap()).unwrap();
        assert_eq!(personal["type"], EVENT_INBOX_READ);
        assert_eq!(workspace["type"], "issue:created");
        let outsider_frame: Value =
            serde_json::from_slice(&outsider.recv().await.unwrap()).unwrap();
        assert_eq!(outsider_frame["type"], "issue:created");
        assert!(outsider.try_recv().is_err());
    }

    #[tokio::test]
    async fn member_added_reaches_other_workspace_once_and_target_workspace_once() {
        let bus = Bus::new();
        let hub = Arc::new(Hub::new());
        let mut invited_target_ws = hub.register("u-invited", "ws-new").1;
        let mut invited_other_ws = hub.register("u-invited", "ws-old").1;
        let mut existing_member = hub.register("u-existing", "ws-new").1;
        let forwarder = RealtimeForwarder::start(&bus, hub);

        bus.publish(&event(
            EVENT_MEMBER_ADDED,
            "ws-new",
            json!({"member": {"user_id": "u-invited"}}),
        ));
        forwarder.shutdown().await;

        assert!(invited_target_ws.recv().await.is_some());
        assert!(invited_other_ws.recv().await.is_some());
        assert!(existing_member.recv().await.is_some());
        assert!(invited_target_ws.try_recv().is_err());
        assert!(invited_other_ws.try_recv().is_err());
    }

    #[test]
    fn outbound_projection_is_non_mutating() {
        let payload = json!({
            "issue": {"description": "new"},
            "prev_description": "old",
            "prev_title": "before"
        });
        let projected = project_outbound(EVENT_ISSUE_UPDATED, &payload);
        assert!(projected.get("prev_description").is_none());
        assert!(projected.get("prev_title").is_none());
        assert_eq!(payload["prev_description"], "old");
    }
}
