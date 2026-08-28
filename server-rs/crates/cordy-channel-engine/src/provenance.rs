//! Reply-origin check: did a completed chat task take its input from an
//! external channel?
//!

use cordy_db::queries::chat::task_has_channel_ingested_messages;

/// The single query the reply-origin check needs.
///
/// Port note: Go defines a one-method interface satisfied by `*db.Queries`
/// and takes it as a parameter; here the query function itself is the
/// seam (callers inject any executor — pool or transaction), so the trait
/// collapses into the direct call.
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
/// (`None` = Go's NULL pgtype).
pub async fn task_input_is_channel_ingested(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_chat_input_task_id: Option<uuid::Uuid>,
) -> anyhow::Result<bool> {
    let Some(task_id) = task_chat_input_task_id else {
        return Ok(true);
    };
    match task_has_channel_ingested_messages(executor, task_id).await? {
        // Go's sqlc-generated bool query treats no-rows as false via the
        // EXISTS wrapper; the Rust generator returns Option<Option<bool>>
        // shaped helpers, so None reads as false here too.
        Some(v) => Ok(v),
        None => Ok(false),
    }
}
