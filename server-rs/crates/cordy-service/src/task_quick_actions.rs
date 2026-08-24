//! Chat quick-actions TaskService methods — port of
//! `service/chat_quick_actions.go` and `service/chat_quick_actions_generate.go`
//! (the service-method half; the pure parsing/rendering half lives in
//! `chat_quick_actions.rs`).

use std::sync::atomic::Ordering;
use std::sync::Arc;

use uuid::Uuid;

use cordy_db::models::{AgentTaskQueue, ChatMessage};
use cordy_db::queries::chat::{
    get_chat_message_by_task_assistant, list_chat_messages_page,
    set_chat_message_quick_actions_by_task, task_has_channel_ingested_messages,
};
use cordy_protocol::messages::ChatQuickActionsPayload;

use crate::chat_quick_actions::{
    collect_previous_chat_quick_actions, parse_chat_quick_actions_output,
    render_chat_quick_actions_context, select_chat_quick_actions_context, ChatQuickAction,
    ChatQuickActionsOrigin, CHAT_QUICK_ACTIONS_CONTEXT_MESSAGES,
    CHAT_QUICK_ACTIONS_MAX_COMPLETION_TOKENS, CHAT_QUICK_ACTIONS_MAX_CONCURRENT,
    CHAT_QUICK_ACTIONS_SYSTEM_PROMPT, CHAT_QUICK_ACTIONS_TEMPERATURE, CHAT_QUICK_ACTIONS_TIMEOUT,
};
use crate::redact;
use crate::task_service::TaskService;
use crate::task_service::TaskServiceError;

pub(crate) async fn chat_quick_actions_eligible(
    svc: &TaskService,
    task: &AgentTaskQueue,
    msg: Option<&ChatMessage>,
) -> bool {
    let enabled = match &svc.quick_actions {
        Some(qa) => qa.enabled(),
        None => false,
    };
    if !enabled {
        return false;
    }
    if task.chat_session_id.is_none() {
        return false;
    }
    // Only an ordinary reply can seed suggestions: a no_response outcome has
    // nothing to build on, and an attachment-only reply has no text to anchor in.
    let Some(msg) = msg else {
        return false;
    };
    if msg.message_kind != cordy_protocol::CHAT_MESSAGE_KIND_MESSAGE {
        return false;
    }
    if msg.content.trim().is_empty() {
        return false;
    }
    // Channel-backed sessions (Slack / Lark) have no pill surface. Same
    // discriminator write_chat_completion_outcome uses: the immutable
    // channel_ingested stamp on the turn's owned input batch. A NULL owner is
    // an agent-initiated intro turn, which does render in Chat.
    if let Some(input_owner) = task.chat_input_task_id {
        match task_has_channel_ingested_messages(&svc.pool, input_owner).await {
            Ok(Some(true)) | Err(_) => return false,
            _ => {}
        }
    }
    true
}

/// Attaches a suggestion pass's raw output to the completed turn's assistant
/// message and broadcasts chat:quick_actions. Best-effort semantics: an
/// unparseable or empty result still broadcasts the row's current (usually
/// empty) actions so pending placeholders resolve. A turn that never wrote an
/// assistant row (no_response, channel empty-drop) returns silently — no
/// client is waiting in that case, because the pending flag is only ever
/// raised for a written ordinary message.
///
/// `failed` marks the broadcast as a failure resolution (an explicit refresh
/// whose regeneration failed): the pills stay unchanged but the client shows a
/// "couldn't refresh" notice instead of treating them as freshly generated.
pub(crate) async fn supplement_chat_quick_actions(
    svc: &TaskService,
    task: &AgentTaskQueue,
    raw: &str,
    failed: bool,
) -> Result<(), TaskServiceError> {
    fn map_err(err: anyhow::Error) -> TaskServiceError {
        TaskServiceError::Sql(crate::task_service::downcast_sqlx(err))
    }

    if task.chat_session_id.is_none() {
        return Ok(());
    }
    let mut actions = parse_chat_quick_actions_output(raw);
    for action in &mut actions {
        action.label = redact::text(&action.label);
        action.prompt = redact::text(&action.prompt);
    }

    let msg = if !actions.is_empty() {
        let encoded = serde_json::to_value(&actions)
            .map_err(|e| TaskServiceError::Internal(format!("marshal chat quick actions: {e}")))?;
        set_chat_message_quick_actions_by_task(&svc.pool, task.id, &encoded)
            .await
            .map_err(map_err)?
    } else {
        get_chat_message_by_task_assistant(&svc.pool, task.id)
            .await
            .map_err(map_err)?
    };
    let Some(msg) = msg else {
        // ErrNoRows equivalent — nothing to attach to or announce.
        return Ok(());
    };

    let Some(workspace_id) = svc.resolve_task_workspace_id(task).await else {
        return Ok(());
    };
    let quick_actions: Vec<ChatQuickAction> =
        serde_json::from_value(msg.quick_actions.clone()).unwrap_or_default();
    let payload = ChatQuickActionsPayload {
        chat_session_id: task
            .chat_session_id
            .map(|u| u.to_string())
            .unwrap_or_default(),
        task_id: task.id.to_string(),
        message_id: msg.id.to_string(),
        quick_actions,
        failed,
    };
    svc.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_CHAT_QUICK_ACTIONS.to_string(),
        workspace_id,
        actor_type: "system".to_string(),
        actor_id: String::new(),
        payload: serde_json::to_value(&payload)
            .map_err(|e| TaskServiceError::Internal(e.to_string()))?,
        task_id: String::new(),
        chat_session_id: task
            .chat_session_id
            .map(|u| u.to_string())
            .unwrap_or_default(),
    });
    Ok(())
}

