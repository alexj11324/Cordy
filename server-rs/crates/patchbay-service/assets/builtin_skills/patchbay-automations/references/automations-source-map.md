# Automations source map

The behavior contract lives in `SKILL.md`. Use these Rust sources to verify
the current implementation; function names are more stable anchors than line
numbers.

- CLI commands and output: `server-rs/crates/patchbay-cli/src/automation_commands.rs`, `automation_output.rs`, and `automation_resolver.rs`.
- Authenticated API and webhook ingress: `server-rs/crates/patchbay-handler/src/automation.rs` and `automation_webhook.rs`.
- Durable webhook delivery recovery: `server-rs/crates/patchbay-handler/src/webhook_delivery_worker.rs`.
- Dispatch, admission, readiness, and execution-mode behavior: `server-rs/crates/patchbay-service/src/automation.rs` and `agent_ready.rs`.
- Persistence queries: `server-rs/crates/patchbay-db/src/queries/automation.rs` and `webhook_delivery.rs`.
- Schema history: matching automation and webhook-delivery files under `migrations/`.
