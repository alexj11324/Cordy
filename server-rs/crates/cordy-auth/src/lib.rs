//! Auth primitives ported from `server/internal/auth`.
//!
//! Modules mirror the Go files one-to-one so review diffs stay aligned:
//! `jwt` (secrets + token minting), `cookie` (session/CSRF), `disabled_users`
//! (emergency denylist), plus the Redis-backed `pat_cache`,
//! `daemon_token_cache`, and `membership_cache` modules.

pub mod cloud_pat;
pub mod cookie;
pub mod daemon_token_cache;
pub mod disabled_users;
pub mod jwt;
pub mod membership_cache;
pub mod pat_cache;