impl TaskService {
    /// Runs one suggestion pass for a completed chat turn and attaches the
    /// result to that turn's assistant row, broadcasting chat:quick_actions.
    /// It is the synchronous core shared by the automatic post-completion pass
    /// and the explicit refresh.
    ///
    /// `origin` decides failure reporting: a refresh surfaces failed=true so
    /// the user who pressed the button learns it did not work, while the
    /// automatic pass resolves the placeholder quietly.
    pub async fn generate_chat_quick_actions_for_task(
        &self,
        task: &AgentTaskQueue,
        origin: ChatQuickActionsOrigin,
    ) -> Result<(), crate::task_service::TaskServiceError> {
        use crate::task_service::TaskServiceError;

        let enabled = match &self.quick_actions {
            Some(qa) => qa.enabled(),
            None => false,
        };
        if !enabled || task.chat_session_id.is_none() {
            return Ok(());
        }

        // Resolve the target turn FIRST and anchor everything to it. The
        // context window, the write, and the broadcast must all describe the
        // same assistant message: this runs detached from the completion path,
        // so by the time it executes the session may already have moved on by
        // a full turn.
        let target = get_chat_message_by_task_assistant(&self.pool, task.id)
            .await
            .map_err(|e| TaskServiceError::Sql(crate::task_service::downcast_sqlx(e)))?;
        let Some(target) = target else {
            // No assistant row to attach to (no_response, channel empty-drop).
            return Ok(());
        };
        if target.message_kind != cordy_protocol::CHAT_MESSAGE_KIND_MESSAGE
            || target.content.trim().is_empty()
        {
            // Nothing to build suggestions on. Resolve the placeholder rather
            // than leaving the client's skeleton to time out.
            return supplement_chat_quick_actions(self, task, "", false).await;
        }

        let prompt = self.build_chat_quick_actions_prompt(task, &target).await?;

        let raw = self
            .quick_actions
            .as_ref()
            .expect("enabled gate checked above")
            .generate_json(
                "",
                CHAT_QUICK_ACTIONS_SYSTEM_PROMPT,
                &prompt,
                CHAT_QUICK_ACTIONS_TEMPERATURE,
                CHAT_QUICK_ACTIONS_MAX_COMPLETION_TOKENS,
            )
            .await;
        match raw {
            Ok(raw) => supplement_chat_quick_actions(self, task, &raw, false).await,
            Err(gen_err) => {
                // Resolve the placeholder either way; only an explicit refresh
                // reports the failure to the user (see origin).
                if let Err(supp_err) = supplement_chat_quick_actions(
                    self,
                    task,
                    "",
                    origin == ChatQuickActionsOrigin::Refresh,
                )
                .await
                {
                    tracing::warn!(
                        task_id = %task.id,
                        error = %supp_err,
                        "chat quick actions failure broadcast failed"
                    );
                }
                Err(TaskServiceError::Internal(format!(
                    "generate chat quick actions: {gen_err}"
                )))
            }
        }
    }

