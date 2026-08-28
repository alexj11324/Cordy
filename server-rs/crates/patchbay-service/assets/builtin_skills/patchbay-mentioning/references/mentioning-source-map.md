# Mentioning — source map

The behavior contract lives in `SKILL.md`. Verify the current implementation
against these Rust sources.

- Mention parsing, comment mutation, preview, and authorization: `server-rs/crates/patchbay-handler/src/comment.rs`.
- Mention-trigger routing, suppression, and task deduplication: `server-rs/crates/patchbay-handler/src/comment_trigger.rs`.
- Squad briefing and mention context: `server-rs/crates/patchbay-handler/src/squad_briefing.rs`.
- Task claim context delivered to agents: `server-rs/crates/patchbay-handler/src/claim_comments.rs` and `claim_response.rs`.
- Agent and squad identifier resolution in the CLI: `server-rs/crates/patchbay-cli/src/issue_actor_resolver.rs`.
- Durable comment and task queries: `server-rs/crates/patchbay-db/src/queries/comment.rs`, `agent.rs`, and `squad.rs`.
- Schema history: matching comment, subscriber, agent, and squad files under `migrations/`.
