//! Port of server/internal/daemon/execenv — module map mirrors the Go files
//! one-to-one. Lane E1 owns the foundation + codex family; the provider
//! families (hermes/openclaw/reasonix/qwenpaw) land later in lane E2 and the
//! sidecar-manifest / runtime-skill-policy helpers they share are temporarily
//! hosted inside context.rs / execenv.rs behind `// S9-integration:` markers.

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
pub mod execenv;
pub mod git;
pub mod isolation;
pub mod local_worktree;
pub mod reclaimable;
pub mod runtime_config_kind;
