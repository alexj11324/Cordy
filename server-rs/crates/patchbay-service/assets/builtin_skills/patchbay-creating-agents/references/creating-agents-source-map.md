# Creating agents — source map

The behavior contract lives in `SKILL.md`. Verify it against these Rust
sources, using symbols rather than line numbers as anchors.

- Agent create, update, copy, environment, skills, and MCP CLI flows: `server-rs/crates/patchbay-cli/src/agent_commands.rs` and `agent_helpers.rs`.
- Agent API validation and response redaction: `server-rs/crates/patchbay-handler/src/agent_api.rs`.
- Workspace MCP assignment and merge behavior: `server-rs/crates/patchbay-handler/src/agent_mcp.rs`, `workspace_mcp.rs`, and `mcp_merge.rs`.
- Claim response assembly: `server-rs/crates/patchbay-handler/src/claim_response.rs` and `daemon.rs`.
- Runtime model discovery and execution-time validation: `server-rs/crates/patchbay-daemon/src/agents_probe.rs`, `provider_adapter.rs`, and `runtime_probe.rs`.
- Built-in and workspace skill loading: `server-rs/crates/patchbay-service/src/builtin_skills.rs` and `skill_bundle.rs`.
- Persistence queries: `server-rs/crates/patchbay-db/src/queries/agent.rs`, `skill.rs`, and `workspace_mcp.rs`.
- Schema history: matching agent, skill, runtime, and workspace-MCP files under `migrations/`.
