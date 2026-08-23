//! Telegram adapter (port of server/internal/integrations/telegram):
//! Bot API long-poll link, markdown→HTML conversion, inbound
//! normalization, sender chunking, resolver set, install service.

pub mod api;
pub mod channel;
pub mod config;
pub mod inbound;
pub mod markdown;
pub mod media;
pub mod outbound;
pub mod replier;
pub mod resolvers;
pub mod sender;

pub use api::{
    ApiError, BotApi, ConflictError, DocumentRef, EditMessageTextParams, MessageEntity, PhotoSize,
    ReplyParameters, SendMessageParams, TelegramChat, TelegramFile, TelegramMessage, TelegramUser,
    Update, VideoRef, VoiceRef, WebhookInfo, DEFAULT_API_BASE,
};
pub use config::{
    decode_credentials, decode_public_config, invalid_bot_token, parse_bot_id, Credentials,
    Decrypter, DecrypterFn, PublicConfig, TYPE_TELEGRAM,
};
pub use inbound::{
    inbound_from_update, message_key, telegram_chat_type, TelegramMediaRef, TelegramRawEvent,
};
pub use markdown::{escape_html, format_html};
pub use outbound::{
    chat_done_content, event_task_id, is_not_modified, outbound_target, stream_partial,
    task_failure_retry_pending, terminal_step, BotFallbackBackoff, ChatSchedule,
    ChatScheduleRegistry, PartialAction, StreamState, TerminalReply, TerminalStep, EDIT_INTERVAL,
    MAX_CHAT_SCHEDULES, STREAM_PLACEHOLDER, TASK_FAILED_TEXT,
};
pub use sender::{chunk_message, parse_message_ref, utf16_units, MAX_MESSAGE_UNITS};
