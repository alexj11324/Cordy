# Squad source map

The behavior contract lives in `SKILL.md`. Verify the current implementation
against these Rust sources.

- Squad command schema, CRUD, and membership CLI flows: `server-rs/crates/patchbay-cli/src/squad_command_schema.rs` and `squad_commands.rs`.
- Squad API, validation, membership, and leader rules: `server-rs/crates/patchbay-handler/src/squad.rs`.
- Leader briefing assembly: `server-rs/crates/patchbay-handler/src/squad_briefing.rs`.
- Issue assignment, comment mentions, and child-done routing: `server-rs/crates/patchbay-handler/src/issue.rs`, `comment_trigger.rs`, and `task.rs`.
- Squad autopilot leader resolution: `server-rs/crates/patchbay-service/src/autopilot.rs`.
- Persistence queries: `server-rs/crates/patchbay-db/src/queries/squad.rs`, `issue.rs`, and `agent.rs`.
- Schema history: matching squad, issue, and assignment files under `migrations/`.
