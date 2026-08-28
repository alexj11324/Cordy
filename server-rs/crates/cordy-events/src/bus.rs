//! In-process synchronous pub/sub event bus.
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, RwLock};

use serde_json::Value;

/// Domain event published by handlers or services.
#[derive(Debug, Clone, Default)]
pub struct Event {
    /// e.g. "issue:created", "inbox:new".
    pub event_type: String,
    /// Routes to the correct Hub room.
    pub workspace_id: String,
    /// "member", "agent", or "system".
    pub actor_type: String,
    pub actor_id: String,
    /// JSON-serializable payload, same shape as the current WS payloads.
    pub payload: Value,

    /// Optional scope hints used by the realtime fanout layer to route the
    /// event to a more specific scope than `workspace:{WorkspaceID}`. When
    /// set these tell the listener which Redis stream / Hub room to publish
    /// on without re-deserializing Payload (MUL-1138 phase 1).
    pub task_id: String,
    pub chat_session_id: String,
}

/// A function that processes an event.
pub type Handler = Arc<dyn Fn(&Event) + Send + Sync>;

#[derive(Default)]
struct BusInner {
    listeners: HashMap<String, Vec<Handler>>,
    global_handlers: Vec<Handler>,
}

/// In-process synchronous pub/sub event bus.
///
/// Handlers run synchronously in registration order; a panic in one handler
/// is contained and logged so the remaining handlers still execute.
#[derive(Default)]
pub struct Bus {
    inner: RwLock<BusInner>,
}

impl Bus {
    /// Creates a new event bus.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a handler for a given event type. Handlers are called
    /// synchronously in registration order.
    pub fn subscribe<F>(&self, event_type: &str, handler: F)
    where
        F: Fn(&Event) + Send + Sync + 'static,
    {
        self.subscribe_handler(event_type, Arc::new(handler));
    }

    /// Arc-carrying variant of [`subscribe`] for pre-built handlers.
    pub fn subscribe_handler(&self, event_type: &str, handler: Handler) {
        self.inner
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .listeners
            .entry(event_type.to_string())
            .or_default()
            .push(handler);
    }

    /// Registers a handler that receives ALL events regardless of type.
    /// Global handlers are called after type-specific handlers.
    pub fn subscribe_all<F>(&self, handler: F)
    where
        F: Fn(&Event) + Send + Sync + 'static,
    {
        self.inner
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .global_handlers
            .push(Arc::new(handler));
    }

    /// Dispatches an event to all registered handlers for that event type.
    /// Type-specific handlers run first, then global (`subscribe_all`)
    /// handlers. Each handler is called synchronously; panics in individual
    /// handlers are recovered so one failing handler does not prevent others
    /// from executing.
    pub fn publish(&self, event: &Event) {
        let (handlers, globals) = {
            let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
            (
                inner
                    .listeners
                    .get(&event.event_type)
                    .cloned()
                    .unwrap_or_default(),
                inner.global_handlers.clone(),
            )
        };

        for h in &handlers {
            dispatch(h, event);
        }
        for h in &globals {
            dispatch(h, event);
        }
    }
}

fn dispatch(handler: &Handler, event: &Event) {
    let result = catch_unwind(AssertUnwindSafe(|| handler(event)));
    if let Err(panic_payload) = result {
        let detail = panic_payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| panic_payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        tracing::error!(
            event_type = %event.event_type,
            recovered = %detail,
            "panic in event listener"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn shared_log() -> Arc<Mutex<Vec<&'static str>>> {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn event_of(event_type: &str) -> Event {
        Event {
            event_type: event_type.to_string(),
            workspace_id: "ws-1".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn typed_subscription_receives_only_matching_events() {
        let bus = Bus::new();
        let hits = Arc::new(Mutex::new(0u32));

        let h = hits.clone();
        bus.subscribe("issue:created", move |_e| {
            *h.lock().unwrap() += 1;
        });

        bus.publish(&event_of("issue:created"));
        bus.publish(&event_of("issue:updated"));
        bus.publish(&event_of("issue:created"));

        assert_eq!(*hits.lock().unwrap(), 2);
    }

    #[test]
    fn multiple_handlers_run_in_registration_order() {
        let bus = Bus::new();
        let log = shared_log();

        let l1 = log.clone();
        bus.subscribe("evt", move |_| l1.lock().unwrap().push("first"));
        let l2 = log.clone();
        bus.subscribe("evt", move |_| l2.lock().unwrap().push("second"));

        bus.publish(&event_of("evt"));
        assert_eq!(*log.lock().unwrap(), vec!["first", "second"]);
    }

    #[test]
    fn global_handlers_run_after_typed_ones() {
        let bus = Bus::new();
        let log = shared_log();

        let l1 = log.clone();
        bus.subscribe("evt", move |_| l1.lock().unwrap().push("typed"));
        let l2 = log.clone();
        bus.subscribe_all(move |_| l2.lock().unwrap().push("global"));

        bus.publish(&event_of("evt"));
        assert_eq!(*log.lock().unwrap(), vec!["typed", "global"]);
    }

    #[test]
    fn panicking_handler_does_not_block_others() {
        let bus = Bus::new();
        let hits = Arc::new(Mutex::new(0u32));

        bus.subscribe("evt", |_| panic!("boom"));
        let h = hits.clone();
        bus.subscribe("evt", move |_| {
            *h.lock().unwrap() += 1;
        });

        bus.publish(&event_of("evt"));
        assert_eq!(*hits.lock().unwrap(), 1);
    }

    #[test]
    fn publish_without_listeners_is_safe() {
        let bus = Bus::new();
        bus.publish(&event_of("nobody:listens"));
    }

    #[test]
    fn event_carries_scope_hints() {
        let bus = Bus::new();
        let seen = Arc::new(Mutex::new(None));
        let s = seen.clone();
        bus.subscribe("task:update", move |e| {
            *s.lock().unwrap() = Some((e.task_id.clone(), e.chat_session_id.clone()));
        });

        bus.publish(&Event {
            event_type: "task:update".into(),
            workspace_id: "ws-1".into(),
            actor_type: "agent".into(),
            payload: serde_json::json!({"k": "v"}),
            task_id: "t-9".into(),
            chat_session_id: "c-7".into(),
            ..Default::default()
        });

        assert_eq!(
            seen.lock().unwrap().clone(),
            Some(("t-9".to_string(), "c-7".to_string()))
        );
    }
}
