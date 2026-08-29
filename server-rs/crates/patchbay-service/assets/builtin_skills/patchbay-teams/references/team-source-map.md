# Team source map

The behavior contract lives in `SKILL.md`. Verify the current implementation
against these Rust sources.

- Team command schema, CRUD, and membership CLI flows: `server-rs/crates/patchbay-cli/src/team_command_schema.rs` and `team_commands.rs`.
- Team API, validation, membership, and leader rules: `server-rs/crates/patchbay-handler/src/team.rs`.
- Leader briefing assembly: `server-rs/crates/patchbay-handler/src/team_briefing.rs`.
- Issue assignment, comment mentions, and child-done routing: `server-rs/crates/patchbay-handler/src/issue.rs`, `comment_trigger.rs`, and `task.rs`.
- Team autopilot leader resolution: `server-rs/crates/patchbay-service/src/autopilot.rs`.
- Persistence queries: `server-rs/crates/patchbay-db/src/queries/team.rs`, `issue.rs`, and `agent.rs`.
- Schema history: matching team, issue, and assignment files under `migrations/`.
