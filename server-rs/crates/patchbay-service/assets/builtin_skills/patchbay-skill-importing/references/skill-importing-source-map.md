# Skill-importing source map

The behavior contract lives in `SKILL.md`. Verify it against these Rust
sources.

- Skill import, refresh, conflict handling, and output CLI flows: `server-rs/crates/patchbay-cli/src/skill_commands.rs` and `skill_command_schema.rs`.
- Import source detection, archive validation, fetch limits, and conflict policy: `server-rs/crates/patchbay-handler/src/skill_import.rs`.
- Skill CRUD, binding, refresh, and reserved-content behavior: `server-rs/crates/patchbay-handler/src/skill.rs`.
- Runtime skill assembly and cache behavior: `server-rs/crates/patchbay-service/src/skill_bundle.rs`, `server-rs/crates/patchbay-daemon/src/local_skills.rs`, and `skill_cache.rs`.
- Persistence queries: `server-rs/crates/patchbay-db/src/queries/skill.rs`.
- Schema history: matching skill and agent-skill files under `migrations/`.
