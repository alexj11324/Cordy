//! Reply-origin check: did a completed chat task take its input from an
//! external channel?
//!

use cordy_db::queries::chat::task_has_channel_ingested_messages;

/// The single query the reply-origin check needs.
///
/// The query function itself is the seam: callers may inject a pool or a
/// transaction through the SQLx executor.
pub struct ProvenanceQueries;

/// Reports whether a completed chat task took its input from the channel,
/// so its reply (or failure notice) belongs on the external platform.
/// Direct (web/mobile) tasks can reuse a channel-bound session, but their
/// replies stay in Cordy (MUL-4988).
///
/// chat_input_task_id alone cannot discriminate: sealed channel tasks own
/// an input batch exactly like direct tasks do. The verdict is the
/// immutable channel_ingested stamp on the owned batch, keyed by the
/// batch OWNER id so an auto-retry clone (which inherits
/// chat_input_task_id while its messages stay tagged with the parent)
/// reaches the same verdict as its parent. A NULL owner is a pre-sealing
/// channel task — direct tasks have owned their batch since MUL-4351 — so
/// it keeps the deliver-by-default behavior #5645 shipped with.
///
/// `task_chat_input_task_id` is `AgentTaskQueue.chat_input_task_id`
/// (`None` represents SQL NULL).
pub async fn task_input_is_channel_ingested(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_chat_input_task_id: Option<uuid::Uuid>,
) -> anyhow::Result<bool> {
    let Some(task_id) = task_chat_input_task_id else {
        return Ok(true);
    };
    match task_has_channel_ingested_messages(executor, task_id).await? {
        // The EXISTS-shaped query treats no rows as false; the SQLx helper
        // returns an optional value, so None reads as false here too.
        Some(v) => Ok(v),
        None => Ok(false),
    }
}
