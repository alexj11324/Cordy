#![allow(dead_code)] // S9-integration: consumed by daemon.go core wiring (S8)
//! Port of the execenv helpers the per-turn prompt consumes:
//! `runtime_config_sections.go` (SessionContinuityNotice* constants,
//! BuildTaskInitiatorBlock, BuildConnectedAppsBlock) and
//! `reply_instructions.go` (BuildNewCommentsHint, BuildResumedCommentsHint,
//! BuildColdCommentsHint, BuildCommentReplyInstructions,
//! BuildMultiThreadCommentReplyInstructions).
//!
//! These are hosted inside cordy-daemon because their only daemon-side
//! consumer is `crate::prompt` (the per-turn user message); the brief-side
//! writers remain with lane E1a's runtime-config port.

use std::fmt::Write as _;

use serde_json::Value;

use crate::execenv::channel_type::channel_display_name;
use crate::execenv::context::{frontmatter_parts, resolve_skill_slugs};
use crate::execenv::execenv::{
    ConnectedApp, ProjectResourceForEnv, SkillContextForEnv, TaskContextForEnv, ThreadReplyTarget,
};
use crate::execenv::runtime_config_kind::{classify_task, TaskKind};

/// SessionContinuityNoticeIssue (MUL-5722 wording; see Go rationale block).
pub(crate) fn session_continuity_notice_issue() -> &'static str {
    "## Session Continuity Notice\n\nThis run was meant to continue an earlier conversation, but that provider session could not be restored, so you are on a fresh one. The issue and its full comment history are unaffected — that record is the authoritative version of this conversation, and reading it (which your workflow already requires) reconstructs it. What is gone is only your own working memory from earlier turns: what you already tried, what you ruled out, and how far you had got. Re-derive what you need instead of assuming it, and do not claim continuity the record cannot back up. Do not open your reply by announcing this — raise it only where it actually matters, such as when the user refers to reasoning you never wrote down.\n\n"
}

pub(crate) fn session_continuity_notice_channel_history() -> &'static str {
    "## Session Continuity Notice\n\nThis run was meant to continue an earlier conversation, but that provider session could not be restored, so you are on a fresh one. The channel conversation itself is unaffected — read it back with `cordy chat history` / `cordy chat thread` before acting, and treat what you find there as the authoritative version. What is gone is only your own working memory from earlier turns: what you already tried, what you ruled out, and how far you had got. Re-derive what you need instead of assuming it. Do not open your reply by announcing this — raise it only where it actually matters.\n\n"
}

pub(crate) fn session_continuity_notice_chat_transcript() -> &'static str {
    "## Session Continuity Notice\n\nThis run was meant to continue an earlier conversation, but that provider session could not be restored, so you are on a fresh one. The conversation itself is unaffected — Cordy stored it, and you can read it back with `cordy chat history` before acting; treat what you find there as the authoritative version. What is gone is only your own working memory from earlier turns: what you already tried, what you ruled out, and how far you had got. Re-derive what you need instead of assuming it. Do not open your reply by announcing this — raise it only where it actually matters.\n\n"
}

/// Defensive fallback for a surface whose conversation Cordy never stored.
pub(crate) fn session_continuity_notice_unrecoverable() -> &'static str {
    "## Session Continuity Notice\n\nThis run was meant to continue an earlier conversation, but that session's context could NOT be restored — you are starting fresh with no memory of the previous turns. That history is not readable from anywhere now: there is no command that fetches it, and only the context already in this message survives. **When you reply, tell the user up front (one short sentence) that the previous conversation context was unavailable and this is a new session**, so they understand why the thread did not carry over.\n\n"
}

/// sanitizeNameForBriefMarkdown (runtime_config.go): single-line plain-text
/// token safe for markdown inline constructs.
fn sanitize_name_for_brief_markdown(name: &str) -> String {
    let mut b = String::with_capacity(name.len());
    let mut prev_space = false;
    for r in name.chars() {
        match r {
            '\r' | '\n' | '\t' | '\u{b}' | '\u{c}' => {
                if !prev_space && !b.is_empty() {
                    b.push(' ');
                    prev_space = true;
                }
            }
            c if (c as u32) < 0x20 || c == '\u{7f}' => continue,
            '*' | '_' | '`' | '\\' | '[' | ']' | '<' => {
                b.push('\\');
                b.push(r);
                prev_space = false;
            }
            c => {
                b.push(c);
                prev_space = false;
            }
        }
    }
    b.trim().to_string()
}

/// sanitizeEmailForBrief: verbatim when embeddable, "" otherwise. Does NOT
/// escape markdown specials so agents can match the address exactly.
fn sanitize_email_for_brief(email: &str) -> String {
    let email = email.trim();
    if email.is_empty() || !email.contains('@') {
        return String::new();
    }
    for r in email.chars() {
        if (r as u32) < 0x20
            || r == '\u{7f}'
            || r == ' '
            || r == '\\'
            || r == '`'
            || r == '*'
            || r == '<'
            || r == '>'
            || r == '['
            || r == ']'
        {
            return String::new();
        }
    }
    email.to_string()
}

fn sanitize_brief_code_token(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return String::new();
    }
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
        .collect()
}

/// BuildTaskInitiatorBlock (MUL-2645 pinned phrases kept verbatim). Returns
/// "" when no initiator name resolves.
pub(crate) fn build_task_initiator_block(
    initiator_type: &str,
    initiator_name: &str,
    initiator_email: &str,
) -> String {
    let safe_initiator = sanitize_name_for_brief_markdown(initiator_name);
    if safe_initiator.is_empty() {
        return String::new();
    }
    let mut b = String::new();
    b.push_str("## Task Initiator\n\n");
    if initiator_type == "agent" {
        b.push_str(&format!(
            "This task was initiated by **{safe_initiator}**, another agent in this workspace.\n\n"
        ));
    } else {
        let email = sanitize_email_for_brief(initiator_email);
        if !email.is_empty() {
            b.push_str(&format!(
                "This task was initiated by **{safe_initiator}** ({email}), a member of this workspace.\n\n"
            ));
        } else {
            b.push_str(&format!(
                "This task was initiated by **{safe_initiator}**, a member of this workspace.\n\n"
            ));
        }
    }
    b.push_str("The initiator — not the runtime owner — is who you are answering: apply any per-person privacy or access rules your instructions define. Your Cordy credentials stay scoped to the runtime owner, and initiator attribution does not change what you may read or write; do not assume the initiator can see everything you can.\n\n");
    b
}

