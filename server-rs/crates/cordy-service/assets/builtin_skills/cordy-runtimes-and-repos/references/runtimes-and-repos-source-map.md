# Runtimes and repos source map

The behavior contract lives in `SKILL.md`. Verify it against these Rust
sources.

- Runtime, profile, update, delete, and repository CLI flows: `server-rs/crates/cordy-cli/src/runtime_commands.rs`, `runtime_profile.rs`, `runtime_update.rs`, `runtime_delete.rs`, and `repo_commands.rs`.
- Runtime API, ownership checks, and update requests: `server-rs/crates/cordy-handler/src/runtime.rs`, `runtime_profile.rs`, and `runtime_requests.rs`.
- Task claim and daemon registration endpoints: `server-rs/crates/cordy-handler/src/daemon.rs` and `claim_response.rs`.
- Checkout registry, health endpoint, and repository isolation: `server-rs/crates/cordy-daemon/src/daemon_core.rs`, `health.rs`, and `repocache.rs`.
- Task configuration and worktree setup: `server-rs/crates/cordy-daemon/src/execenv/execenv.rs`, `execenv/local_worktree.rs`, and `runtime_config_sections.rs`.
- Persistence queries: `server-rs/crates/cordy-db/src/queries/runtime.rs`, `runtime_profile.rs`, and `project_resource.rs`.
- Schema history: matching runtime, runtime-profile, and project-resource files under `migrations/`.
