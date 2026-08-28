# Projects and resources source map

The behavior contract lives in `SKILL.md`. Verify it against these Rust
sources.

- Project and resource CLI flows: `server-rs/crates/patchbay-cli/src/project_commands.rs` and `project_resource_commands.rs`.
- Project API, validation, and resource persistence: `server-rs/crates/patchbay-handler/src/project.rs`.
- Claim-time project and repository context: `server-rs/crates/patchbay-handler/src/claim_response.rs` and `daemon.rs`.
- Local-directory and worktree execution: `server-rs/crates/patchbay-daemon/src/local_directory.rs` and `execenv/local_worktree.rs`.
- Repository checkout behavior: `server-rs/crates/patchbay-daemon/src/health.rs` and `repocache.rs`.
- Persistence queries: `server-rs/crates/patchbay-db/src/queries/project.rs` and `project_resource.rs`.
- Schema history: matching project and project-resource files under `migrations/`.