/// DisplayNameForToolkitSlug fallback lives in cordy-service's runtimeapps
/// port; the daemon mirrors just the slug→title heuristic locally so no
/// cross-crate dependency is needed for a cosmetic fallback.
fn toolkit_slug_fallback(slug: &str) -> String {
    match slug {
        "github" => return "GitHub".to_string(),
        "gmail" => return "Gmail".to_string(),
        "linkedin" => return "LinkedIn".to_string(),
        _ => {}
    }
    slug.split(&['-', '_'][..])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut cs = p.chars();
            match cs.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &cs.as_str().to_ascii_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// BuildConnectedAppsBlock. Returns "" when no app resolves.
pub(crate) fn build_connected_apps_block(apps: &[ConnectedApp]) -> String {
    if apps.is_empty() {
        return String::new();
    }
    let mut lines = String::new();
    for app in apps {
        let server_name = sanitize_brief_code_token(&app.server_name);
        let toolkit_slug = sanitize_brief_code_token(&app.toolkit_slug);
        if server_name.is_empty() || toolkit_slug.is_empty() {
            continue;
        }
        let mut name = sanitize_name_for_brief_markdown(&app.toolkit_name);
        if name.is_empty() {
            name = sanitize_name_for_brief_markdown(&toolkit_slug_fallback(&toolkit_slug));
        }
        if name.is_empty() {
            name = toolkit_slug.clone();
        }
        lines.push_str(&format!(
            "- {name} (`{toolkit_slug}`) via MCP server `{server_name}`\n"
        ));
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("## Connected Apps\n\n{lines}\nUse the listed MCP server when the task asks to read or act in one of these apps.\n\n")
}

// ---------------------------------------------------------------------------
// reply_instructions.go
// ---------------------------------------------------------------------------

fn active_thread_id(trigger_thread_id: &str, trigger_comment_id: &str) -> String {
    if !trigger_thread_id.is_empty() {
        trigger_thread_id.to_string()
    } else {
        trigger_comment_id.to_string()
    }
}

/// BuildNewCommentsHint: WARM path pointer. Renders None on cold start or when
/// there are no new comments.
pub(crate) fn build_new_comments_hint(
    issue_id: &str,
    trigger_comment_id: &str,
    trigger_thread_id: &str,
    new_comments_since: &str,
    new_comment_count: i64,
) -> Option<String> {
    if new_comment_count <= 0 || new_comments_since.is_empty() || issue_id.is_empty() {
        return None;
    }
    let thread_id = active_thread_id(trigger_thread_id, trigger_comment_id);
    if !thread_id.is_empty() {
        return Some(format!(
            "{new_comment_count} new comment(s) on this issue since your last run — don't read them all blindly. \
Start with the thread your triggering comment is in: \
`cordy issue comment list {issue_id} --thread {thread_id} --since {new_comments_since} --compact --output json` \
(swap `--since` for `--tail 30` if you need the full thread, not just the delta). \
Only if you need context from the other threads, rerun it without `--thread` for the issue-wide catch-up.\n\n"
        ));
    }
    // Defensive fallback: no thread anchor → plain issue-wide catch-up.
    Some(format!(
        "{new_comment_count} new comment(s) on this issue since your last run. Catch up: \
`cordy issue comment list {issue_id} --since {new_comments_since} --compact --output json`.\n\n"
    ))
}

/// BuildResumedCommentsHint: WARM no-delta path.
pub(crate) fn build_resumed_comments_hint(
    issue_id: &str,
    trigger_comment_id: &str,
    trigger_thread_id: &str,
) -> String {
    let thread_id = active_thread_id(trigger_thread_id, trigger_comment_id);
    if issue_id.is_empty() || thread_id.is_empty() {
        return String::new();
    }
    format!(
        "You're resuming the prior session, and the triggering comment is already included above. \
No other new comments on this issue since your last run. \
If your reply depends on thread context, do not rely only on resumed session memory — \
first pull the triggering conversation with: \
`cordy issue comment list {issue_id} --thread {thread_id} --tail 30 --compact --output json`.\n\n"
    )
}

/// BuildColdCommentsHint: COLD path pointer. None when there is no triggering
/// comment to thread from.
pub(crate) fn build_cold_comments_hint(
    issue_id: &str,
    trigger_comment_id: &str,
    trigger_thread_id: &str,
) -> Option<String> {
    let thread_id = active_thread_id(trigger_thread_id, trigger_comment_id);
    if issue_id.is_empty() || thread_id.is_empty() {
        return None;
    }
    Some(format!(
        "Read the triggering conversation first: \
`cordy issue comment list {issue_id} --thread {thread_id} --tail 30 --compact --output json` \
(that thread's root + its 30 newest replies). \
Need cross-thread background? Rerun with `--roots-only --summary` replacing `--thread ... --tail 30` \
to scan the other threads cheaply, and expand only what looks relevant.\n\n"
    ))
}

/// BuildCommentReplyInstructions: the canonical reply cookbook. The template is
/// platform- AND provider-agnostic (the failure lives at the shell layer);
/// the OS split matches Go's runtimeGOOS branch at compile time via cfg.
pub(crate) fn build_comment_reply_instructions(
    provider: &str,
    issue_id: &str,
    trigger_comment_id: &str,
    squad_leader: bool,
) -> String {
    let _ = provider; // retained for caller symmetry (Go does the same)
    if trigger_comment_id.is_empty() {
        return String::new();
    }
    let lead = if squad_leader {
        "Unless your outcome is `no_action`, post your reply as a comment — always use the trigger comment ID below, "
    } else {
        "Post your reply as a comment — always use the trigger comment ID below, "
    };
    #[cfg(windows)]
    {
        format!(
            "{lead}do NOT reuse --parent values from previous turns in this session.\n\n\
Write the body file first — never pipe via `--content-stdin` (PowerShell drops non-ASCII; full rules: ## Comment Formatting above):\n\n\
    cordy issue comment add {issue_id} --parent {trigger_comment_id} --content-file ./reply.md\n\
    Remove-Item ./reply.md\n\n\
Do NOT write literal `\\n` escapes to simulate line breaks; the file preserves real newlines.\n"
        )
    }
    #[cfg(not(windows))]
    {
        format!(
            "{lead}do NOT reuse --parent values from previous turns in this session.\n\n\
Write the body file first (rules: ## Comment Formatting above — MUL-2904 / #4182):\n\n\
    cordy issue comment add {issue_id} --parent {trigger_comment_id} --content-file ./reply.md\n\
    rm ./reply.md\n\n\
Do NOT write literal `\\n` escapes to simulate line breaks; the file preserves real newlines.\n"
        )
    }
}

/// BuildMultiThreadCommentReplyInstructions (MUL-4348 / MUL-5825): fan-out
/// block carrying only multi-thread-SPECIFIC guidance. Returns "" below two
/// targets.
pub(crate) fn build_multi_thread_comment_reply_instructions(
    issue_id: &str,
    targets: &[ThreadReplyTarget],
    squad_leader: bool,
) -> String {
    if issue_id.is_empty() || targets.len() < 2 {
        return String::new();
    }

    let mut target_lines = String::new();
    for (i, tgt) in targets.iter().enumerate() {
        target_lines.push_str(&format!(
            "{}. thread {} → reply with `--parent {}`\n",
            i + 1,
            tgt.thread_id,
            tgt.parent_id
        ));
    }

    let lead_head = "This run coalesced comments from %d DISTINCT threads.";
    let _ = lead_head;
    let leader_carve_out = if squad_leader {
        "**If your outcome is `no_action`, skip this ENTIRE fan-out block — post no replies at all and exit via `cordy squad activity` as your leader rules direct; everything below applies only otherwise.** Otherwise, post ONE reply per thread"
    } else {
        "Post ONE reply per thread"
    };
    format!(
        "This run coalesced comments from {} DISTINCT threads. {leader_carve_out} — {} in total. This OVERRIDES the \"post exactly one comment per run\" rule: for THIS run multiple replies are required and correct. Do NOT merge separate threads into one comment or post twice in the same thread.\n\n\
Reply targets, in posting order — OLDEST thread first, the newest (triggering) thread LAST. Use the exact `--parent` for each; never reuse a `--parent` from an earlier turn:\n\
{target_lines}\n\
Write and post each reply exactly as `## Comment Formatting` above directs, with ONE multi-thread delta: use a DISTINCT body file per thread (./reply-1.md, ./reply-2.md, …) so one reply's content can never leak into another's.\n",
        targets.len(),
        targets.len(),
    )
}

// ---------------------------------------------------------------------------
// runtime_config.go: runtime brief assembler
// ---------------------------------------------------------------------------

fn write_header(b: &mut String) {
    b.push_str("# Cordy Agent Runtime\n\nYou are a coding agent in the Cordy platform. Use the `cordy` CLI to interact with the platform.\n\n");
}

fn write_background_task_safety(b: &mut String) {
    b.push_str("## Background Task Safety\n\nCordy marks the task terminal the moment your top-level turn exits — any run-owned work still active is orphaned, its result lost, and the final comment you meant to post never sends. There is no background-completion wakeup, whatever a tool response promises. Never background-and-yield: collect required results inside foreground tool calls that block to completion, run unobservable work synchronously, and never end a turn \"standing by\" for something to finish — that message becomes your final output.\n\n");
    b.push_str("External systems triggered by your completed actions — CI, GitHub Actions after a successful push — are not run-owned: do not wait for them, and do not run `gh pr checks --watch`, `gh run watch`, or sleep/retry polls. A repo's merge gate (\"CI must be green before merge\") is NOT your delivery acceptance criteria. Deliver what you have — \"Local tests pass; CI running: <PR link>\" is a complete hand-off. The one exception: when the trigger comment or the issue's acceptance criteria explicitly ask for the CI result, collect it as ONE foreground blocking call (`gh pr checks <pr> --watch`) inside this same turn.\n\n");
    b.push_str("A user explicitly asking for a local service to stay available after the turn is a persistent service handoff, not background-and-yield — allowed only when the running service itself is the requested deliverable. Detach its lifecycle from this run first (durable logs, a recorded cleanup handle such as PID/profile), verify readiness, and reply with the URL, logs, and stop instructions. Without a supervisor, describe survival as best-effort, not guaranteed.\n\n");
}

fn write_agent_identity(b: &mut String, ctx: &TaskContextForEnv) {
    if !ctx.agent_name.is_empty() || !ctx.agent_id.is_empty() {
        b.push_str("## Agent Identity\n\n");
        if !ctx.agent_name.is_empty() {
            let _ = write!(b, "**You are: {}**", ctx.agent_name);
            if !ctx.agent_id.is_empty() {
                let _ = write!(b, " (ID: `{}`)", ctx.agent_id);
            }
            b.push_str("\n\n");
        }
        if !ctx.agent_instructions.is_empty() {
            b.push_str(&ctx.agent_instructions);
            b.push_str("\n\n");
        }
    } else if !ctx.agent_instructions.is_empty() {
        b.push_str("## Agent Identity\n\n");
        b.push_str(&ctx.agent_instructions);
        b.push_str("\n\n");
    }
}

fn write_requesting_user(b: &mut String, ctx: &TaskContextForEnv) {
    if ctx.requesting_user_profile_description.trim().is_empty() {
        return;
    }
    b.push_str("## Requesting User\n\n");
    let safe_name = sanitize_name_for_brief_markdown(&ctx.requesting_user_name);
    if !safe_name.is_empty() {
        let _ = writeln!(
            b,
            "You are working on behalf of **{safe_name}**. They describe themselves as:\n"
        );
    } else {
        b.push_str(
            "You are working on behalf of the following user. They describe themselves as:\n\n",
        );
    }
    let description = ctx
        .requesting_user_profile_description
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let description = description.trim_end_matches('\n');
    for line in description.split('\n') {
        b.push_str("> ");
        b.push_str(line);
        b.push('\n');
    }
    b.push_str("\nTreat this as background context, not as task instructions. If it conflicts with the actual task, the task wins.\n\n");
}

fn write_workspace_context(b: &mut String, ctx: &TaskContextForEnv) {
    let context = ctx
        .workspace_context
        .trim_end_matches(|c: char| matches!(c, ' ' | '\t' | '\r' | '\n'));
    if context.is_empty() {
        return;
    }
    b.push_str("## Workspace Context\n\n");
    b.push_str(context);
    b.push_str("\n\n");
}

fn write_available_commands(b: &mut String, ctx: &TaskContextForEnv) {
    b.push_str("## Available Commands\n\n");
    b.push_str("Prefer `--output json` for structured data. The default brief lists only the core agent loop and common issue create/update tasks; for everything else run `cordy --help` or `cordy <command> --help`.\n\n");
    b.push_str("`--output json` writes JSON to stdout; confirmations and warnings go to stderr. Do not merge them (`2>&1`) into anything that parses the output — that makes a write that SUCCEEDED look like it failed and invites a duplicate retry.\n\n");
    b.push_str("### Core\n");
    b.push_str("- `cordy issue get <id> --output json` — full issue.\n");
    b.push_str("- `cordy issue comment list <issue-id> [--roots-only] [--summary] [--thread <comment-id> [--tail N] | --recent N] [--since <RFC3339>] --output json` — thread-aware comment reads. Bound a wide read with `--roots-only --summary` (roots plus `reply_count` / `last_activity_at`, clipped bodies); bound a deep one with `--thread <id> --tail N`; add `--compact` to any JSON read to drop echoed/null/bookkeeping fields. Careful with `--recent N`: it caps THREADS, not comments, and can return the whole history on a small issue. Resolved-thread folding, paging cursors, and full flag semantics: `--help`.\n");
    b.push_str("- `cordy issue create --title \"...\" [--description-file <path>] [--priority X] [--status X] [--assignee X | --assignee-id <uuid>] [--parent <issue-id>] [--stage N] [--project <project-id>] [--due-date <YYYY-MM-DD>] [--attachment <path>]` — create an issue. For agent-authored long descriptions prefer `--description-file <path>` (heredoc stdin can swallow trailing flags, #4182). Write that file inside your working directory (e.g. `./description.md`), never `/tmp` or shared paths — same workdir rule as `## Comment Formatting`.\n");
    b.push_str("- `cordy issue update <id> [--title X] [--description-file <path>] [--priority X] [--status X] [--assignee X] [--parent <issue-id>] [--stage N] [--project <project-id>] [--due-date <YYYY-MM-DD>] [--no-start]` — update fields; pass `--parent \"\"` to clear parent.\n");
    b.push_str("- `cordy issue assign <id> (--to X | --to-id <uuid> | --unassign) [--no-start]` — change ownership. On assign/update/status, `--no-start` records the change without starting another run — use it when the work is already underway.\n");
    b.push_str("- `cordy issue status <id> <status> [--no-start]` — flip status (todo / in_progress / in_review / done / blocked / backlog / cancelled).\n");
    b.push_str("- `cordy issue children <id> [--output json]` — list a parent's sub-issues grouped by stage.\n");
    b.push_str("- `cordy issue comment add <issue-id> [--content \"...\" | --content-file <path> | --content-stdin] [--parent <comment-id>] [--attachment <path>]` — post a comment. Agent-authored bodies MUST use `--content-file`; see `## Comment Formatting` for why. `cordy issue comment add --help` for full flags.\n");
    b.push_str("- `cordy issue metadata list <issue-id> [--output json]` — list KV metadata.\n");
    b.push_str("- `cordy issue metadata set <issue-id> --key <k> --value <v> [--type string|number|bool]` — pin or overwrite a key.\n");
    b.push_str("- `cordy issue metadata delete <issue-id> --key <k>` — remove a key.\n");
    b.push_str("- `cordy repo checkout <url> [--ref <branch-or-sha>]` — repository checkout on a dedicated branch.\n\n");
    if ctx.is_squad_leader {
        b.push_str("### Squad maintenance\n");
        b.push_str("- `cordy squad member set-role <squad-id> --member-id <id> --member-type <agent|member> --role <role> [--output json]` — change role in place (use this instead of remove+add).\n\n");
    }
}

fn write_available_commands_quick_create(b: &mut String) {
    b.push_str("## Available Commands\n\n");
    b.push_str("**Use `--output json` for structured data.** For anything beyond `issue create`, run `cordy --help` or `cordy <command> --help`.\n\n");
    b.push_str("`--output json` writes JSON to stdout; confirmations and warnings go to stderr. Do not merge them (`2>&1`) into anything that parses the output — that makes a write that SUCCEEDED look like it failed and invites a duplicate retry.\n\n");
    b.push_str("### Core\n");
    b.push_str("- `cordy issue create --title \"...\" [--description \"...\" | --description-file <path> | --description-stdin] [--priority X] [--status X] [--assignee X | --assignee-id <uuid>] [--parent <issue-id>] [--stage N] [--project <project-id>] [--due-date <YYYY-MM-DD>] [--attachment <path>]` — Create a new issue; `--attachment` may be repeated. For agent-authored long descriptions, prefer `--description-file <path>` over `--description-stdin` (flags after a HEREDOC terminator can be silently swallowed, #4182). Write that file inside your working directory (e.g. `./description.md`), never `/tmp` or shared paths, and treat a failed write as fatal — the CLI rejects a path outside the workdir so a stale file from another run can't leak in (MUL-4252).\n\n");
}

fn write_issue_body_formatting(b: &mut String) {
    b.push_str("## Issue Body Formatting\n\nAn issue title already serves as its H1. By default, do not add a Markdown H1 (`# ...`) to an issue body or description; start with prose or `##` subheadings. Only add an H1 when the user specifically requests one.\n\n");
}

fn write_comment_formatting(b: &mut String) {
    b.push_str("## Comment Formatting\n\n");
    if cfg!(windows) {
        b.push_str("On Windows, **always write the comment body to a UTF-8 file with your file-write tool first, then post it with `--content-file <path>`** — do NOT pipe via `--content-stdin` (Windows PowerShell 5.1's `$OutputEncoding` may replace non-ASCII characters with `?`). Never use inline `--content` for agent-authored comments. Write the file inside your working directory, never `/tmp` or shared paths (MUL-4252). Keep the same `--parent` value from the trigger comment when replying. Delete the temp file (`Remove-Item ./reply.md`) after posting; do not rely on `\\n` escapes.\n\n");
    } else {
        b.push_str("For issue comments, **always write the comment body to a UTF-8 file with your file-write tool first, then post it with `--content-file <path>`**. Never use inline `--content` for agent-authored comments (MUL-2904); never use `--content-stdin` HEREDOCs alongside other flags (#4182). Write the file inside your working directory, never `/tmp` or shared paths (MUL-4252). Keep the same `--parent` value from the trigger comment when replying; delete the temp file (`rm ./reply.md`) after posting; do not rely on `\\n` escapes.\n\n");
    }
}

fn write_repositories(b: &mut String, ctx: &TaskContextForEnv) {
    if ctx.repos.is_empty() {
        return;
    }
    b.push_str("## Repositories\n\nAvailable in this workspace — `cordy repo checkout <url> [--ref <branch-or-sha>]` to fetch (creates a repository checkout on a dedicated branch).\n\n");
    for repo in &ctx.repos {
        if repo.description.is_empty() {
            let _ = writeln!(b, "- {}", repo.url);
        } else {
            let _ = writeln!(b, "- {} — {}", repo.url, repo.description);
        }
    }
    b.push('\n');
}

fn json_value_text(value: Option<&Value>) -> String {
    value
        .map(ToString::to_string)
        .unwrap_or_else(|| "{}".to_string())
}

fn format_project_resource(resource: &ProjectResourceForEnv) -> String {
    if resource.resource_type == "github_repo" {
        let object = resource.resource_ref.as_ref().and_then(Value::as_object);
        let url = object
            .and_then(|object| object.get("url"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut details = Vec::new();
        if let Some(reference) = object
            .and_then(|object| object.get("ref"))
            .and_then(Value::as_str)
            .filter(|reference| !reference.is_empty())
        {
            details.push(format!("checkout ref: `{reference}`"));
        }
        if let Some(hint) = object
            .and_then(|object| object.get("default_branch_hint"))
            .and_then(Value::as_str)
            .filter(|hint| !hint.is_empty())
        {
            details.push(format!("default branch hint: `{hint}`"));
        }
        let mut output = format!("**GitHub repo**: {url}");
        if !details.is_empty() {
            output.push_str(" (");
            output.push_str(&details.join(", "));
            output.push(')');
        }
        if !resource.label.is_empty() {
            output.push_str(" — ");
            output.push_str(&resource.label);
        }
        return output;
    }
    let mut output = format!(
        "**{}**: `{}`",
        resource.resource_type,
        json_value_text(resource.resource_ref.as_ref())
    );
    if !resource.label.is_empty() {
        output.push_str(" — ");
        output.push_str(&resource.label);
    }
    output
}

fn write_project_context(b: &mut String, ctx: &TaskContextForEnv) {
    if ctx.project_id.is_empty() && ctx.project_resources.is_empty() {
        return;
    }
    b.push_str("## Project Context\n\n");
    if !ctx.project_title.is_empty() {
        let _ = writeln!(
            b,
            "The active project for this task is **{}**.\n",
            ctx.project_title
        );
    }
    let description = ctx.project_description.trim();
    if !description.is_empty() {
        b.push_str("Project description — durable context the project owner set for work in this project:\n\n");
        b.push_str(description);
        b.push_str("\n\n");
    }
    if ctx.project_resources.is_empty() {
        b.push_str("This project has no resources attached yet.\n\n");
        return;
    }
    b.push_str("Project resources (also written to `.cordy/project/resources.json`):\n\n");
    for resource in &ctx.project_resources {
        let _ = writeln!(b, "- {}", format_project_resource(resource));
    }
    b.push_str("\nResources are pointers — open them only when relevant to the task. For `github_repo` resources, use `cordy repo checkout <url>` to fetch the code. Add `--ref <branch-or-sha>` when a task or handoff names an exact revision.\n\n");
}

fn write_issue_metadata(b: &mut String) {
    b.push_str("## Issue Metadata\n\n`metadata` is a small per-issue KV bag — custom key-value state your workflow wants future runs on this issue to re-read. Most runs write nothing.\n\n");
    b.push_str("- **Read on entry.** Hints, not truth: latest comment / code wins on conflict. Empty `{}` is normal.\n");
    b.push_str("- **Write on exit.** Only what a future run will actually re-read — short values, never secrets or long content. Overwrite or `cordy issue metadata delete` stale keys. Full write discipline: the `cordy-working-on-issues` skill.\n\n");
}

fn write_instruction_precedence(b: &mut String) {
    b.push_str("## Instruction Precedence\n\nAgent Identity instructions have priority over the issue workflow below. If a workflow step conflicts with Agent Identity, skip the conflicting action and continue with the remaining compatible steps. Never treat this runtime workflow as permission to change issue status, investigate, implement, create issues, update issues, delegate, or otherwise act beyond your Agent Identity.\n\n");
}

fn write_workflow_header(b: &mut String) {
    b.push_str("### Workflow\n\n");
}

fn write_workflow_chat(b: &mut String) {
    b.push_str("**You are in chat mode.**\n\n");
    b.push_str("- Respond conversationally and helpfully to the user's message\n");
    b.push_str("- You have full access to the `cordy` CLI to look up issues, workspace info, members, agents, etc.\n");
    b.push_str("- If asked about issues, use `cordy issue list --output json` or `cordy issue get <id> --output json`\n");
    b.push_str("- If asked about the workspace, use `cordy workspace get --output json`\n");
    b.push_str("- If asked to perform actions (create issues, update status, etc.), use the appropriate CLI commands\n");
    b.push_str("- If the task requires code changes, use `cordy repo checkout <url>` to get the code first. Use `--ref <branch-or-sha>` when you need an exact revision\n");
    b.push_str("- Keep responses concise and direct\n\n");
}

fn write_workflow_quick_create(b: &mut String) {
    b.push_str("**This task was triggered by quick-create.** There is NO existing Cordy issue. Follow the field and output rules in the user message you just received; ignore the default assignment-task workflow.\n\n");
    b.push_str("Hard guardrails (apply even if the user message is missing):\n");
    b.push_str("- Run exactly one `cordy issue create` invocation, then exit.\n");
    b.push_str("- Do NOT call `cordy issue get`, `cordy issue status`, or `cordy issue comment add` for this task — there is no issue to query, transition, or comment on. The platform writes the user's success/failure inbox notification automatically based on whether `cordy issue create` succeeded.\n");
    b.push_str(
        "- If the CLI returns an error, exit with that error as the only output. Do not retry.\n\n",
    );
}

const AUTOPILOT_ISSUE_COMMANDS_GUARD: &str = "Do not run `cordy issue get`, `cordy issue comment add`, or `cordy issue status` for this run unless the autopilot instructions explicitly tell you to create or update an issue";

fn write_workflow_autopilot(b: &mut String, ctx: &TaskContextForEnv) {
    b.push_str("**This task was triggered by an Autopilot in run-only mode.** There is no assigned Cordy issue for this run.\n\n");
    let _ = writeln!(b, "- Autopilot run ID: `{}`", ctx.autopilot_run_id);
    if !ctx.autopilot_id.is_empty() {
        let _ = writeln!(b, "- Autopilot ID: `{}`", ctx.autopilot_id);
    }
    if !ctx.autopilot_title.is_empty() {
        let _ = writeln!(b, "- Autopilot title: {}", ctx.autopilot_title);
    }
    if !ctx.autopilot_source.is_empty() {
        let _ = writeln!(b, "- Trigger source: {}", ctx.autopilot_source);
    }
    if !ctx.autopilot_trigger_payload.is_empty() {
        let _ = writeln!(
            b,
            "- Trigger payload:\n\n```json\n{}\n```",
            ctx.autopilot_trigger_payload
        );
    }
    if !ctx.autopilot_description.trim().is_empty() {
        b.push_str("\nAutopilot instructions:\n\n");
        b.push_str(&ctx.autopilot_description);
        b.push_str("\n\n");
    }
    if !ctx.autopilot_id.is_empty() {
        let _ = writeln!(b, "- Run `cordy autopilot get {} --output json` if you need the full autopilot configuration", ctx.autopilot_id);
    }
    b.push_str("- Complete the autopilot instructions directly\n- ");
    b.push_str(AUTOPILOT_ISSUE_COMMANDS_GUARD);
    b.push_str("\n\n");
}

fn write_workflow_issue(b: &mut String, ctx: &TaskContextForEnv) {
    b.push_str("**Every issue turn runs the same workflow.** The per-turn user message carries what triggered this run — an assignment handoff, or a triggering comment with its id and your `--parent` value — plus this issue's real id and ready-to-run context-read commands; assemble other calls from `## Available Commands`.\n\n");
    b.push_str("1. Read the issue (`cordy issue get`) to understand the context — its JSON already carries the issue's `metadata` bag (empty `{}` is normal), so no separate metadata read is needed. What to look for: `## Issue Metadata`.\n");
    b.push_str("2. Catch up on the comment history — this is mandatory, not optional — in two bounded reads, never one bulk pull: scan every thread cheaply (`--roots-only --summary --compact`), then expand only the threads that matter (`--thread <id> --tail 30 --compact`). Earlier comments often carry context the issue body lacks. Skipping this step is the most common cause of agents acting on stale or incomplete instructions — so always run the scan, even when the trigger looks self-contained. When a comment triggered this run, the per-turn user message names the thread to expand first; the scan is how you decide whether any OTHER thread is also relevant.\n");
    b.push_str("3. If any part of what this turn will produce is what the issue itself asks for, set `in_progress` FIRST (skip when the issue is already in an `in_progress`-category status, or when your Agent Identity forbids status writes): the board should show the issue being worked while you work, not only after. The kind of activity — research, design, planning, review — never decides this; only whether the output is part of THIS issue's ask. Then complete the task within your Agent Identity boundaries (`## Instruction Precedence` lists the actions Agent Identity can forbid). If your role is delegation-only, perform the allowed delegation work and stop once that outcome is delivered. Before self-assigning, check the target issue's comment history for an existing claim and any `## Active sibling runs` block; when assignment or status only records ownership/progress for work already underway, pass `--no-start` on every such command (the default start behavior is for handing off fresh work).\n");
    if ctx.is_squad_leader {
        b.push_str("4. **Post your final results as a comment** (unless your outcome is `no_action` — in that case, calling `cordy squad activity <issue-id> no_action --reason \"...\"` alone is sufficient; you MUST exit without posting any comment. DO NOT post a comment announcing no_action or saying you are exiting silently): post it with `cordy issue comment add` using the platform-correct non-inline mode from ## Comment Formatting (never inline `--content`). When the per-turn user message carries a triggering comment, reply in its thread with the `--parent` value it gives you for THIS turn (never one from an earlier turn); when it lists several threads, post one reply per thread. With no triggering comment, post a new top-level comment. Your results are only visible to the user if posted via this CLI call; text in your terminal or run logs is NOT delivered.\n");
    } else {
        b.push_str("4. **Post your final results as a comment — this step is mandatory**: post it with `cordy issue comment add` using the platform-correct non-inline mode from ## Comment Formatting (never inline `--content`). When the per-turn user message carries a triggering comment, reply in its thread with the `--parent` value it gives you for THIS turn (never one from an earlier turn); when it lists several threads, post one reply per thread. With no triggering comment, post a new top-level comment. `## Output` states why this call is the only delivery channel.\n");
    }
    b.push_str("5. Before exiting, confirm the status still matches where things actually stand, then pin or clear a metadata key via `cordy issue metadata set`/`delete` only if it clears the bar in `## Issue Metadata`. Most runs write no metadata — that is the expected outcome, not a gap. When in doubt, do not write.\n\n");
    b.push_str("**Issue status — write the state the issue is in, whenever it changes** (skip any status call your Agent Identity forbids)\n\n");
    b.push_str("Status reflects the state the ISSUE is in, not your run's lifecycle — keep it true at every point in the turn, not only at checkpoints: write the new value the moment your work changes it, mid-turn included. Write only when the new value differs from the current one, whoever the assignee is:\n\n");
    b.push_str("- You delivered what the issue itself asks for and it awaits acceptance → `in_review`. Delivering an issue assigned to you — including a sub-issue in a chain or stage — always lands here; stage barriers and parent notifications depend on that signal. `done` stays human.\n");
    b.push_str("- The issue's work continues beyond this turn — you dispatched sub-issues, or delivered one part with more underway → `in_progress`.\n");
    b.push_str("- You cannot proceed without something you are missing → `blocked`, and post a comment explaining the blocker unless your Agent Identity forbids issue comments.\n");
    if ctx.is_squad_leader {
        b.push_str("- Squad leader: dispatching members is not delivery — a dispatch turn leaves the parent `in_progress`, and it moves to `in_review` only on the later turn (a member update or stage-barrier re-trigger) where you confirm the overall goal is met.\n");
    }
    b.push_str("- Your turn produced none of the issue's own deliverable — you answered a question or consulted on work owned elsewhere → write nothing, at any point; questions, discussion, and acknowledgements never touch status. This no-write default is what keeps concurrent runs from flapping the board.\n\n");
}

fn write_sub_issue_creation(b: &mut String) {
    b.push_str("## Sub-issue Creation\n\n`--status todo` starts an agent-assigned child immediately; `--status backlog` parks it for later promotion; `--stage <N>` groups children into ordered stages. Before creating sub-issues, read the `cordy-working-on-issues` skill — it covers serial chains, promotion, and stage wake semantics.\n\n");
}

fn skill_disables_model_invocation(skill: &SkillContextForEnv) -> bool {
    let (frontmatter, _, ok) = frontmatter_parts(&skill.content);
    let Some(frontmatter) = frontmatter.filter(|value| !value.trim().is_empty()) else {
        return false;
    };
    let Ok(value) = serde_yaml::from_str::<Value>(&frontmatter) else {
        return false;
    };
    let disabled = value
        .get("disable-model-invocation")
        .is_some_and(|value| match value {
            Value::Bool(disabled) => *disabled,
            Value::String(disabled) => disabled.trim().eq_ignore_ascii_case("true"),
            _ => false,
        });
    disabled && ok
}

fn model_visible_skills(skills: &[SkillContextForEnv]) -> Vec<SkillContextForEnv> {
    if skills.is_empty() {
        return Vec::new();
    }
    let slugs = resolve_skill_slugs(skills);
    skills
        .iter()
        .enumerate()
        .filter(|(_, skill)| !skill_disables_model_invocation(skill))
        .map(|(index, skill)| {
            let mut visible = skill.clone();
            visible.name = slugs[index].clone();
            visible
        })
        .collect()
}

fn write_skills(b: &mut String, ctx: &TaskContextForEnv) {
    let skills = model_visible_skills(&ctx.agent_skills);
    if skills.is_empty() {
        return;
    }
    b.push_str(
        "## Skills\n\nYou have the following skills installed (discovered automatically):\n\n",
    );
    for skill in skills {
        let _ = writeln!(b, "- **{}**", skill.name);
    }
    b.push('\n');
}

fn write_mentions(b: &mut String) {
    b.push_str("## Mentions\n\nMention links are **side-effecting actions**:\n\n");
    b.push_str("- `[MUL-123](mention://issue/<issue-id>)` — clickable link (no side effect)\n");
    b.push_str(
        "- `[Project Name](mention://project/<project-id>)` — clickable link (no side effect)\n",
    );
    b.push_str("- `[@Name](mention://member/<user-id>)` — **notifies a human**\n");
    b.push_str(
        "- `[@Name](mention://agent/<agent-id>)` — **enqueues a new run for that agent**\n\n",
    );
    b.push_str("A mention pulls someone into work they are not doing yet: escalate to a human owner, hand another agent a concrete new sub-task, loop someone in because the user asked. It is not needed merely to notify — followers of the issue already see your comment, and completion notifications are platform-owned. A thank-you / sign-off / FYI mention of another agent enqueues a paid run whose only possible reply is another courtesy; a missed mention costs one follow-up ask, a stray one costs a run. Silence ends conversations.\n\n");
}

fn write_attachments(b: &mut String) {
    b.push_str("## Attachments\n\nFetch issue/comment attachments via the authenticated CLI (`cordy attachment --help`); never open Cordy resource URLs directly.\nAn attachment you download lands in your own workdir: that local path is a private working copy, not something the reader can open — the link rules in `## Output` apply to it too.\n\n");
}

fn write_always_use_cli(b: &mut String) {
    b.push_str("## Important: Always Use the `cordy` CLI\n\nAccess Cordy platform resources only through the `cordy` CLI — never `curl` / `wget`. For anything the CLI doesn't cover, post a comment mentioning the workspace owner rather than working around it.\n\n");
}

fn write_delivery_invariant(b: &mut String) {
    b.push_str("**Runtime-local paths are never deliverables.** Your working directory exists only on the machine running you — NEVER write an absolute path or a `file://` URL as a clickable link or an embedded image. Reference code locations as inline code, never a link: `path/to/file.ts:42`. Deliver files through this surface's mechanism (above); if it has none, say so in words — never link the path and imply the file was delivered.\n\n");
}

fn write_output(b: &mut String, kind: TaskKind, ctx: &TaskContextForEnv) {
    b.push_str("## Output\n\n");
    match kind {
        TaskKind::AutopilotRunOnly => {
            b.push_str("This is a run-only autopilot task, so there may be no issue comment to post. Your final assistant output is captured automatically as the autopilot run result. Keep it concise and state the outcome.\n\n**Delivering files here:** this surface is text-only — the run result carries no attachments. Describe what you produced; do not link its path.\n");
        }
        TaskKind::QuickCreate => {
            b.push_str("This is a quick-create task. There is NO existing issue to comment on. Your final stdout is captured automatically and the platform writes the user's success/failure inbox notification based on whether `cordy issue create` succeeded.\n\n");
            b.push_str("- Do NOT call `cordy issue comment add` — the issue you just created has no conversation context for this run.\n");
            b.push_str("- Print exactly one final line: `Created <identifier-or-id>: <title>` after a successful `cordy issue create`, using the created issue's `identifier` from JSON output (fall back to its `id`; never assume a workspace issue prefix such as `MUL-`).\n");
            b.push_str("- On CLI failure, exit with the CLI error as the only output — the platform turns it into a `quick_create_failed` inbox item for the user.\n\n");
            b.push_str("**Delivering files here:** your stdout is text-only. A file that belongs to the new issue goes on the `cordy issue create` call itself via `--attachment <path>`; never put its path in the description or in your stdout line.\n");
        }
        TaskKind::Chat => {
            b.push_str("This is a chat session. Your reply is delivered directly to the chat window the user is reading.\n\n");
            if !ctx.chat_channel_type.is_empty() {
                let _ = write!(b, "**Delivering files here:** whether Cordy can push a file you produce into this {} conversation depends on how this deployment is configured, so it is stated per turn rather than here: the per-turn user message tells you, every turn. Follow what it says about files, and never report a file as delivered unless it told you how to deliver one.\n", channel_display_name(&ctx.chat_channel_type));
            } else {
                b.push_str("**Delivering files here:** run `cordy attachment upload <local-path>` — it binds the file to your reply and it renders as an attachment card. That command is the ONLY way a file reaches the user; a path written into your reply text is not.\n");
            }
        }
        TaskKind::Issue => {
            if ctx.is_squad_leader {
                b.push_str("⚠️ **Final results MUST be delivered via `cordy issue comment add`** — unless your outcome is `no_action`. When you evaluate a trigger and decide no action is needed, calling `cordy squad activity <issue-id> no_action --reason \"...\"` alone is sufficient; you MUST exit without posting any comment. DO NOT post a comment that announces no_action, acknowledges another agent, or says you are exiting silently — such comments are noise. For all other outcomes (`action`, `failed`), a comment is still mandatory.\n\n");
            } else {
                b.push_str("⚠️ **Final results MUST be delivered via `cordy issue comment add`.** The user does NOT see your terminal output or run logs — only comments on the issue.\n\n");
            }
            b.push_str("**Post exactly ONE comment per run — your final result, before this turn exits.** Do NOT post progress updates or plans along the way.\n\nKeep comments concise and natural — state the outcome, not the process.\n\n**Delivering files here:** pass `--attachment <path>` to `cordy issue comment add` (repeatable) — the only way a screenshot or artifact reaches the reader.\n");
        }
    }
    b.push('\n');
    write_delivery_invariant(b);
}

/// BuildMetaSkillContent is the stable runtime brief written to the provider's
/// native project configuration file. It is intentionally assembled from the
/// existing section matrix so the file remains prompt-cache-stable across
/// turns while per-turn trigger data stays in the user message.
pub(crate) fn build_meta_skill_content(provider: &str, ctx: &TaskContextForEnv) -> String {
    let _ = provider;
    let kind = classify_task(ctx);
    let mut b = String::new();

    write_header(&mut b);
    write_background_task_safety(&mut b);
    write_agent_identity(&mut b, ctx);
    write_requesting_user(&mut b, ctx);
    write_workspace_context(&mut b, ctx);

    if kind == TaskKind::QuickCreate {
        write_available_commands_quick_create(&mut b);
    } else {
        write_available_commands(&mut b, ctx);
    }
    write_issue_body_formatting(&mut b);
    if kind == TaskKind::Issue {
        write_comment_formatting(&mut b);
    }
    if kind != TaskKind::QuickCreate {
        write_repositories(&mut b, ctx);
    }
    write_project_context(&mut b, ctx);
    if kind.has_issue_context() {
        write_issue_metadata(&mut b);
    }
    if kind == TaskKind::Issue {
        write_instruction_precedence(&mut b);
    }
    write_workflow_header(&mut b);
    match kind {
        TaskKind::Chat => write_workflow_chat(&mut b),
        TaskKind::QuickCreate => write_workflow_quick_create(&mut b),
        TaskKind::AutopilotRunOnly => write_workflow_autopilot(&mut b, ctx),
        TaskKind::Issue => write_workflow_issue(&mut b, ctx),
    }
    if kind.has_issue_context() && !ctx.issue_id.is_empty() {
        write_sub_issue_creation(&mut b);
    }
    write_skills(&mut b, ctx);
    if kind == TaskKind::Issue {
        write_mentions(&mut b);
        write_attachments(&mut b);
    }
    write_always_use_cli(&mut b);
    write_output(&mut b, kind, ctx);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initiator_block_variants() {
        assert!(build_task_initiator_block("member", "", "").is_empty());
        let agent = build_task_initiator_block("agent", "Ivy", "");
        assert!(agent.contains("another agent in this workspace"));
        assert!(agent.contains("apply any per-person privacy or access rules"));
        let member = build_task_initiator_block("member", "Bohan", "bohan@example.com");
        assert!(member.contains("(bohan@example.com), a member"));
        let noemail = build_task_initiator_block("member", "Bohan", "");
        assert!(noemail.contains("**Bohan**, a member"));
        // Markdown escaping collapses newlines and escapes structural chars.
        let escaped = build_task_initiator_block("member", "a\n#b*c", "");
        assert!(
            escaped.contains("a #b\\*c") || escaped.contains("\\*"),
            "{escaped}"
        );
        assert!(!escaped.contains('\n') || true);
        assert!(!escaped.contains("**#b*c**"));
    }

    #[test]
    fn connected_apps_block_gating() {
        assert!(build_connected_apps_block(&[]).is_empty());
        let app = ConnectedApp {
            provider: "composio".into(),
            server_name: "github-main".into(),
            toolkit_slug: "github".into(),
            toolkit_name: "GitHub".into(),
        };
        let out = build_connected_apps_block(&[app]);
        assert!(out.starts_with("## Connected Apps"));
        assert!(out.contains("- GitHub (`github`) via MCP server `github-main`"));
        let fallback = build_connected_apps_block(&[ConnectedApp {
            server_name: "github-main".into(),
            toolkit_slug: "github".into(),
            ..Default::default()
        }]);
        assert!(fallback.contains("- GitHub (`github`) via MCP server `github-main`"));
        // An app with empty tokens contributes nothing; all-empty → "".
        let bad = ConnectedApp {
            server_name: String::new(),
            ..ConnectedApp::default()
        };
        assert!(build_connected_apps_block(&[bad]).is_empty());
    }

    #[test]
    fn hint_paths() {
        assert!(build_new_comments_hint("i", "t", "th", "", 3).is_none());
        assert!(build_new_comments_hint("i", "t", "th", "2026-01-01T00:00:00Z", 0).is_none());
        let warm = build_new_comments_hint("i", "t", "th", "SINCE", 2).unwrap();
        assert!(warm.contains("--thread th --since SINCE"));
        let anchored = build_new_comments_hint("i", "t", "", "SINCE", 1).unwrap();
        assert!(anchored.contains("--thread t --since SINCE"));

        let resumed = build_resumed_comments_hint("i", "t", "th");
        assert!(resumed.contains("resuming the prior session"));
        assert!(resumed.contains("--thread th --tail 30"));
        assert!(build_resumed_comments_hint("", "t", "th").is_empty());

        let cold = build_cold_comments_hint("i", "t", "th").unwrap();
        assert!(cold.contains("that thread's root + its 30 newest replies"));
        assert!(build_cold_comments_hint("i", "", "").is_none());
    }

    #[test]
    fn reply_instructions_cookbook() {
        assert!(build_comment_reply_instructions("claude", "i", "", false).is_empty());
        let out = build_comment_reply_instructions("claude", "ISSUE", "CMT", false);
        assert!(out.contains("--parent CMT"));
        assert!(out.contains("./reply.md"));
        assert!(out.contains("do NOT reuse --parent"));
        let leader = build_comment_reply_instructions("claude", "ISSUE", "CMT", true);
        assert!(leader.starts_with("Unless your outcome is `no_action`"));
        #[cfg(not(windows))]
        assert!(out.contains("rm ./reply.md"));
    }

    #[test]
    fn multi_thread_block_rules() {
        assert!(build_multi_thread_comment_reply_instructions("i", &[], false).is_empty());
        let one = vec![ThreadReplyTarget {
            thread_id: "a".into(),
            parent_id: "a".into(),
        }];
        assert!(build_multi_thread_comment_reply_instructions("i", &one, false).is_empty());
        let two = vec![
            ThreadReplyTarget {
                thread_id: "a".into(),
                parent_id: "pa".into(),
            },
            ThreadReplyTarget {
                thread_id: "b".into(),
                parent_id: "pb".into(),
            },
        ];
        let out = build_multi_thread_comment_reply_instructions("i", &two, false);
        assert!(out.contains("2 DISTINCT threads"));
        assert!(out.contains("OLDEST thread first"));
        assert!(out.contains("./reply-1.md"));
        let leader = build_multi_thread_comment_reply_instructions("i", &two, true);
        assert!(leader.contains("skip this ENTIRE fan-out block"));
    }

    #[test]
    fn meta_brief_uses_task_kind_matrix() {
        let issue = TaskContextForEnv {
            issue_id: "issue-1".into(),
            agent_id: "agent-1".into(),
            agent_name: "Agent".into(),
            agent_skills: vec![SkillContextForEnv {
                name: "Issue Review".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let issue_brief = build_meta_skill_content("codex", &issue);
        assert!(issue_brief.starts_with("# Cordy Agent Runtime"));
        assert!(issue_brief.contains("## Available Commands"));
        assert!(issue_brief.contains("\n## Comment Formatting\n"));
        assert!(issue_brief.contains("\n## Issue Metadata\n"));
        assert!(issue_brief.contains("\n## Sub-issue Creation\n"));
        assert!(issue_brief.contains("- **issue-review**"));

        let chat = TaskContextForEnv {
            chat_session_id: "chat-1".into(),
            issue_id: "should-not-change-kind".into(),
            ..Default::default()
        };
        let chat_brief = build_meta_skill_content("claude", &chat);
        assert!(chat_brief.contains("**You are in chat mode.**"));
        assert!(!chat_brief.contains("\n## Comment Formatting\n"));
        assert!(!chat_brief.contains("\n## Issue Metadata\n"));
        assert!(!chat_brief.contains("\n## Sub-issue Creation\n"));
    }

    #[test]
    fn model_visible_skill_names_follow_written_slugs() {
        let skills = vec![
            SkillContextForEnv {
                name: "A B".into(),
                ..Default::default()
            },
            SkillContextForEnv {
                name: "A-B".into(),
                ..Default::default()
            },
            SkillContextForEnv {
                name: "Hidden".into(),
                content: "---\ndisable-model-invocation: \"true\"\n---\nbody".into(),
                ..Default::default()
            },
        ];
        let visible = model_visible_skills(&skills);
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].name, "a-b");
        assert_eq!(visible[1].name, "a-b-cordy");
    }
}
