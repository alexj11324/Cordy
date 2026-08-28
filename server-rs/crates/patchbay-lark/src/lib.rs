//! Feishu/Lark adapter: WS long-conn, card builder, docx content, resolvers
//!
//! Production modules cover the complete integration lifecycle.

pub mod audit;
pub mod backfill;
pub mod binding_token;
pub mod channel;
pub mod channel_store;
pub mod chat;
pub mod client;
pub mod connector;
pub mod content_flatten;
pub mod feishu_types;
pub mod frame_decoder;
pub mod http_client;
pub mod inbound_enricher;
pub mod installation;
pub mod markdown_detect;
pub mod media_ingest;
pub mod outbound;
pub mod outcome_replier;
pub mod params;
pub mod registration;
pub mod resolvers;
pub mod store;
pub mod types;
pub mod typing_indicator;
pub mod ws_chunk_assembler;
pub mod ws_connector;
pub mod ws_endpoint;
pub mod ws_frame;
