//! Daemon manager (port of server/internal/daemon + internal/daemonws).
//!
//! Module map mirrors the Go package layout one-to-one; each module header
//! records the Go source file it ports. S9 lanes own disjoint files — see
//! tasks/go-to-rust-migration.md for the lane split.
//!
//! Slices:
//! - W  (daemonws): hub.rs, notifier.rs
//! - E1 (execenv foundation + codex family): execenv/{execenv,context,
//!   channel_type,runtime_config_kind,reclaimable,isolation,git,
//!   local_worktree,codex_home,codex_sandbox,codex_memory,codex_shell_env,
//!   codex_multi_agent,codex_user_skills,codex_skill_strip,cursor_mcp}
//! - R  (repo lifecycle): repocache.rs, gc.rs

pub mod execenv;

// Lane A (client/config/infra) — stubs pre-declared, launch when a slot frees.
pub mod canonical_path;
pub mod client;
pub mod config;
pub mod diskusage;
pub mod health;
pub mod helpers;
pub mod identity;
pub mod poisoned;
pub mod reconcile;
pub mod thread_name;
pub mod types;
pub mod wakeup;
pub mod wsrpc;

pub mod gc;
pub mod hub;
pub mod notifier;
pub mod repocache;
