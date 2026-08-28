//! Platform-agnostic foundation for Patchbay's inbound IM integrations
//! (Feishu/Lark, Slack, WeCom, …).
//!
//! Shared channel contract (PB-3506 / PB-3515).
//! This crate owns the contract every integration implements so the core
//! never learns what a given platform's event JSON looks like.
//!
//! The contract has four pieces:
//!
//! 1. [`Channel`] — the per-integration async trait
//!    (`r#type`/connect/disconnect/send/capabilities). An adapter
//!    translates platform payloads in both directions and owns its own
//!    connection mode (outbound WebSocket long-conn, inbound HTTP, …);
//!    the core only calls these methods.
//! 2. [`message::InboundMessage`] / [`message::OutboundMessage`] — the
//!    normalized message envelopes. Every platform's inbound payload is
//!    translated by its adapter into one `InboundMessage`; the core
//!    routes, dedups, and persists only this struct.
//! 3. [`capability::Capability`] — a bitmask each Channel uses to DECLARE
//!    what it can do. This crate only models the declaration; it
//!    intentionally contains no degrade logic.
//! 4. [`registry::Registry`] — a `Type`→`Factory` map with
//!    last-writer-wins semantics. Adding a platform is "register a
//!    factory", not "edit the core".
//!
//! Boundary rule (PB-3515 decision §2): the envelope holds ONLY fields
//! that are true across every platform. Anything platform-specific lives
//! in [`message::InboundMessage::raw`] and is read ONLY by the adapter
//! that produced it. The core never reads `raw`.
//!
//! This crate is pure: it has no database, network, or platform
//! dependencies, and nothing in it depends on another integration crate.

pub mod capability;
pub mod channel;
pub mod generation;
pub mod handler;
pub mod history;
pub mod member_text;
pub mod message;
pub mod registry;
pub mod runtime_tasks;

pub use capability::Capability;
pub use channel::{BuiltChannel, Channel, Config, Factory, FactoryFuture, Type};
pub use generation::{GenerationExpired, GenerationHandle, GenerationRegistry, LeaseGeneration};
pub use handler::{HandlerFuture, InboundHandler};
pub use history::{HistoryMessage, HistoryOptions, HistoryPage, HistoryRole};
pub use member_text::break_markdown_link_adjacency;
pub use message::{
    ChatType, InboundMessage, MediaRef, MsgType, OutboundMessage, ReplyCtx, SendResult, Source,
};
pub use registry::{Registry, UnknownTypeError};
pub use runtime_tasks::{shutdown_join_handles, RuntimeTasks};
