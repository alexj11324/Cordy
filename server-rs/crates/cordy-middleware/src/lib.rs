//! HTTP middleware — port of `server/internal/middleware`.
//!
//! Modules mirror the Go files one-to-one: `auth` (JWT/PAT/task-token/cloud
//! authentication), with workspace guards and daemon/plugin auth landing as
//! the port progresses.

pub mod auth;
pub mod client;
pub mod csp;
pub mod daemon_auth;
pub mod plugin_auth;
pub mod ratelimit;
pub mod request_id;
pub mod request_logger;
pub mod workspace;