    /// Runs `generate_chat_quick_actions_for_task` on a detached task and
    /// returns immediately. Used on the completion path, where the user's
    /// reply is already delivered and must never wait on this.
    ///
    /// Two admission gates before anything is spawned:
    ///   - One pass per chat session. Concurrent passes on one session would
    ///     race to write the same row and burn duplicate spend for a single
    ///     visible outcome.
    ///   - A process-wide ceiling (CHAT_QUICK_ACTIONS_MAX_CONCURRENT).
    ///
    /// A rejected pass still resolves the client's placeholder, so a skeleton
    /// never hangs waiting for work that was never started.
    pub fn generate_chat_quick_actions_async(
        self: &Arc<Self>,
        task: AgentTaskQueue,
        origin: ChatQuickActionsOrigin,
    ) {
        let enabled = match &self.quick_actions {
            Some(qa) => qa.enabled(),
            None => false,
        };
        if !enabled {
            return;
        }
        let Some(session_id) = task.chat_session_id else {
            return;
        };

        let admitted = {
            let mut guard = self
                .quick_actions_in_flight
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            match guard.entry(session_id) {
                std::collections::hash_map::Entry::Occupied(_) => {
                    tracing::info!(
                        chat_session_id = %session_id,
                        task_id = %task.id,
                        "chat quick actions pass skipped: one already running for this session"
                    );
                    None
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    if self.quick_actions_running.load(Ordering::Relaxed) + 1
                        > CHAT_QUICK_ACTIONS_MAX_CONCURRENT
                    {
                        tracing::warn!(
                            ceiling = CHAT_QUICK_ACTIONS_MAX_CONCURRENT,
                            task_id = %task.id,
                            "chat quick actions pass shed: process-wide concurrency ceiling reached"
                        );
                        None
                    } else {
                        slot.insert(());
                        self.quick_actions_running.fetch_add(1, Ordering::Relaxed);
                        Some(())
                    }
                }
            }
        };
        if admitted.is_none() {
            self.resolve_chat_quick_actions_placeholder(task);
            return;
        }

        let svc = Arc::clone(self);
        self.spawn_side_effect(async move {
            let outcome = tokio::time::timeout(CHAT_QUICK_ACTIONS_TIMEOUT, async {
                svc.generate_chat_quick_actions_for_task(&task, origin)
                    .await
            })
            .await;
            svc.quick_actions_in_flight
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&session_id);
            svc.quick_actions_running.fetch_sub(1, Ordering::Relaxed);
            match outcome {
                Ok(Ok(())) => {}
                Ok(Err(err)) => tracing::warn!(
                    task_id = %task.id,
                    error = %err,
                    "chat quick actions generation failed"
                ),
                Err(_) => tracing::warn!(
                    task_id = %task.id,
                    "chat quick actions generation timed out"
                ),
            }
        });
    }

    /// Reports whether a pass is already running for this session. The refresh
    /// endpoint checks it so a duplicate request is refused with a 409 the
    /// client can act on, instead of being accepted and silently dropped by
    /// the admission gate in `generate_chat_quick_actions_async`.
    pub fn chat_quick_actions_in_flight(&self, session_id: Uuid) -> bool {
        self.quick_actions_in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&session_id)
    }

    /// Ends a client's pending skeleton without generating anything,
    /// rebroadcasting the turn's current pills. Used when a pass is refused
    /// admission — the placeholder was already raised by chat:done, and
    /// leaving it to expire would show a spinner for work that never started.
    pub fn resolve_chat_quick_actions_placeholder(self: &Arc<Self>, task: AgentTaskQueue) {
        let svc = Arc::clone(self);
        self.spawn_side_effect(async move {
            if let Err(err) = supplement_chat_quick_actions(&svc, &task, "", false).await {
                tracing::warn!(
                    task_id = %task.id,
                    error = %err,
                    "chat quick actions placeholder resolve failed"
                );
            }
        });
    }

    /// Renders the pass's user message from the window of conversation ENDING
    /// AT `target`, which is the assistant turn the suggestions will be
    /// attached to.
    ///
    /// Anchoring on the target (rather than re-reading the session's newest
    /// messages) is what keeps an async pass correct: a turn that lands between
    /// the completion callback and this read would otherwise supply the context
    /// while the result is still written to the older turn, and a newly-sent
    /// user message would leave the window ending on a user row with no reply
    /// to build on.
    async fn build_chat_quick_actions_prompt(
        &self,
        task: &AgentTaskQueue,
        target: &ChatMessage,
    ) -> Result<String, crate::task_service::TaskServiceError> {
        // Strictly older than target, newest-first; reversed inside select.
        // Over-fetch so dropped rows (no_response, failures) don't shrink the
        // window below the intended turn count.
        let rows = list_chat_messages_page(
            &self.pool,
            target.chat_session_id,
            (CHAT_QUICK_ACTIONS_CONTEXT_MESSAGES * 2) as i32,
            Some(target.created_at),
            target.id,
        )
        .await
        .map_err(|e| {
            crate::task_service::TaskServiceError::Internal(format!(
                "load chat messages for quick actions: {e}"
            ))
        })?;
        // Auto-retry replies use the child task ID while their input keeps
        // the root; the owner id covers both.
        let msgs = select_chat_quick_actions_context(
            &rows,
            target,
            Some(crate::task_service::chat_input_owner_id(task)),
        );
        Ok(render_chat_quick_actions_context(
            &msgs,
            &collect_previous_chat_quick_actions(&msgs),
        ))
    }
}
