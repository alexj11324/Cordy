# Working on issues source map

The behavior contract lives in `SKILL.md`. Verify it against these Rust
sources, using symbols rather than line numbers as anchors.

- Issue create, update, assignment, status, children, metadata, property, and pull-request CLI flows: the `issue_*_commands.rs` modules and `property_commands.rs` under `server-rs/crates/patchbay-cli/src/`.
- Issue API, status transitions, hierarchy, and assignment: `server-rs/crates/patchbay-handler/src/issue.rs` and `issue_status.rs`.
- Pull-request attachment and webhook linkage: `server-rs/crates/patchbay-handler/src/issue_pull_request.rs`, `github.rs`, and `vcs_webhook.rs`.
- Custom properties and metadata validation: `server-rs/crates/patchbay-handler/src/property.rs` and `issue_property_value.rs`.
- Task admission, completion, and failure recovery: `server-rs/crates/patchbay-handler/src/task.rs` and `server-rs/crates/patchbay-service/src/task_service.rs`.
- Persistence queries: `server-rs/crates/patchbay-db/src/queries/issue.rs`, `issue_property.rs`, `github.rs`, and `task_message.rs`.
- Schema history: matching issue, property, GitHub pull-request, and task files under `migrations/`.
