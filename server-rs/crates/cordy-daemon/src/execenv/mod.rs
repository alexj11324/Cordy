//! Port of server/internal/daemon/execenv — module map mirrors the Go files
//! one-to-one. Lane E1 owns the foundation + codex family; the remaining
//! provider work is migrated as complete capabilities (Reasonix and QwenPaw
//! are now production paths, while Hermes/OpenClaw still have explicit
//! fail-closed stubs). Shared sidecar-manifest/runtime-skill-policy helpers
//! remain hosted inside context.rs / execenv.rs until their owning capability
//! moves them.

pub mod channel_type;
pub mod codex_home;
pub mod codex_memory;
pub mod codex_multi_agent;
pub mod codex_sandbox;
pub mod codex_shell_env;
pub mod codex_skill_strip;
pub mod codex_user_skills;
pub mod context;
pub mod cursor_mcp;
// Mirrors the Go file name execenv.go one-to-one per the module map above.
#[allow(clippy::module_inception)]
pub mod execenv;
pub mod git;
pub mod hermes;
pub mod isolation;
pub mod local_worktree;
pub mod openclaw;
pub mod reasonix;
pub mod reclaimable;
pub mod runtime_config_kind;
pub mod runtime_config;
