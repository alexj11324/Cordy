//! Daemon manager, execution environment, and WebSocket control plane.
//!
//! The production daemon stack is wired. Any intentionally dormant
//! compatibility seam must carry a narrow module/item-level allowance with a
//! local rationale; the crate must not hide unwired code globally.

pub mod execenv;

// Lane A (client/config/infra) — stubs pre-declared, launch when a slot frees.
pub mod assembly;
pub mod auth_lifecycle;
pub mod bootstrap;
pub mod canonical_path;
pub mod client;
pub mod config;
pub mod control_client;
pub mod control_lifecycle;
pub mod daemon_core;
pub mod diskusage;
pub mod execution_plan;
pub mod health;
pub mod helpers;
pub mod identity;
pub mod lifecycle;
pub mod manager;
pub mod poisoned;
pub mod process_control;
pub mod production_services;
pub mod production_stack;
pub mod provider_adapter;
mod provider_isolation;
pub mod provider_registration;
pub mod reconcile;
pub mod registration;
pub mod repo_state;
pub mod runtime_registry;
pub mod runtime_set;
pub mod task_execution;
pub mod thread_name;
pub mod types;
pub mod update_executor;
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
pub mod activity;
pub mod agents_probe;
pub mod agents_refresh;
pub mod artifact_matcher;
pub mod claude_plugins;
pub mod local_skills;
pub mod prompt;
pub mod runtime_config_sections;
pub mod runtime_probe;
pub mod skill_cache;
pub mod slash_skill;
