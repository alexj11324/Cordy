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
//!
//! All modules are ports awaiting daemon wiring (lanes A/B/D + S8); until
//! then dead_code is expected and silenced crate-wide.
#![allow(dead_code)]

pub mod execenv;

// Lane A (client/config/infra) — stubs pre-declared, launch when a slot frees.
pub mod canonical_path;
pub mod client;
pub mod config;
pub mod control_lifecycle;
pub mod diskusage;
pub mod health;
pub mod helpers;
pub mod identity;
pub mod manager;
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

// Lane D (auto_update / local MCP / local_directory) — disjoint from lanes A/B.
pub mod auto_update;
pub mod local_directory;
pub mod openclaw_runtime_config;
pub mod plugin_hook_mcp;
pub mod remote_mcp_broker;
pub mod runtime_mcp;

// Lane B (daemon.go core + surrounding surfaces) — S9-B.
pub mod agents_probe;
pub mod agents_refresh;
pub mod artifact_matcher;
pub mod claude_plugins;
pub mod local_skills;
pub mod prompt;
pub mod runtime_config_sections;
pub mod skill_cache;
pub mod slash_skill;
