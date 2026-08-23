//! Port of `server/internal/daemon/prompt.go` (686 lines).
//!
//! Deviations from Go:
//! - execenv pieces that already exist in `crate::execenv::channel_type`
//!   (channel constants, audience, display names, transcript/files policy)
//!   are used directly; the remaining execenv helpers (session-continuity
//!   notices, initiator/connected-apps blocks, comment hints, reply
//!   instructions) are local seam stand-ins in [`execenv_seams`] mirroring
//!   `runtime_config_sections.go`, `reply_instructions.go`, and
//!   `runtime_config.go`; S9-integration: swap to the execenv ports when
//!   those lanes land.
//! - `fmt.Fprintf(&b, ...)` → `format!` + `push_str`.
//! - `strings.NewReplacer("\r"," ","\n"," ")` → chained `replace`.

// S9-integration: consumed by manager/task-dispatch wiring that lands with
// integration; silence dead-code until then.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::execenv::channel_type::{
    audience_of, channel_carries_files, channel_display_name, surface_persists_transcript,
    ChatAudience, CHANNEL_TYPE_SLACK,
};
use crate::execenv::execenv::ThreadReplyTarget;
use crate::slash_skill::extract_slash_skills;
use crate::types::{ActiveSiblingRunData, Task};

/// `sessionContinuityNoticeFor` (prompt.go:15–30): picks the notice matching
/// what this surface actually lost — the question is whether the conversation
/// is still READABLE, not whether it is a chat (MUL-5722).
fn session_continuity_notice_for(task: &Task) -> String {
    if task.chat_session_id.is_empty() {
        return execenv_seams::SESSION_CONTINUITY_NOTICE_ISSUE.to_string();
    }
    if task.chat_channel_type == CHANNEL_TYPE_SLACK {
        return execenv_seams::SESSION_CONTINUITY_NOTICE_CHANNEL_HISTORY.to_string();
    }
    // Every other chat session that persists a transcript reads it back via
    // `cordy chat history`; Slack alone reads the live channel. Only a
    // surface that never stored a transcript falls through to Unrecoverable.
    if surface_persists_transcript(&task.chat_channel_type) {
        return execenv_seams::SESSION_CONTINUITY_NOTICE_CHAT_TRANSCRIPT.to_string();
    }
    execenv_seams::SESSION_CONTINUITY_NOTICE_UNRECOVERABLE.to_string()
}

/// `backendResumeContinuityNotice` (prompt.go:43–48): the notice the BACKEND
/// should inject if it lands on a fresh thread, or "" when the prompt already
/// carries one. Deriving the backend's copy from the daemon's makes a
/// duplicate structurally impossible rather than merely unlikely.
pub(crate) fn backend_resume_continuity_notice(task: &Task) -> String {
    if task.prior_session_resume_unavailable {
        return String::new();
    }
    session_continuity_notice_for(task)
}

/// `perTurnContextBlocks` (prompt.go:63–72): run-scoped context blocks that
/// used to live in the runtime brief. Appending them to the per-turn user
/// message keeps them after the cached prefix (MUL-5377). Returns "" when
/// none of the blocks apply.
fn per_turn_context_blocks(task: &Task) -> String {
    let mut b = String::new();
    b.push_str(&build_active_sibling_runs_block(
        &task.issue_id,
        &task.active_sibling_runs,
    ));
    if task.prior_session_resume_unavailable {
        b.push_str(&session_continuity_notice_for(task));
    }
    b.push_str(&execenv_seams::build_task_initiator_block(
        &task.initiator_type,
        &task.initiator_name,
        &task.initiator_email,
    ));
    b.push_str(&execenv_seams::build_connected_apps_block(&task.connected_apps));
    b
}

/// `buildActiveSiblingRunsBlock` (prompt.go:74–106).
fn build_active_sibling_runs_block(current_issue_id: &str, runs: &[ActiveSiblingRunData]) -> String {
    // Sibling issue work is useful context only for another issue task.
    if current_issue_id.is_empty() || runs.is_empty() {
        return String::new();
    }
    let mut b = String::new();
    b.push_str("## Active sibling runs\n\n");
    b.push_str(
        "This agent has other in-flight issue tasks. Before starting overlapping code or PR work, \
         check this issue's comment history for a claim or handoff",
    );
    let _ = write!(
        b,
        " (`cordy issue comment list {current_issue_id} --roots-only --summary --compact --output json`)"
    );
    b.push_str(
        " and inspect relevant siblings with the `run-messages` commands below — coordinate with \
         existing work instead of opening a second PR. For writes that only record ownership or \
         status of work already underway, use `--no-start` on `cordy issue assign`/`update`/`status`.\n\n",
    );
    for run in runs {
        let issue_label = if run.issue_identifier.is_empty() {
            &run.issue_id
        } else {
            &run.issue_identifier
        };
        let _ = write!(b, "- {issue_label} — task `{}`, status `{}`", run.task_id, run.status);
        if !run.started_at.is_empty() {
            let _ = write!(b, ", started {}", run.started_at);
        } else if !run.created_at.is_empty() {
            let _ = write!(b, ", created {}", run.created_at);
        }
        let title = run.issue_title.replace('\r', " ").replace('\n', " ").trim().to_string();
        if !title.is_empty() {
            let _ = write!(b, ": {title}");
        }
        let _ = writeln!(b, "; inspect: `cordy issue run-messages {}`", run.task_id);
    }
    b.push('\n');
    b
}

/// `BuildPrompt` (prompt.go:115–127): constructs the task prompt for an agent
/// CLI. Keep this minimal — detailed instructions live in CLAUDE.md /
/// AGENTS.md injected by execenv.InjectRuntimeConfig.
pub(crate) fn build_prompt(task: &Task, provider: &str) -> String {
    let mut body = build_prompt_body(task, provider);
    // Run-scoped context is appended, never prepended: everything ahead of it
    // is stable across runs of a resumed session (MUL-5377).
    let blocks = per_turn_context_blocks(task);
    if !blocks.is_empty() {
        if !body.ends_with("\n\n") {
            body.push('\n');
        }
        body.push_str(&blocks);
    }
    body
}

/// `buildPromptBody` (prompt.go:129–155).
fn build_prompt_body(task: &Task, provider: &str) -> String {
    if !task.chat_session_id.is_empty() {
        return build_chat_prompt(task);
    }
    if !task.trigger_comment_id.is_empty() {
        return build_comment_prompt(task, provider);
    }
    if !task.autopilot_run_id.is_empty() {
        return build_autopilot_prompt(task);
    }
    if !task.quick_create_prompt.is_empty() {
        return build_quick_create_prompt(task);
    }
    let mut b = String::new();
    b.push_str("You are running as a local coding agent for a Cordy workspace.\n\n");
    let _ = write!(b, "Your assigned issue ID is: {}\n\n", task.issue_id);
    // Assignment handoff (MUL-3375): frame it as a handoff, not a comment to
    // reply to — there is no comment thread to answer here.
    if !task.handoff_note.is_empty() {
        b.push_str(
            "You were handed this issue with a handoff note. Treat it as the assigner's scoping \
             instruction for this run; follow it before doing anything broader, and do not reply \
             to it as if it were a comment:\n\n",
        );
        let _ = write!(b, "> {}\n\n", task.handoff_note);
    }
    let _ = write!(
        b,
        "Start by running `cordy issue get {} --output json` to understand your task, then complete it.\n",
        task.issue_id
    );
    let _ = write!(
        b,
        "For comment history, follow the rule in your runtime workflow file (assignment-triggered \
         tasks treat the read as mandatory). Scan the threads first with `cordy issue comment list \
         {} --roots-only --summary --compact --output json`, then expand only what matters with \
         `--thread <thread-id> --tail 30`. For `--since` incremental polling, pagination, and \
         folding, see `cordy issue comment list --help`.\n",
        task.issue_id
    );
    b
}

/// `buildQuickCreatePrompt` (prompt.go:164–258): the user typed one natural-
/// language sentence in the create-issue modal; the agent translates it into
/// one `cordy issue create` invocation. No issue exists yet, so the agent must
/// NOT call `cordy issue get` or attempt to comment.
fn build_quick_create_prompt(task: &Task) -> String {
    let mut b = String::new();
    b.push_str("You are running as a quick-create assistant for a Cordy workspace.\n\n");
    b.push_str(
        "A user captured the following input via the quick-create modal. There is NO existing \
         issue. Your job is to create a well-formed issue from this input with a single \
         `cordy issue create` command.\n\n",
    );
    let _ = write!(b, "User input:\n> {}\n\n", task.quick_create_prompt);

    b.push_str("Field rules:\n\n");

    // title
    b.push_str("- **title**: required. A concise but semantically rich summary. If the input references external resources (PRs, issues, URLs), use your judgment on whether fetching the resource would produce a meaningfully better title — e.g. \"review PR #123\" → \"Review PR #123: Refactor auth module to OAuth2\". Strip filler words but preserve key semantic information.\n\n");

    // description — the core optimization
    b.push_str("- **description**: The description is the executing agent's primary context. Aim for high fidelity — they should grasp the user's intent as if they had read the raw input themselves. Use a two-section structure:\n\n");
    b.push_str("  1. **User request** — Faithfully restate what the user wants in their own words. Preserve specific names, identifiers, file paths, code snippets, and technical terms verbatim. Strip non-spec material before writing it (this is removal, not paraphrasing): verbal routing wrappers about creating the issue or routing it (e.g. \"create an issue\", \"分配给 X\", \"让 @X 处理\") and pure conversational fillers (e.g. \"对吧？\"). When in doubt, keep it.\n\n");
    b.push_str("     CC exception: `cordy issue create` has no `--subscriber` flag, and the platform auto-subscribes members whose `[@Name](mention://member/<uuid>)` link appears in the description. When the user wrote \"cc @Y\", strip the verbal \"cc\" wrapper from the User request body and append a final `CC: <mention link(s)>` line to the description so the cc routing still fires.\n\n");
    b.push_str("  2. **Context** — include ONLY when the input cited external resources AND you successfully fetched them AND they produced verifiable facts worth recording. Summarize facts only (e.g. \"PR #45 changes auth to JWT\"), not interpretation or unsolicited reference implementations. If you have nothing factual to add, omit the section entirely — never use it as an apology log for resources you could not fetch.\n\n");
    b.push_str("  Hard rules: never invent requirements, implementation details, or acceptance criteria the user did not express; never reduce multi-sentence input to a single vague sentence; never echo the title.\n\n");
    b.push_str("  Passing the description: a short, single-line body with no code, quotes, backticks, `$()`, or other special characters may go inline via `--description \"...\"`. Anything multi-line, or containing code snippets / file paths / quotes / backticks / `$()` / special characters, or otherwise long — which quick-create descriptions usually are — MUST be written to `./description.md` and passed with `--description-file ./description.md`; passing rich text inline lets the shell rewrite or truncate it (MUL-2904). That file MUST live inside your current working directory (e.g. `./description.md`) — never `/tmp` or any machine-shared path, where a different run may have left a stale file that would silently become this issue's description. If the file write fails for any reason, stop and fix it; never run `--description-file` against a file whose write did not succeed.\n\n");

    // priority
    if !task.quick_create_priority.is_empty() {
        let _ = write!(
            b,
            "- **priority**: required for this run. Pass `--priority {}`; the quick-create selection is authoritative.\n\n",
            task.quick_create_priority
        );
    } else {
        b.push_str("- **priority**: one of `urgent`, `high`, `medium`, `low`, or omit. Map P0/P1 → urgent/high; \"asap\" → urgent. If unspecified, omit.\n\n");
    }

    // assignee
    b.push_str("- **assignee**:\n");
    b.push_str("    - When the user names someone (\"assign to X\" / \"@X\"), call `cordy workspace member list --output json`, `cordy agent list --output json`, and `cordy squad list --output json` and find the matching entity by display name. Squads are first-class assignees too — a squad name (e.g. \"Super Human\") routes work to the squad leader, who then delegates. On a clean unambiguous match, prefer `--assignee-id <uuid>` using the `user_id` (member) or `id` (agent or squad) from that JSON — UUID matching is exact and robust to name collisions in workspaces with overlapping names. `--assignee <name>` (fuzzy) is acceptable as a fallback when names are unambiguous. On no match or ambiguous match, do NOT pass either flag — instead append a final line to the description: `Unrecognized assignee: X`.\n");
    b.push_str("    - Treat bare @-routing as an assignee directive even when the user did not write the English word \"assign\". This includes Chinese imperatives like `让 @独立团 review 这个 PR`, `给 @X 处理`, or `交给 @X`; strip the leading `@`/`＠` before matching display names. Do not keep that routing wrapper or `@Name` in the description unless it is a true CC-style notification rather than ownership. If the matched entity is a squad, pass the squad's `id` as `--assignee-id`, not the leader agent's id.\n");
    let mut agent_id = String::new();
    let mut agent_name = String::new();
    if let Some(agent) = &task.agent {
        agent_id = agent.id.clone();
        agent_name = agent.name.clone();
    }
    if !task.squad_id.is_empty() {
        // The user opened quick-create with a SQUAD selected: the squad is
        // the expected owner — assigning to the leader would mask the
        // squad's delegation flow.
        if !task.squad_name.is_empty() {
            let _ = write!(b,
                "    - When the user did NOT name an assignee, default to the picker SQUAD {:?}: pass `--assignee-id {:?}` (the squad's UUID). The user opened quick-create with the squad selected; you (the leader agent) are running on the squad's behalf, so the squad — not you — is the expected owner. Never leave the issue unassigned, and do not assign it to your own agent UUID.\n\n",
                task.squad_name, task.squad_id);
        } else {
            let _ = write!(b,
                "    - When the user did NOT name an assignee, default to the picker SQUAD: pass `--assignee-id {:?}` (the squad's UUID). The user opened quick-create with the squad selected; you (the leader agent) are running on the squad's behalf, so the squad — not you — is the expected owner. Never leave the issue unassigned, and do not assign it to your own agent UUID.\n\n",
                task.squad_id);
        }
    } else if !agent_id.is_empty() {
        let _ = write!(b,
            "    - When the user did NOT name an assignee, default to YOURSELF: pass `--assignee-id {:?}` (your agent UUID). The picker agent is the expected owner because the user opened quick-create with you selected — never leave the issue unassigned. Use the UUID flag, not `--assignee <name>`, so the assignment is unambiguous even when other agents share part of your name.\n\n",
            agent_id);
    } else if !agent_name.is_empty() {
        let _ = write!(b,
            "    - When the user did NOT name an assignee, default to YOURSELF: pass `--assignee {:?}`. The picker agent is the expected owner because the user opened quick-create with you selected — never leave the issue unassigned.\n\n",
            agent_name);
    } else {
        b.push_str("    - When the user did NOT name an assignee, default to YOURSELF (the picker agent): pass `--assignee-id <your agent UUID>` (preferred) or `--assignee <your agent name>`. Never leave the issue unassigned.\n\n");
    }

    if !task.quick_create_due_date.is_empty() {
        let _ = write!(
            b,
            "- **due-date**: required for this run. Pass `--due-date {}`; the quick-create selection is authoritative.\n\n",
            task.quick_create_due_date
        );
    }

    // project — pinned by the modal when the user picked one, otherwise
    // omitted so the platform routes to the workspace default.
    if !task.project_id.is_empty() {
        if !task.project_title.is_empty() {
            let _ = write!(b,
                "- **project**: required for this run. Pass `--project {:?}` so the new issue lands in project {:?} (the user picked it in the quick-create modal). Do not infer a different project from the prompt text — the modal selection is authoritative.\n",
                task.project_id, task.project_title);
        } else {
            let _ = write!(b,
                "- **project**: required for this run. Pass `--project {:?}` so the new issue lands in the project the user picked in the quick-create modal. Do not infer a different project from the prompt text — the modal selection is authoritative.\n",
                task.project_id);
        }
    } else {
        b.push_str("- **project**: omit. The platform will route the issue to the workspace default.\n");
    }
    // parent — pinned by the modal when the user opened it from "Add sub
    // issue" on an existing issue.
    if !task.parent_issue_id.is_empty() {
        if !task.parent_issue_identifier.is_empty() {
            let _ = write!(b,
                "- **parent**: required for this run. Pass `--parent {:?}` so the new issue is filed as a sub-issue of {} (the user opened quick-create from that issue's \"Add sub issue\" entry). Do not infer a different parent from the prompt text — the modal entry point is authoritative.\n",
                task.parent_issue_id, task.parent_issue_identifier);
        } else {
            let _ = write!(b,
                "- **parent**: required for this run. Pass `--parent {:?}` so the new issue is filed as a sub-issue of the parent the user picked in the quick-create modal. Do not infer a different parent from the prompt text — the modal entry point is authoritative.\n",
                task.parent_issue_id);
        }
    }
    b.push_str("- **status**: omit (defaults to `todo`).\n");
    b.push_str("- **attachments**: `--attachment` takes LOCAL file paths, never URLs. Image URLs in the user input are already markdown — keep them inline. Files you produced: see `## Output`.\n\n");

    // output format
    b.push_str("Output format:\n");
    b.push_str("- Run exactly one `cordy issue create --output json` invocation. Do not retry for any reason — even on non-zero exit. The issue may already exist; another attempt would create a duplicate.\n");
    b.push_str("- Parse the JSON response to read the created issue's `identifier` (preferred) or `id` (fallback). Do not scrape human output and do not assume any workspace issue prefix such as `MUL-`; workspaces can use custom prefixes.\n");
    b.push_str("- After success, print exactly one line: `Created <identifier-or-id>: <title>` and exit. No commentary, no follow-up tool calls.\n");
    b.push_str("- Do NOT call `cordy issue get` or `cordy issue comment add` — there is no issue to query or comment on.\n");
    b.push_str("- On CLI error or JSON parse error, exit with the error as the only output. The platform writes a failure notification automatically.\n");
    b
}

/// `buildCommentPrompt` (prompt.go:267–387): the triggering comment content is
/// embedded directly so the agent cannot miss it; reply instructions are
/// re-emitted every turn so resumed sessions cannot carry forward a previous
/// turn's --parent UUID.
fn build_comment_prompt(task: &Task, provider: &str) -> String {
    let mut b = String::new();
    b.push_str("You are running as a local coding agent for a Cordy workspace.\n\n");
    let _ = write!(b, "Your assigned issue ID is: {}\n\n", task.issue_id);
    if !task.trigger_comment_content.is_empty() {
        let author_label = author_label(&task.trigger_author_type, &task.trigger_author_name);
        let _ = write!(
            b,
            "[NEW COMMENT] {author_label} just left a new comment. Focus on THIS comment — do not confuse it with previous ones:\n\n"
        );
        let _ = write!(b, "> {}\n\n", task.trigger_comment_content);
        // MUL-4195: comments that arrived before this run started were folded
        // into it rather than dropped; the agent must ALSO address them.
        if !task.coalesced_comments.is_empty() {
            let _ = write!(
                b,
                "This run also covers {} earlier comment(s) posted before it started — you must read and address them too, not just the one above. They may be in different threads, so each is reproduced here with its own thread:\n\n",
                task.coalesced_comments.len()
            );
            for cc in &task.coalesced_comments {
                let label = if cc.author_type == "system" {
                    "The platform".to_string()
                } else if cc.author_type == "agent" {
                    let name = if cc.author_name.is_empty() { "another agent" } else { &cc.author_name };
                    format!("Another agent ({name})")
                } else if !cc.author_name.is_empty() {
                    cc.author_name.clone()
                } else {
                    "A user".to_string()
                };
                let _ = write!(b, "- comment {}", cc.id);
                if !cc.created_at.is_empty() {
                    let _ = write!(b, " ({label}, {})", cc.created_at);
                } else {
                    let _ = write!(b, " ({label})");
                }
                if !cc.thread_id.is_empty() {
                    let _ = write!(b, " [thread {}]", cc.thread_id);
                }
                b.push_str(":\n");
                let quoted = cc.content.trim().replace('\n', "\n  > ");
                let _ = writeln!(b, "  > {quoted}");
            }
            let _ = write!(
                b,
                "\nIf you need the surrounding discussion for any of them, fetch its thread with `cordy issue comment list {} --thread <thread-id> --tail 30 --compact --output json` using the thread id shown above.\n\n",
                task.issue_id
            );
        } else if !task.coalesced_comment_ids.is_empty() {
            // MUL-5442 replacement: per-id lookup via `--thread <comment-id>`,
            // which is deterministic because `--thread` accepts ANY comment id.
            let _ = write!(
                b,
                "This run also covers {} earlier comment(s) posted before it started — you must read and address every one of them, not just the one above: {}. They may be in DIFFERENT threads, so do not assume they share the triggering thread.\n\n",
                task.coalesced_comment_ids.len(),
                task.coalesced_comment_ids.join(", ")
            );
            if !task.new_comments_since.is_empty() {
                let _ = write!(
                    b,
                    "Start with `cordy issue comment list {} --since {} --compact --output json`. Treat that as a candidate window, not a guarantee — it also carries unrelated comments, and a retried run can carry ids older than the window. Check every id above against the result.\n\n",
                    task.issue_id, task.new_comments_since
                );
            }
            let _ = write!(
                b,
                "Fetch each id you still need directly: `cordy issue comment list {} --thread <comment-id> --tail 30 --compact --output json`. `--thread` accepts a reply id, not just a thread root, so you do not need to know which thread the comment lives in. If it is older than those 30 replies, page back with the `Next reply cursor` values (`--before` / `--before-id`) until it appears. Do not finish this turn until every id above is accounted for.\n\n",
                task.issue_id
            );
        }
        if task_is_squad_leader(task) {
            let _ = write!(
                b,
                "⚠️ **Squad leader no_action rule:** If you decide no action is needed, call `cordy squad activity {} no_action --reason \"...\"` and EXIT. DO NOT post any comment — not even one that says \"no action needed\" or \"exiting silently\". The squad activity call records your decision; a comment is redundant noise.\n\n",
                task.issue_id
            );
        }
    }
    let _ = write!(
        b,
        "Start by running `cordy issue get {} --output json` to understand your task, then decide how to proceed.\n\n",
        task.issue_id
    );
    // Comment-reading pointer: warm-with-delta → resumed → cold → plain read.
    let hint = execenv_seams::build_new_comments_hint(
        &task.issue_id,
        &task.trigger_comment_id,
        &task.trigger_thread_id,
        &task.new_comments_since,
        task.new_comment_count,
    );
    if !hint.is_empty() {
        b.push_str(&hint);
    } else if !task.prior_session_id.is_empty() {
        b.push_str(&execenv_seams::build_resumed_comments_hint(
            &task.issue_id,
            &task.trigger_comment_id,
            &task.trigger_thread_id,
        ));
    } else {
        let cold = execenv_seams::build_cold_comments_hint(
            &task.issue_id,
            &task.trigger_comment_id,
            &task.trigger_thread_id,
        );
        if !cold.is_empty() {
            b.push_str(&cold);
        } else {
            let _ = write!(
                b,
                "Read the discussion: scan with `cordy issue comment list {} --roots-only --summary --compact --output json`, then expand what matters with `--thread <thread-id> --tail 30`.\n\n",
                task.issue_id
            );
        }
    }
    // Reply routing: coalesced comments spanning MORE THAN ONE root thread get
    // per-thread fan-out instead of one merged comment (MUL-4348).
    let targets = comment_reply_threads(task);
    if targets.len() >= 2 {
        b.push_str(&execenv_seams::build_multi_thread_comment_reply_instructions(
            &task.issue_id,
            &targets,
            task_is_squad_leader(task),
        ));
    } else {
        b.push_str(&execenv_seams::build_comment_reply_instructions(
            provider,
            &task.issue_id,
            &task.trigger_comment_id,
            task_is_squad_leader(task),
        ));
    }
    b
}

/// Trigger/coalesced author label shared by the comment prompt paths.
fn author_label(author_type: &str, author_name: &str) -> String {
    if author_type == "system" {
        "The platform".to_string()
    } else if author_type == "agent" {
        let name = if author_name.is_empty() { "another agent" } else { author_name };
        format!("Another agent ({name})")
    } else {
        "A user".to_string()
    }
}

/// `commentReplyThreads` (prompt.go:405–448): groups this run's trigger +
/// coalesced comments by their root thread, in first-seen order; the newest
/// comment wins each thread's reply target. Returns empty when there is no
/// trigger or only a single distinct thread.
fn comment_reply_threads(task: &Task) -> Vec<ThreadReplyTarget> {
    if task.trigger_comment_id.is_empty() {
        return Vec::new();
    }
    // A comment with no explicit thread id is a root comment: its own thread.
    let thread_key = |thread_id: &str, comment_id: &str| -> String {
        if thread_id.is_empty() { comment_id.to_string() } else { thread_id.to_string() }
    };

    let mut order: Vec<String> = Vec::with_capacity(task.coalesced_comments.len() + 1);
    let mut parent_by_thread: HashMap<String, String> =
        HashMap::with_capacity(task.coalesced_comments.len() + 1);
    // First-seen order, newest-comment-wins reply target: inputs are
    // chronological (coalesced oldest-first, trigger last).
    let mut note = |order: &mut Vec<String>, thread_id: String, parent_id: String| {
        if !parent_by_thread.contains_key(&thread_id) {
            order.push(thread_id.clone());
        }
        parent_by_thread.insert(thread_id, parent_id);
    };

    for cc in &task.coalesced_comments {
        note(&mut order, thread_key(&cc.thread_id, &cc.id), cc.id.clone());
    }
    note(
        &mut order,
        thread_key(&task.trigger_thread_id, &task.trigger_comment_id),
        task.trigger_comment_id.clone(),
    );

    if order.len() <= 1 {
        return Vec::new();
    }
    order
        .into_iter()
        .map(|tid| ThreadReplyTarget {
            thread_id: tid.clone(),
            parent_id: parent_by_thread.remove(&tid).unwrap_or_default(),
        })
        .collect()
}

/// `buildChatPrompt` (prompt.go:451–603): interactive chat tasks.
fn build_chat_prompt(task: &Task) -> String {
    // Legacy compatibility for historical proactive-introduction sessions.
    if task.chat_intro {
        return "You are running as a chat assistant for a Cordy workspace.\nYou were just created, and this is the very first message in a direct chat with the person who created you. They have not written anything yet — you are opening the conversation. Send a short, warm, first-person introduction: who you are, what you're good at, and how they can work with you. Do NOT phrase it as an answer to a question or repeat any prompt back; just introduce yourself as if you reached out first.\n".to_string();
    }

    let mut b = String::new();
    b.push_str("You are running as a chat assistant for a Cordy workspace.\n");
    // Audience is per-session context, kept out of the cached runtime brief.
    match audience_of(&task.chat_channel_type, &task.chat_type) {
        ChatAudience::Group => {
            b.push_str("Audience: group room; not private; unseen members may read replies.\n\n");
        }
        ChatAudience::Unknown => {
            b.push_str("Audience: unknown.\n\n");
        }
        _ => {
            b.push_str("Audience: direct room.\n\n");
        }
    }
    // Channel awareness (MUL-3871): WHERE the conversation lives is
    // per-branch, not shared; the no-narration rule is emitted for every
    // channel type (GH #6006).
    if !task.chat_channel_type.is_empty() {
        let platform = channel_display_name(&task.chat_channel_type);
        let _ = write!(
            b,
            "You are operating inside a {platform} conversation — not the Cordy web app. Never look in Cordy issues or comments for this conversation.\n"
        );
        if task.chat_channel_type == CHANNEL_TYPE_SLACK {
            let _ = write!(
                b,
                "This conversation and its history live in {platform}, NOT in Cordy. The message below may be only what triggered you. Read the conversation with:\n"
            );
            b.push_str("- `cordy chat history --output json` — the channel overview: recent top-level messages, each thread tagged with a `thread_id` and `reply_count`. It does NOT expand thread contents.\n");
            b.push_str("- `cordy chat thread [<thread_id>] --output json` — read one thread's messages; omit the id to read the thread you are in, or pass a `thread_id` from the overview to read a specific thread.\n");
            if task.chat_in_thread {
                b.push_str("You were @mentioned inside a thread: start with `cordy chat thread` to read it; if you need the wider channel, run `cordy chat history` and open a specific thread with `cordy chat thread <thread_id>`.\n");
            } else {
                b.push_str("You were @mentioned at the channel top level: start with `cordy chat history` to see the channel, then read a specific thread's contents with `cordy chat thread <thread_id>`.\n");
            }
            // These reads are the agent's private context-gathering; narrating
            // them into a chat reply reads as noise.
            b.push_str("Do these reads SILENTLY as an internal step — they are how you gather context, not part of your answer.\n");
        } else if surface_persists_transcript(&task.chat_channel_type) {
            let _ = write!(
                b,
                "The conversation happens in {platform}, and Cordy stores a transcript of it. The message below may be only what triggered you — read it back with `cordy chat history` when you need earlier context that is not below.\n"
            );
        } else {
            let _ = write!(
                b,
                "This conversation and its history live in {platform}, NOT in Cordy, and Cordy has no history reader for it. Work from the context already provided to you below — no command can fetch more of this conversation. If you genuinely need earlier context that is not here, ask the user for it rather than guessing.\n"
            );
        }
        // Scoped to process, not results — a completion confirmation IS the deliverable.
        let _ = write!(
            b,
            "Reply to {platform} with the final outcome only. Do NOT narrate planned or in-progress steps (\"我先读取…\"); completed actions are part of the outcome.\n"
        );
        b.push('\n');
    }
    if task.agent.as_ref().is_some_and(|a| !a.skills.is_empty()) {
        let refs = extract_slash_skills(&task.chat_message);
        if !refs.is_empty() {
            let agent_skills: HashMap<&str, &str> = task
                .agent
                .as_ref()
                .unwrap()
                .skills
                .iter()
                .map(|s| (s.id.as_str(), s.name.as_str()))
                .collect();

            let mut selected: Vec<&str> = Vec::new();
            let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for r in &refs {
                let Some(name) = agent_skills.get(r.id.as_str()) else { continue };
                if !seen.insert(r.id.as_str()) {
                    continue;
                }
                selected.push(name);
            }

            if !selected.is_empty() {
                b.push_str("Explicitly selected skills:\n");
                for name in selected {
                    let _ = writeln!(b, "- {name}");
                }
                b.push('\n');
            }
        }
    }
    let _ = write!(b, "User message:\n{}\n", task.chat_message);
    // List attachments by id + filename; URLs are deliberately not inlined
    // (signed CDN with a short TTL).
    if !task.chat_message_attachments.is_empty() {
        b.push_str("\nAttachments on this message:\n");
        for a in &task.chat_message_attachments {
            if !a.content_type.is_empty() {
                let _ = writeln!(b, "- id={} filename={:?} content_type={}", a.id, a.filename, a.content_type);
            } else {
                let _ = writeln!(b, "- id={} filename={:?}", a.id, a.filename);
            }
        }
        b.push_str("Use `cordy attachment download <id>` to fetch each file locally before referring to it.\n");
        b.push_str("When creating an issue that should preserve one of these attachments, pass `--attachment-id <id>` to `cordy issue create` in addition to keeping the attachment markdown inline.\n");
    }
    // Outbound attachments: the DELIVERY layer of the channel policy has three
    // answers, not two (MUL-4899). This is the ONLY place the verdict is
    // stated; it must be re-emitted on every turn.
    if task.chat_channel_type.is_empty() {
        b.push_str("\nTo include a file or image you produced in your reply, run `cordy attachment upload <local-path>`. The file binds to your reply automatically and appears as an attachment card below it even if you paste nothing. The command also returns a `markdown` snippet you may paste on its own line to place the item where you want it (files render as a card, images inline).\n");
    } else if channel_carries_files(&task.chat_channel_type, task.chat_channel_delivers_files) {
        let _ = write!(
            b,
            "\nTo include a file or image you produced in your reply, run `cordy attachment upload <local-path>`. It binds to your reply and Cordy sends it into the {} conversation as a separate message right after your text — there is no way to place it inline, so write your reply to read correctly with the file arriving after it.\n",
            channel_display_name(&task.chat_channel_type)
        );
    } else {
        let _ = write!(
            b,
            "\nThis reply is delivered to {} as text. You cannot attach a file to it: `cordy attachment upload` binds to a Cordy chat reply, which this is not. If you produce a file, describe it in words — never write its local path as a link, and never upload it and then write as though it arrived.\n",
            channel_display_name(&task.chat_channel_type)
        );
    }
    b
}

/// `buildAutopilotPrompt` (prompt.go:613–649): run_only autopilot tasks.
fn build_autopilot_prompt(task: &Task) -> String {
    let mut b = String::new();
    b.push_str("You are running as a local coding agent for a Cordy workspace.\n\n");
    b.push_str("This task was triggered by an Autopilot in run-only mode. There is no assigned Cordy issue for this run.\n\n");
    let _ = write!(b, "Autopilot run ID: {}\n", task.autopilot_run_id);
    if !task.autopilot_id.is_empty() {
        let _ = write!(b, "Autopilot ID: {}\n", task.autopilot_id);
    }
    if !task.autopilot_title.is_empty() {
        let _ = write!(b, "Autopilot title: {}\n", task.autopilot_title);
    }
    if !task.autopilot_source.is_empty() {
        let _ = write!(b, "Trigger source: {}\n", task.autopilot_source);
    }
    let payload = task
        .autopilot_trigger_payload
        .as_ref()
        .map(|v| v.to_string())
        .unwrap_or_default();
    if !payload.trim().is_empty() {
        let _ = write!(b, "Trigger payload:\n{}\n", payload.trim());
    }
    b.push_str("\nAutopilot instructions:\n");
    if !task.autopilot_description.trim().is_empty() {
        b.push_str(&task.autopilot_description);
        b.push_str("\n\n");
    } else if !task.autopilot_title.is_empty() {
        let _ = write!(b, "{}\n\n", task.autopilot_title);
    } else {
        b.push_str("No additional autopilot instructions were provided. Inspect the autopilot configuration before proceeding.\n\n");
    }
    if !task.autopilot_id.is_empty() {
        let _ = write!(
            b,
            "Start by running `cordy autopilot get {} --output json` if you need the full autopilot configuration, then complete the instructions above.\n",
            task.autopilot_id
        );
    } else {
        b.push_str("Complete the instructions above.\n");
    }
    // The issue-command boundary is NOT restated here: the brief's autopilot
    // workflow section is its single emission point (MUL-5696).
    b
}

/// `squadBriefingMarker` (prompt.go:654): legacy role signal only.
const SQUAD_BRIEFING_MARKER: &str = "## Squad Operating Protocol";

/// `taskIsSquadLeader` (prompt.go:681–686): leadership is a PER-TASK role. A
/// current server says so explicitly (`leader_role_resolved` +
/// `is_leader_task`/`squad_id`); absent capability means the server never
/// authoritatively answered, and the legacy briefing-marker inference is the
/// only correct read (MUL-5811).
fn task_is_squad_leader(task: &Task) -> bool {
    if !task.leader_role_resolved {
        return task
            .agent
            .as_ref()
            .is_some_and(|a| a.instructions.contains(SQUAD_BRIEFING_MARKER));
    }
    task.is_leader_task || !task.squad_id.is_empty()
}

/// S9-integration seam stand-ins for the execenv prompt-section builders
/// (server/internal/daemon/execenv/{runtime_config_sections.go,
/// reply_instructions.go,runtime_config.go}). Byte-faithful mirrors; swap to
/// the execenv ports when those lanes land.
pub(crate) mod execenv_seams {
    /// `SessionContinuityNoticeIssue`
    /// (runtime_config_sections.go:433–435).
    pub const SESSION_CONTINUITY_NOTICE_ISSUE: &str = "## Session Continuity Notice\n\nThis run was meant to continue an earlier conversation, but that provider session could not be restored, so you are on a fresh one. The issue and its full comment history are unaffected — that record is the authoritative version of this conversation, and reading it (which your workflow already requires) reconstructs it. What is gone is only your own working memory from earlier turns: what you already tried, what you ruled out, and how far you had got. Re-derive what you need instead of assuming it, and do not claim continuity the record cannot back up. Do not open your reply by announcing this — raise it only where it actually matters, such as when the user refers to reasoning you never wrote down.\n\n";

    /// `SessionContinuityNoticeChannelHistory`
    /// (runtime_config_sections.go:437–439).
    pub const SESSION_CONTINUITY_NOTICE_CHANNEL_HISTORY: &str = "## Session Continuity Notice\n\nThis run was meant to continue an earlier conversation, but that provider session could not be restored, so you are on a fresh one. The channel conversation itself is unaffected — read it back with `cordy chat history` / `cordy chat thread` before acting, and treat what you find there as the authoritative version. What is gone is only your own working memory from earlier turns: what you already tried, what you ruled out, and how far you had got. Re-derive what you need instead of assuming it. Do not open your reply by announcing this — raise it only where it actually matters.\n\n";

    /// `SessionContinuityNoticeChatTranscript`
    /// (runtime_config_sections.go:441–443).
    pub const SESSION_CONTINUITY_NOTICE_CHAT_TRANSCRIPT: &str = "## Session Continuity Notice\n\nThis run was meant to continue an earlier conversation, but that provider session could not be restored, so you are on a fresh one. The conversation itself is unaffected — Cordy stored it, and you can read it back with `cordy chat history` before acting; treat what you find there as the authoritative version. What is gone is only your own working memory from earlier turns: what you already tried, what you ruled out, and how far you had got. Re-derive what you need instead of assuming it. Do not open your reply by announcing this — raise it only where it actually matters.\n\n";

    /// `SessionContinuityNoticeUnrecoverable`
    /// (runtime_config_sections.go:448–450).
    pub const SESSION_CONTINUITY_NOTICE_UNRECOVERABLE: &str = "## Session Continuity Notice\n\nThis run was meant to continue an earlier conversation, but that session's context could NOT be restored — you are starting fresh with no memory of the previous turns. That history is not readable from anywhere now: there is no command that fetches it, and only the context already in this message survives. **When you reply, tell the user up front (one short sentence) that the previous conversation context was unavailable and this is a new session**, so they understand why the thread did not carry over.\n\n";

    /// `sanitizeNameForBriefMarkdown` (runtime_config.go:70–93).
    fn sanitize_name_for_brief_markdown(name: &str) -> String {
        let mut b = String::with_capacity(name.len());
        let mut prev_space = false;
        for r in name.chars() {
            match r {
                '\r' | '\n' | '\t' | '\u{0b}' | '\u{0c}' => {
                    if !prev_space && !b.is_empty() {
                        b.push(' ');
                        prev_space = true;
                    }
                }
                r if (r as u32) < 0x20 || r == '\u{7f}' => {}
                '*' | '_' | '`' | '\\' | '[' | ']' | '<' => {
                    b.push('\\');
                    b.push(r);
                    prev_space = false;
                }
                r => {
                    b.push(r);
                    prev_space = false;
                }
            }
        }
        b.trim().to_string()
    }

    /// `sanitizeEmailForBrief` (runtime_config.go:103–114).
    fn sanitize_email_for_brief(email: &str) -> String {
        let email = email.trim();
        if email.is_empty() || !email.contains('@') {
            return String::new();
        }
        for r in email.chars() {
            if (r as u32) < 0x20 || r == '\u{7f}' || matches!(r, ' ' | '\\' | '`' | '*' | '<' | '>' | '[' | ']') {
                return String::new();
            }
        }
        email.to_string()
    }

    /// `sanitizeBriefCodeToken` (runtime_config_sections.go:230–242).
    fn sanitize_brief_code_token(s: &str) -> String {
        let s = s.trim();
        if s.is_empty() {
            return String::new();
        }
        for r in s.chars() {
            if !(r.is_ascii_lowercase()
                || r.is_ascii_uppercase()
                || r.is_ascii_digit()
                || r == '_'
                || r == '-'
                || r == '.')
            {
                return String::new();
            }
        }
        s.to_string()
    }

    /// `runtimeapps.DisplayNameForToolkitSlug`
    /// (internal/runtimeapps/connected_app.go:27–50).
    fn display_name_for_toolkit_slug(slug: &str) -> String {
        let slug = slug.trim();
        if slug.is_empty() {
            return String::new();
        }
        match slug {
            "github" => return "GitHub".to_string(),
            "gmail" => return "Gmail".to_string(),
            "linkedin" => return "LinkedIn".to_string(),
            _ => {}
        }
        slug.split(['_', '-'])
            .filter(|w| !w.is_empty())
            .map(|w| {
                let mut cs = w.chars();
                match cs.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + cs.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `BuildTaskInitiatorBlock`
    /// (runtime_config_sections.go:165–183). Returns "" when no initiator
    /// name resolves.
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
            let _ = std::fmt::Write::write_fmt(
                &mut b,
                format_args!(
                    "This task was initiated by **{safe_initiator}**, another agent in this workspace.\n\n"
                ),
            );
        } else {
            let email = sanitize_email_for_brief(initiator_email);
            if !email.is_empty() {
                let _ = std::fmt::Write::write_fmt(
                    &mut b,
                    format_args!(
                        "This task was initiated by **{safe_initiator}** ({email}), a member of this workspace.\n\n"
                    ),
                );
            } else {
                let _ = std::fmt::Write::write_fmt(
                    &mut b,
                    format_args!(
                        "This task was initiated by **{safe_initiator}**, a member of this workspace.\n\n"
                    ),
                );
            }
        }
        b.push_str("The initiator — not the runtime owner — is who you are answering: apply any per-person privacy or access rules your instructions define. Your Cordy credentials stay scoped to the runtime owner, and initiator attribution does not change what you may read or write; do not assume the initiator can see everything you can.\n\n");
        b
    }

    /// `BuildConnectedAppsBlock`
    /// (runtime_config_sections.go:200–228). Returns "" when no app resolves.
    pub(crate) fn build_connected_apps_block(apps: &[crate::types::ConnectedAppData]) -> String {
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
                name = sanitize_name_for_brief_markdown(&display_name_for_toolkit_slug(&toolkit_slug));
            }
            if name.is_empty() {
                name = toolkit_slug.clone();
            }
            let _ = std::fmt::Write::write_fmt(
                &mut lines,
                format_args!("- {name} (`{toolkit_slug}`) via MCP server `{server_name}`\n"),
            );
        }
        if lines.is_empty() {
            return String::new();
        }
        let mut b = String::new();
        b.push_str("## Connected Apps\n\n");
        b.push_str(&lines);
        b.push_str("\nUse the listed MCP server when the task asks to read or act in one of these apps.\n\n");
        b
    }

    /// `activeThreadID` (reply_instructions.go:114–119).
    fn active_thread_id(trigger_thread_id: &str, trigger_comment_id: &str) -> String {
        if !trigger_thread_id.is_empty() {
            trigger_thread_id.to_string()
        } else {
            trigger_comment_id.to_string()
        }
    }

    /// `BuildNewCommentsHint` (reply_instructions.go:24–53): warm-path
    /// comment-reading pointer; renders nothing on cold start, zero delta, or
    /// empty issue id.
    pub(crate) fn build_new_comments_hint(
        issue_id: &str,
        trigger_comment_id: &str,
        trigger_thread_id: &str,
        new_comments_since: &str,
        new_comment_count: i64,
    ) -> String {
        if new_comment_count <= 0 || new_comments_since.is_empty() || issue_id.is_empty() {
            return String::new();
        }
        let thread_id = active_thread_id(trigger_thread_id, trigger_comment_id);
        if !thread_id.is_empty() {
            return format!(
                "{new_comment_count} new comment(s) on this issue since your last run — don't read them all blindly. Start with the thread your triggering comment is in: `cordy issue comment list {issue_id} --thread {thread_id} --since {new_comments_since} --compact --output json` (swap `--since` for `--tail 30` if you need the full thread, not just the delta). Only if you need context from the other threads, rerun it without `--thread` for the issue-wide catch-up.\n\n"
            );
        }
        format!(
            "{new_comment_count} new comment(s) on this issue since your last run. Catch up: `cordy issue comment list {issue_id} --since {new_comments_since} --compact --output json`.\n\n"
        )
    }

    /// `BuildResumedCommentsHint` (reply_instructions.go:63–79): warm
    /// no-delta path.
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
            "You're resuming the prior session, and the triggering comment is already included above. No other new comments on this issue since your last run. If your reply depends on thread context, do not rely only on resumed session memory — first pull the triggering conversation with: `cordy issue comment list {issue_id} --thread {thread_id} --tail 30 --compact --output json`.\n\n"
        )
    }

    /// `BuildColdCommentsHint` (reply_instructions.go:96–112): cold path;
    /// returns "" when there is no triggering comment to thread from.
    pub(crate) fn build_cold_comments_hint(
        issue_id: &str,
        trigger_comment_id: &str,
        trigger_thread_id: &str,
    ) -> String {
        let thread_id = active_thread_id(trigger_thread_id, trigger_comment_id);
        if issue_id.is_empty() || thread_id.is_empty() {
            return String::new();
        }
        format!(
            "Read the triggering conversation first: `cordy issue comment list {issue_id} --thread {thread_id} --tail 30 --compact --output json` (that thread's root + its 30 newest replies). Need cross-thread background? Rerun with `--roots-only --summary` replacing `--thread ... --tail 30` to scan the other threads cheaply, and expand only what looks relevant.\n\n"
        )
    }

    /// `BuildCommentReplyInstructions` →
    /// `buildCommentReplyInstructionsSlim` (reply_instructions.go:165–214):
    /// the canonical block telling an agent how to post its reply. Provider is
    /// retained for caller symmetry; the guardrail is identical across
    /// providers and hosts.
    pub(crate) fn build_comment_reply_instructions(
        provider: &str,
        issue_id: &str,
        trigger_comment_id: &str,
        squad_leader: bool,
    ) -> String {
        let _ = provider;
        if trigger_comment_id.is_empty() {
            return String::new();
        }
        let lead = if squad_leader {
            "Unless your outcome is `no_action`, post your reply as a comment — always use the trigger comment ID below, "
        } else {
            "Post your reply as a comment — always use the trigger comment ID below, "
        };
        if cfg!(windows) {
            format!(
                "{lead}do NOT reuse --parent values from previous turns in this session.\n\nWrite the body file first — never pipe via `--content-stdin` (PowerShell drops non-ASCII; full rules: ## Comment Formatting above):\n\n    cordy issue comment add {issue_id} --parent {trigger_comment_id} --content-file ./reply.md\n    Remove-Item ./reply.md\n\nDo NOT write literal `\\n` escapes to simulate line breaks; the file preserves real newlines.\n"
            )
        } else {
            format!(
                "{lead}do NOT reuse --parent values from previous turns in this session.\n\nWrite the body file first (rules: ## Comment Formatting above — MUL-2904 / #4182):\n\n    cordy issue comment add {issue_id} --parent {trigger_comment_id} --content-file ./reply.md\n    rm ./reply.md\n\nDo NOT write literal `\\n` escapes to simulate line breaks; the file preserves real newlines.\n"
            )
        }
    }

    /// `BuildMultiThreadCommentReplyInstructions`
    /// (reply_instructions.go:251–279): reply fan-out for runs whose coalesced
    /// comments span more than one root thread (MUL-4348). Returns "" for
    /// fewer than two targets.
    pub(crate) fn build_multi_thread_comment_reply_instructions(
        issue_id: &str,
        targets: &[crate::execenv::execenv::ThreadReplyTarget],
        squad_leader: bool,
    ) -> String {
        if issue_id.is_empty() || targets.len() < 2 {
            return String::new();
        }
        let mut target_lines = String::new();
        for (i, tgt) in targets.iter().enumerate() {
            let _ = std::fmt::Write::write_fmt(
                &mut target_lines,
                format_args!(
                    "{}. thread {} → reply with `--parent {}`\n",
                    i + 1,
                    tgt.thread_id,
                    tgt.parent_id
                ),
            );
        }
        let lead = if squad_leader {
            format!(
                "This run coalesced comments from {} DISTINCT threads. **If your outcome is `no_action`, skip this ENTIRE fan-out block — post no replies at all and exit via `cordy squad activity` as your leader rules direct; everything below applies only otherwise.** Otherwise, post ONE reply per thread",
                targets.len()
            )
        } else {
            format!(
                "This run coalesced comments from {} DISTINCT threads. Post ONE reply per thread",
                targets.len()
            )
        };
        format!(
            "{lead} — {count} in total. This OVERRIDES the \"post exactly one comment per run\" rule: for THIS run multiple replies are required and correct. Do NOT merge separate threads into one comment or post twice in the same thread.\n\nReply targets, in posting order — OLDEST thread first, the newest (triggering) thread LAST. Use the exact `--parent` for each; never reuse a `--parent` from an earlier turn:\n{target_lines}\nWrite and post each reply exactly as `## Comment Formatting` above directs, with ONE multi-thread delta: use a DISTINCT body file per thread (./reply-1.md, ./reply-2.md, …) so one reply's content can never leak into another's.\n",
            count = targets.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentData, ChatAttachmentMeta, CoalescedCommentData, SkillData};

    fn task(fields: impl FnOnce(&mut Task)) -> Task {
        let mut t = Task::default();
        fields(&mut t);
        t
    }

    /// TestBuildQuickCreatePromptRules (prompt_test.go:19–65).
    #[test]
    fn quick_create_prompt_rules() {
        let out = build_quick_create_prompt(&task(|t| {
            t.quick_create_prompt = "fix the login button color".into();
        }));
        for s in [
            "Faithfully restate what the user wants",
            "Preserve specific names, identifiers, file paths",
            "verbal routing wrappers about creating the issue",
            "pure conversational fillers",
            "CC exception",
            "auto-subscribes members",
            "include ONLY when the input cited external resources",
            "never use it as an apology log",
            "cordy issue create --output json",
            "JSON response",
            "identifier",
            "Do not scrape human output",
            "do not assume any workspace issue prefix",
            "Created <identifier-or-id>: <title>",
            "never invent requirements",
            "never reduce multi-sentence input",
            "`--attachment` takes LOCAL file paths, never URLs",
            "Files you produced: see `## Output`",
        ] {
            assert!(out.contains(s), "quick-create output missing required rule: {s:?}\n---\n{out}");
        }
        assert!(
            !out.contains("do NOT pass `--attachment`"),
            "unconditional --attachment ban conflicts with ## Output (MUL-5696)\n{out}"
        );
    }

    /// TestBuildQuickCreatePromptAssigneeIncludesSquads
    /// (prompt_test.go:72–86).
    #[test]
    fn quick_create_assignee_includes_squads() {
        let out = build_quick_create_prompt(&task(|t| {
            t.quick_create_prompt = "fix the login button color".into();
        }));
        for s in [
            "cordy squad list",
            "Squads are first-class assignees",
            "Treat bare @-routing as an assignee directive",
            "让 @独立团 review 这个 PR",
            "pass the squad's `id` as `--assignee-id`",
        ] {
            assert!(out.contains(s), "assignee block missing {s:?}\n---\n{out}");
        }
    }

    /// TestBuildQuickCreatePromptSquadDefaultsToSquad
    /// (prompt_test.go:94–132).
    #[test]
    fn quick_create_squad_defaults_to_squad() {
        const SQUAD_ID: &str = "aaaa1111-2222-3333-4444-555555555555";
        const SQUAD_NAME: &str = "独立团";
        const LEADER_ID: &str = "bbbb1111-2222-3333-4444-666666666666";
        let out = build_quick_create_prompt(&task(|t| {
            t.quick_create_prompt = "fix the login button color".into();
            t.agent = Some(AgentData { id: LEADER_ID.into(), name: "leader-agent".into(), ..Default::default() });
            t.squad_id = SQUAD_ID.into();
            t.squad_name = SQUAD_NAME.into();
        }));
        assert!(out.contains(&format!("--assignee-id \"{SQUAD_ID}\"")));
        assert!(!out.contains(&format!("--assignee-id \"{LEADER_ID}\"")));
        assert!(out.contains(SQUAD_NAME));
        for s in ["picker SQUAD", "running on the squad's behalf", "do not assign it to your own agent UUID"] {
            assert!(out.contains(s), "missing {s:?}\n---\n{out}");
        }
    }

    /// TestBuildQuickCreatePromptProjectPinning (prompt_test.go:140–168).
    #[test]
    fn quick_create_project_pinning() {
        const PROJECT_ID: &str = "11111111-2222-3333-4444-555555555555";
        let out = build_quick_create_prompt(&task(|t| {
            t.quick_create_prompt = "fix the login button color".into();
            t.project_id = PROJECT_ID.into();
            t.project_title = "Web App".into();
        }));
        for s in [format!("--project \"{PROJECT_ID}\""), "Web App".into(), "modal selection is authoritative".into()] {
            assert!(out.contains(s.as_str()), "missing {s:?}\n---\n{out}");
        }
        let plain = build_quick_create_prompt(&task(|t| {
            t.quick_create_prompt = "fix the login button color".into();
        }));
        assert!(plain.contains("**project**: omit"));
        assert!(!plain.contains("--project"));
    }

    /// TestBuildQuickCreatePromptExplicitPriorityAndDueDate
    /// (prompt_test.go:170–188).
    #[test]
    fn quick_create_explicit_priority_and_due_date() {
        let out = build_quick_create_prompt(&task(|t| {
            t.quick_create_prompt = "fix the login button color".into();
            t.quick_create_priority = "urgent".into();
            t.quick_create_due_date = "2026-08-01".into();
        }));
        for s in ["--priority urgent", "--due-date 2026-08-01", "quick-create selection is authoritative"] {
            assert!(out.contains(s), "missing {s:?}\n---\n{out}");
        }
        assert!(!out.contains("Map P0/P1"), "explicit priority must replace inference rules\n{out}");
    }

    /// TestBuildQuickCreatePromptParentPinning (prompt_test.go:198–237).
    #[test]
    fn quick_create_parent_pinning() {
        const PARENT_ID: &str = "33333333-2222-1111-4444-555555555555";
        const PARENT_IDENTIFIER: &str = "MUL-2534";
        let out = build_quick_create_prompt(&task(|t| {
            t.quick_create_prompt = "fix the login button color".into();
            t.parent_issue_id = PARENT_ID.into();
            t.parent_issue_identifier = PARENT_IDENTIFIER.into();
        }));
        for s in [
            format!("--parent \"{PARENT_ID}\""),
            PARENT_IDENTIFIER.to_string(),
            "modal entry point is authoritative".to_string(),
            "filed as a sub-issue".to_string(),
        ] {
            assert!(out.contains(s.as_str()), "missing {s:?}\n---\n{out}");
        }
        let uuid_only = build_quick_create_prompt(&task(|t| {
            t.quick_create_prompt = "fix the login button color".into();
            t.parent_issue_id = PARENT_ID.into();
        }));
        assert!(uuid_only.contains(&format!("--parent \"{PARENT_ID}\"")));
        let plain = build_quick_create_prompt(&task(|t| {
            t.quick_create_prompt = "fix the login button color".into();
        }));
        assert!(!plain.contains("--parent"));
    }

    /// TestBuildPromptSquadLeaderNoActionForMemberTrigger
    /// (prompt_test.go:246–266).
    #[test]
    fn squad_leader_no_action_for_member_trigger() {
        let out = build_prompt(
            &task(|t| {
                t.issue_id = "issue-123".into();
                t.trigger_comment_id = "comment-456".into();
                t.trigger_comment_content = "LGTM".into();
                t.trigger_author_type = "member".into();
                t.trigger_author_name = "Bohan".into();
                t.is_leader_task = true;
                t.leader_role_resolved = true;
                t.agent = Some(AgentData {
                    instructions: "Some instructions\n\n## Squad Operating Protocol\n\nYou are the LEADER...".into(),
                    ..Default::default()
                });
            }),
            "claude",
        );
        assert!(out.contains("Squad leader no_action rule"), "{out}");
        assert!(out.contains("DO NOT post any comment"), "{out}");
    }

    /// TestTaskIsSquadLeaderReadsProtocolFields
    /// (prompt_test.go:302–375).
    #[test]
    fn task_is_squad_leader_reads_protocol_fields() {
        const BRIEFED: &str = "Some instructions\n\n## Squad Operating Protocol\n\nYou are the LEADER...";
        const PLAIN: &str = "You are a regular agent.";
        let cases: Vec<(String, Task, bool)> = vec![
            (
                "current: issue-bound leader task".into(),
                task(|t| { t.leader_role_resolved = true; t.is_leader_task = true; t.agent = Some(AgentData { instructions: BRIEFED.into(), ..Default::default() }); }),
                true,
            ),
            (
                "current: quick-create routed through a squad picker".into(),
                task(|t| { t.leader_role_resolved = true; t.squad_id = "5f7f7c12-b579-4c6d-aaa0-8ae1d7e72b61".into(); t.agent = Some(AgentData { instructions: BRIEFED.into(), ..Default::default() }); }),
                true,
            ),
            (
                "current: leader flag without agent payload".into(),
                task(|t| { t.leader_role_resolved = true; t.is_leader_task = true; }),
                true,
            ),
            (
                "current: ordinary agent whose own instructions carry the protocol heading".into(),
                task(|t| { t.leader_role_resolved = true; t.agent = Some(AgentData { instructions: BRIEFED.into(), ..Default::default() }); }),
                false,
            ),
            (
                "current: withheld briefing leaves no leader signal".into(),
                task(|t| { t.leader_role_resolved = true; t.agent = Some(AgentData { instructions: PLAIN.into(), ..Default::default() }); }),
                false,
            ),
            (
                "legacy: real leader recognised by the injected briefing".into(),
                task(|t| { t.agent = Some(AgentData { instructions: BRIEFED.into(), ..Default::default() }); }),
                true,
            ),
            (
                "legacy: ordinary agent".into(),
                task(|t| { t.agent = Some(AgentData { instructions: PLAIN.into(), ..Default::default() }); }),
                false,
            ),
            (
                "legacy: leader flag but briefing withheld".into(),
                task(|t| { t.is_leader_task = true; t.agent = Some(AgentData { instructions: PLAIN.into(), ..Default::default() }); }),
                false,
            ),
        ];
        for (name, t, want) in cases {
            assert_eq!(task_is_squad_leader(&t), want, "case: {name}");
        }
    }

    /// TestBuildPromptProtocolHeadingInInstructionsIsNotALeader
    /// (prompt_test.go:383–412).
    #[test]
    fn protocol_heading_in_instructions_is_not_a_leader() {
        let out = build_prompt(
            &task(|t| {
                t.issue_id = "issue-123".into();
                t.trigger_comment_id = "comment-456".into();
                t.trigger_comment_content = "please take a look".into();
                t.trigger_author_type = "member".into();
                t.trigger_author_name = "Bohan".into();
                t.leader_role_resolved = true;
                t.agent = Some(AgentData {
                    name: "Docs writer".into(),
                    instructions: "I document squads.\n\n## Squad Operating Protocol\n\nHow leaders dispatch work...".into(),
                    ..Default::default()
                });
            }),
            "claude",
        );
        for banned in ["Squad leader no_action rule", "cordy squad activity", "DO NOT post any comment", "Unless your outcome is `no_action`"] {
            assert!(!out.contains(banned), "leaked squad-leader rule {banned:?}\n---\n{out}");
        }
        assert!(out.contains("Post your reply as a comment"), "{out}");
    }

    /// TestBuildPromptLegacyServerKeepsBriefingBasedLeaderRole
    /// (prompt_test.go:420–446).
    #[test]
    fn legacy_server_keeps_briefing_based_leader_role() {
        let out = build_prompt(
            &task(|t| {
                t.issue_id = "issue-123".into();
                t.trigger_comment_id = "comment-456".into();
                t.trigger_comment_content = "LGTM".into();
                t.trigger_author_type = "member".into();
                t.trigger_author_name = "Bohan".into();
                t.agent = Some(AgentData {
                    name: "Lead".into(),
                    instructions: "You lead the team.\n\n## Squad Operating Protocol\n\nYou are the LEADER...".into(),
                    ..Default::default()
                });
            }),
            "claude",
        );
        for want in ["Squad leader no_action rule", "cordy squad activity", "DO NOT post any comment"] {
            assert!(out.contains(want), "lost {want:?}\n---\n{out}");
        }
    }

    /// TestBuildChatPromptAttachmentIDsCanBeBoundToCreatedIssues
    /// (prompt_test.go:448–467).
    #[test]
    fn chat_attachment_ids_bindable_to_created_issues() {
        let out = build_chat_prompt(&task(|t| {
            t.chat_session_id = "sess-1".into();
            t.chat_message = "please create an issue with this screenshot".into();
            t.chat_message_attachments = vec![ChatAttachmentMeta {
                id: "019ec09d-6222-722b-bdfa-427b105d80be".into(),
                filename: "shot.png".into(),
                content_type: "image/png".into(),
            }];
        }));
        for want in [
            "Attachments on this message:",
            "id=019ec09d-6222-722b-bdfa-427b105d80be",
            "cordy attachment download <id>",
            "--attachment-id <id>",
        ] {
            assert!(out.contains(want), "missing {want:?}\n---\n{out}");
        }
    }

    /// TestBuildChatPromptChannelAwareness (prompt_test.go:469–495+).
    #[test]
    fn chat_channel_awareness() {
        let slack = build_chat_prompt(&task(|t| {
            t.chat_session_id = "sess-1".into();
            t.chat_channel_type = "slack".into();
            t.chat_message = "你刚刚和 xxx 聊了什么".into();
        }));
        for want in ["Slack", "NOT in Cordy", "cordy chat history", "cordy chat thread", "Do NOT narrate"] {
            assert!(slack.contains(want), "slack prompt missing {want:?}\n---\n{slack}");
        }
        let top = build_chat_prompt(&task(|t| {
            t.chat_session_id = "s".into();
            t.chat_channel_type = "slack".into();
            t.chat_in_thread = false;
            t.chat_message = "hi".into();
        }));
        assert!(top.contains("top level: start with `cordy chat history`"), "{top}");
        let in_thread = build_chat_prompt(&task(|t| {
            t.chat_session_id = "s".into();
            t.chat_channel_type = "slack".into();
            t.chat_in_thread = true;
            t.chat_message = "hi".into();
        }));
        assert!(in_thread.contains("inside a thread: start with `cordy chat thread`"), "{in_thread}");
    }

    /// TestBuildChatPromptSlashSkills (prompt_test.go:844–947).
    #[test]
    fn chat_slash_skills() {
        // injects selected skills block
        let out = build_chat_prompt(&task(|t| {
            t.chat_session_id = "sess-1".into();
            t.chat_message = "please [/deploy](slash://skill/abc-123) this".into();
            t.agent = Some(AgentData {
                skills: vec![SkillData { id: "abc-123".into(), name: "deploy".into(), ..Default::default() }],
                ..Default::default()
            });
        }));
        assert!(out.contains("Explicitly selected skills:\n- deploy\n"), "{out}");
        assert!(out.contains("User message:\nplease [/deploy](slash://skill/abc-123) this"), "{out}");

        // ignores skills not belonging to agent / validates by ID not label
        for msg in ["[/hacker-skill](slash://skill/evil-id)", "[/deploy](slash://skill/wrong-id)"] {
            let out = build_chat_prompt(&task(|t| {
                t.chat_session_id = "sess-1".into();
                t.chat_message = msg.into();
                t.agent = Some(AgentData {
                    skills: vec![SkillData { id: "good-id".into(), name: "deploy".into(), ..Default::default() }],
                    ..Default::default()
                });
            }));
            assert!(!out.contains("Explicitly selected skills"), "msg {msg:?}\n{out}");
        }

        // uses canonical name not label
        let out = build_chat_prompt(&task(|t| {
            t.chat_session_id = "sess-1".into();
            t.chat_message = "[/spoofed-name](slash://skill/real-id)".into();
            t.agent = Some(AgentData {
                skills: vec![SkillData { id: "real-id".into(), name: "deploy".into(), ..Default::default() }],
                ..Default::default()
            });
        }));
        assert!(out.contains("- deploy\n"), "{out}");
        assert!(!out.contains("- spoofed-name\n"), "{out}");

        // deduplicates skills
        let out = build_chat_prompt(&task(|t| {
            t.chat_session_id = "sess-1".into();
            t.chat_message = "[/deploy](slash://skill/a) and [/deploy](slash://skill/a) again".into();
            t.agent = Some(AgentData {
                skills: vec![SkillData { id: "a".into(), name: "deploy".into(), ..Default::default() }],
                ..Default::default()
            });
        }));
        assert_eq!(out.matches("- deploy").count(), 1, "{out}");

        // omits block when no slash links or agent has no skills
        for make: fn(&mut Task) = |t| {
            t.chat_session_id = "sess-1".into();
            t.chat_message = "just a normal message".into();
            t.agent = Some(AgentData {
                skills: vec![SkillData { id: "a".into(), name: "deploy".into(), ..Default::default() }],
                ..Default::default()
            });
        } {
            let out = build_chat_prompt(&task(make));
            assert!(!out.contains("Explicitly selected skills"), "{out}");
        }
        let out = build_chat_prompt(&task(|t| {
            t.chat_session_id = "sess-1".into();
            t.chat_message = "[/deploy](slash://skill/abc-123)".into();
            t.agent = Some(AgentData::default());
        }));
        assert!(!out.contains("Explicitly selected skills"), "{out}");
    }

    /// TestBuildPromptDefaultScansRootsFirst (prompt_test.go:954–993).
    #[test]
    fn default_scans_roots_first() {
        let out = build_prompt(&task(|t| t.issue_id = "issue-default-1".into()), "claude");
        for s in [
            "cordy issue comment list issue-default-1 --roots-only --summary --compact --output json",
            "--since",
        ] {
            assert!(out.contains(s), "missing {s:?}\n---\n{out}");
        }
        assert!(!out.contains("--recent"), "{out}");
        assert!(!out.contains("Next thread cursor:"), "{out}");
        for seg in out.split("--thread").skip(1) {
            assert!(seg.starts_with(" <thread-id>"), "concrete anchor leaked\n{out}");
        }
        assert!(!out.contains("If you need comment history"), "{out}");
        assert!(!out.contains("cordy issue comment list issue-default-1 --output json"), "{out}");
    }

    /// TestBuildPromptWarnsAboutActiveSiblingRuns +
    /// TestBuildPromptOmitsActiveSiblingRunsForChatTask
    /// (prompt_test.go:995–1042).
    #[test]
    fn warns_about_active_sibling_runs() {
        let sibling = ActiveSiblingRunData {
            task_id: "task-existing".into(),
            issue_id: "issue-source".into(),
            issue_identifier: "MUL-6000".into(),
            issue_title: "Existing work".into(),
            status: "running".into(),
            started_at: "2026-08-14T03:00:00Z".into(),
        };
        let out = build_prompt(
            &task(|t| {
                t.issue_id = "issue-target".into();
                t.active_sibling_runs = vec![sibling.clone()];
            }),
            "claude",
        );
        for want in [
            "Active sibling runs",
            "MUL-6000",
            "task-existing",
            "cordy issue comment list issue-target --roots-only --summary --compact --output json",
            "cordy issue run-messages task-existing",
            "--no-start",
        ] {
            assert!(out.contains(want), "missing {want:?}\n---\n{out}");
        }
        assert!(!out.contains("cordy issue runs"), "{out}");
        assert!(!out.contains("run-messages task-existing --issue"), "{out}");

        let chat_out = build_prompt(
            &task(|t| {
                t.chat_session_id = "chat-1".into();
                t.active_sibling_runs = vec![ActiveSiblingRunData {
                    task_id: "task-existing".into(),
                    issue_id: "issue-source".into(),
                    issue_identifier: "MUL-6000".into(),
                    status: "running".into(),
                    ..Default::default()
                }];
            }),
            "claude",
        );
        assert!(!chat_out.contains("Active sibling runs") && !chat_out.contains("task-existing"), "{chat_out}");
    }

    /// TestBuildPromptNonSquadLeaderNoRule (prompt_test.go:1046–1061).
    #[test]
    fn non_squad_leader_no_rule() {
        let out = build_prompt(
            &task(|t| {
                t.issue_id = "issue-123".into();
                t.trigger_comment_id = "comment-456".into();
                t.trigger_comment_content = "LGTM".into();
                t.trigger_author_type = "member".into();
                t.trigger_author_name = "Bohan".into();
                t.agent = Some(AgentData {
                    instructions: "Some instructions without the squad marker".into(),
                    ..Default::default()
                });
            }),
            "claude",
        );
        assert!(!out.contains("Squad leader no_action rule"), "{out}");
    }

    /// TestBuildPromptNewCommentsHint (prompt_test.go:1068–1112).
    #[test]
    fn new_comments_hint() {
        const ISSUE_ID: &str = "issue-new-1";
        const SINCE: &str = "2026-05-28T11:00:00Z";
        let out = build_prompt(
            &task(|t| {
                t.issue_id = ISSUE_ID.into();
                t.trigger_comment_id = "trigger-1".into();
                t.trigger_thread_id = "thread-root-1".into();
                t.trigger_comment_content = "please look".into();
                t.trigger_author_type = "member".into();
                t.new_comment_count = 3;
                t.new_comments_since = SINCE.into();
            }),
            "claude",
        );
        assert!(out.contains("3 new comment(s) on this issue since your last run"), "{out}");
        assert!(out.contains("blindly"), "{out}");
        assert!(
            out.contains(&format!("cordy issue comment list {ISSUE_ID} --thread thread-root-1 --since {SINCE} --compact --output json")),
            "{out}"
        );
        assert!(out.contains("--tail 30"), "{out}");
        assert!(out.contains("rerun it without `--thread` for the issue-wide catch-up"), "{out}");
        assert!(
            !out.contains(&format!("cordy issue comment list {ISSUE_ID} --since {SINCE} --output json")),
            "{out}"
        );
        assert!(!out.contains("Next reply cursor") && !out.contains("--before-id"), "{out}");
    }

    /// TestBuildPromptColdStartThreadRead (prompt_test.go:1118–1151).
    #[test]
    fn cold_start_thread_read() {
        const ISSUE_ID: &str = "issue-cold-1";
        let out = build_prompt(
            &task(|t| {
                t.issue_id = ISSUE_ID.into();
                t.trigger_comment_id = "trigger-1".into();
                t.trigger_thread_id = "thread-root-1".into();
                t.trigger_comment_content = "hi".into();
                t.trigger_author_type = "member".into();
            }),
            "claude",
        );
        assert!(!out.contains("new comment(s) since your last run"), "{out}");
        assert!(
            out.contains(&format!("cordy issue comment list {ISSUE_ID} --thread thread-root-1 --tail 30 --compact --output json")),
            "{out}"
        );
        assert!(out.contains("Rerun with `--roots-only --summary` replacing `--thread ... --tail 30`"), "{out}");
        assert!(
            !out.contains(&format!("cordy issue comment list {ISSUE_ID} --roots-only --summary --output json")),
            "{out}"
        );
        assert!(!out.contains("--recent"), "{out}");
    }

    /// TestBuildPromptResumedNoDeltaDoesNotForceThreadRead
    /// (prompt_test.go:1157–1195).
    #[test]
    fn resumed_no_delta_does_not_force_thread_read() {
        const ISSUE_ID: &str = "issue-resumed-1";
        let out = build_prompt(
            &task(|t| {
                t.issue_id = ISSUE_ID.into();
                t.trigger_comment_id = "trigger-1".into();
                t.trigger_thread_id = "thread-root-1".into();
                t.trigger_comment_content = "hi again".into();
                t.trigger_author_type = "member".into();
                t.prior_session_id = "session-123".into();
            }),
            "claude",
        );
        for want in [
            "triggering comment is already included above",
            "No other new comments on this issue since your last run",
            "If your reply depends on thread context",
            "do not rely only on resumed session memory",
            &format!("cordy issue comment list {ISSUE_ID} --thread thread-root-1 --tail 30 --compact --output json"),
        ] {
            assert!(out.contains(want), "missing {want:?}\n---\n{out}");
        }
        assert!(!out.contains("active thread anchor"), "{out}");
        assert!(!out.contains("scoped to the triggering thread"), "{out}");
        assert!(!out.contains("Read the triggering conversation first"), "{out}");
    }

    /// TestBuildCommentPromptCoalescedCrossThread
    /// (prompt_test.go:1204–1243).
    #[test]
    fn coalesced_cross_thread() {
        let out = build_prompt(
            &task(|t| {
                t.issue_id = "issue-xthread-1".into();
                t.trigger_comment_id = "trigger-newest".into();
                t.trigger_thread_id = "thread-root-A".into();
                t.trigger_comment_content = "latest instruction".into();
                t.trigger_author_type = "member".into();
                t.coalesced_comment_ids = vec!["c-old-1".into(), "c-old-2".into()];
                t.coalesced_comments = vec![
                    CoalescedCommentData {
                        id: "c-old-1".into(),
                        thread_id: "thread-root-A".into(),
                        author_type: "member".into(),
                        author_name: "Alice".into(),
                        content: "first earlier comment".into(),
                        created_at: "2026-07-08T01:00:00Z".into(),
                    },
                    CoalescedCommentData {
                        id: "c-old-2".into(),
                        thread_id: "thread-root-B".into(),
                        author_type: "member".into(),
                        author_name: "Bob".into(),
                        content: "comment in a different thread".into(),
                        created_at: "2026-07-08T02:00:00Z".into(),
                    },
                ];
            }),
            "claude",
        );
        assert!(!out.contains("they are in the triggering thread"), "{out}");
        for want in ["first earlier comment", "comment in a different thread", "thread-root-A", "thread-root-B", "c-old-1", "c-old-2"] {
            assert!(out.contains(want), "missing {want:?}\n---\n{out}");
        }
    }

    /// TestBuildCommentPromptLabelsDelegatedFailureSignalAsPlatform
    /// (prompt_test.go:1245–1259).
    #[test]
    fn delegated_failure_signal_labeled_as_platform() {
        let out = build_prompt(
            &task(|t| {
                t.issue_id = "issue-recovery-1".into();
                t.trigger_comment_id = "recovery-comment-1".into();
                t.trigger_comment_content = "Delegated task failed; resume coordination.".into();
                t.trigger_author_type = "system".into();
            }),
            "codex",
        );
        assert!(out.contains("[NEW COMMENT] The platform just left a new comment"), "{out}");
        assert!(!out.contains("[NEW COMMENT] A user just left a new comment"), "{out}");
    }

    /// TestBuildCommentPromptCoalescedIDsOnlyFallback
    /// (prompt_test.go:1269–1352).
    #[test]
    fn coalesced_ids_only_fallback() {
        let base = |t: &mut Task| {
            t.issue_id = "issue-fallback-1".into();
            t.trigger_comment_id = "trigger-newest".into();
            t.trigger_thread_id = "thread-root-A".into();
            t.trigger_comment_content = "latest instruction".into();
            t.trigger_author_type = "member".into();
            t.coalesced_comment_ids = vec!["c-old-1".into(), "c-old-2".into()];
        };

        let anchored = build_prompt(&task(|t| {
            base(t);
            t.new_comments_since = "2026-08-03T06:00:00Z".into();
        }), "claude");
        assert!(
            anchored.contains("cordy issue comment list issue-fallback-1 --since 2026-08-03T06:00:00Z --compact --output json"),
            "{anchored}"
        );
        for want in ["candidate window, not a guarantee", "can carry ids older than the window"] {
            assert!(anchored.contains(want), "missing {want:?}\n{anchored}");
        }
        assert_bounded_id_only_fallback(&anchored);

        let anchorless = build_prompt(&task(base), "claude");
        assert!(!anchorless.contains("--since"), "{anchorless}");
        assert!(!anchorless.contains("last_activity_at"), "{anchorless}");
        assert_bounded_id_only_fallback(&anchorless);
    }

    /// assertBoundedIDOnlyFallback (prompt_test.go:1325–1352).
    fn assert_bounded_id_only_fallback(out: &str) {
        assert!(!out.contains("they are in the triggering thread"), "{out}");
        assert!(!out.contains("--recent"), "{out}");
        for want in [
            "cordy issue comment list issue-fallback-1 --thread <comment-id> --tail 30 --compact --output json",
            "accepts a reply id",
            "Next reply cursor",
            "--before-id",
            "Do not finish this turn until every id above is accounted for",
        ] {
            assert!(out.contains(want), "missing {want:?}\n---\n{out}");
        }
        for id in ["c-old-1", "c-old-2"] {
            assert!(out.contains(id), "missing id {id}\n{out}");
        }
    }

    /// TestCommentReplyThreadsGrouping (prompt_test.go:1361–1456).
    #[test]
    fn comment_reply_threads_grouping() {
        // three distinct root threads fan out
        let targets = comment_reply_threads(&task(|t| {
            t.trigger_comment_id = "c3".into();
            t.trigger_thread_id = "c3".into();
            t.coalesced_comments = vec![
                CoalescedCommentData { id: "c1".into(), thread_id: "c1".into(), content: "背一首宋词".into(), ..Default::default() },
                CoalescedCommentData { id: "c2".into(), thread_id: "c2".into(), content: "毛泽东诗词背一首".into(), ..Default::default() },
            ];
        }));
        assert_eq!(targets.len(), 3);
        for tgt in &targets {
            let want_parent = match tgt.thread_id.as_str() {
                "c1" => "c1",
                "c2" => "c2",
                "c3" => "c3",
                other => panic!("unexpected thread {other}"),
            };
            assert_eq!(tgt.parent_id, want_parent);
        }

        // same-thread follow-ups consolidate to a single group
        let targets = comment_reply_threads(&task(|t| {
            t.trigger_comment_id = "c3".into();
            t.trigger_thread_id = "thread-A".into();
            t.coalesced_comments = vec![
                CoalescedCommentData { id: "c1".into(), thread_id: "thread-A".into(), content: "追问 1".into(), ..Default::default() },
                CoalescedCommentData { id: "c2".into(), thread_id: "thread-A".into(), content: "追问 2".into(), ..Default::default() },
            ];
        }));
        assert!(targets.is_empty());

        // mixed: trigger thread plus one other thread
        let targets = comment_reply_threads(&task(|t| {
            t.trigger_comment_id = "c3".into();
            t.trigger_thread_id = "thread-A".into();
            t.coalesced_comments = vec![
                CoalescedCommentData { id: "c1".into(), thread_id: "thread-A".into(), content: "same-thread follow-up".into(), ..Default::default() },
                CoalescedCommentData { id: "c2".into(), thread_id: "thread-B".into(), content: "other thread".into(), ..Default::default() },
            ];
        }));
        assert_eq!(targets.len(), 2);
        let got: std::collections::HashMap<&str, &str> =
            targets.iter().map(|t| (t.thread_id.as_str(), t.parent_id.as_str())).collect();
        assert_eq!(got["thread-A"], "c3");
        assert_eq!(got["thread-B"], "c2");

        // no coalesced comments → empty
        let targets = comment_reply_threads(&task(|t| {
            t.trigger_comment_id = "c1".into();
            t.trigger_thread_id = "thread-A".into();
        }));
        assert!(targets.is_empty());

        // non-trigger thread replies under its newest mention, not root
        let targets = comment_reply_threads(&task(|t| {
            t.trigger_comment_id = "c9".into();
            t.trigger_thread_id = "thread-A".into();
            t.coalesced_comments = vec![
                CoalescedCommentData { id: "c1".into(), thread_id: "thread-B".into(), content: "older mention".into(), created_at: "2026-07-10T01:00:00Z".into() },
                CoalescedCommentData { id: "c2".into(), thread_id: "thread-B".into(), content: "newer mention".into(), created_at: "2026-07-10T02:00:00Z".into() },
            ];
        }));
        let got: std::collections::HashMap<&str, &str> =
            targets.iter().map(|t| (t.thread_id.as_str(), t.parent_id.as_str())).collect();
        assert_eq!(got["thread-B"], "c2");
        assert_eq!(got["thread-A"], "c9");
    }

    /// TestBuildCommentPromptCrossThreadFansOutReplies
    /// (prompt_test.go:1463–1517).
    #[test]
    fn cross_thread_fans_out_replies() {
        let out = build_prompt(
            &task(|t| {
                t.issue_id = "issue-xthread-2".into();
                t.trigger_comment_id = "c3".into();
                t.trigger_thread_id = "c3".into();
                t.trigger_comment_content = "莎士比亚名言来一句".into();
                t.trigger_author_type = "member".into();
                t.coalesced_comment_ids = vec!["c1".into(), "c2".into()];
                t.coalesced_comments = vec![
                    CoalescedCommentData { id: "c1".into(), thread_id: "c1".into(), author_type: "member".into(), author_name: "Yushen".into(), content: "背一首宋词".into(), created_at: "2026-07-10T01:00:00Z".into() },
                    CoalescedCommentData { id: "c2".into(), thread_id: "c2".into(), author_type: "member".into(), author_name: "Yushen".into(), content: "毛泽东诗词背一首".into(), created_at: "2026-07-10T02:00:00Z".into() },
                ];
            }),
            "claude",
        );
        for want in ["3 DISTINCT threads", "Post ONE reply per thread", "OVERRIDES", "--parent c1", "--parent c2", "--parent c3"] {
            assert!(out.contains(want), "missing {want:?}\n---\n{out}");
        }
        assert!(!out.contains("always use the trigger comment ID below"), "{out}");
        assert!(!out.contains("cordy issue comment add"), "re-grew embedded commands (MUL-5825)\n{out}");
        assert!(out.contains("`## Comment Formatting`"), "{out}");
        assert!(out.contains("OLDEST thread first"), "{out}");
        let (pos_c1, pos_c2, pos_c3) =
            (out.find("--parent c1"), out.find("--parent c2"), out.find("--parent c3"));
        assert!(matches!((pos_c1, pos_c2, pos_c3), (Some(a), Some(b), Some(c)) if a < b && b < c), "{out}");
    }

    /// TestBuildCommentPromptSameThreadKeepsSingleReply
    /// (prompt_test.go:1523–1546).
    #[test]
    fn same_thread_keeps_single_reply() {
        let out = build_prompt(
            &task(|t| {
                t.issue_id = "issue-samethread-1".into();
                t.trigger_comment_id = "c3".into();
                t.trigger_thread_id = "thread-A".into();
                t.trigger_comment_content = "追问 3".into();
                t.trigger_author_type = "member".into();
                t.coalesced_comment_ids = vec!["c1".into(), "c2".into()];
                t.coalesced_comments = vec![
                    CoalescedCommentData { id: "c1".into(), thread_id: "thread-A".into(), author_type: "member".into(), author_name: "Yushen".into(), content: "追问 1".into(), created_at: "2026-07-10T01:00:00Z".into() },
                    CoalescedCommentData { id: "c2".into(), thread_id: "thread-A".into(), author_type: "member".into(), author_name: "Yushen".into(), content: "追问 2".into(), created_at: "2026-07-10T02:00:00Z".into() },
                ];
            }),
            "claude",
        );
        assert!(!out.contains("DISTINCT threads"), "{out}");
        assert!(out.contains("--parent c3 --content-file ./reply.md"), "{out}");
    }

    /// TestPerTurnContextBlocksCarryMovedBriefSections
    /// (prompt_test.go:1552–1590).
    #[test]
    fn per_turn_blocks_carry_moved_brief_sections() {
        let prompt = build_prompt(
            &task(|t| {
                t.issue_id = "issue-1".into();
                t.trigger_comment_id = "comment-1".into();
                t.trigger_comment_content = "please look at this".into();
                t.prior_session_resume_unavailable = true;
                t.initiator_type = "member".into();
                t.initiator_name = "Bohan".into();
                t.initiator_email = "bohan@example.com".into();
                t.connected_apps = vec![crate::types::ConnectedAppData {
                    provider: "composio".into(),
                    server_name: "composio".into(),
                    toolkit_slug: "notion".into(),
                    toolkit_name: "Notion".into(),
                }];
            }),
            "claude",
        );
        for want in [
            "## Session Continuity Notice",
            "could not be restored",
            "## Task Initiator",
            "initiated by **Bohan** (bohan@example.com), a member of this workspace",
            "credentials stay scoped to the runtime owner",
            "## Connected Apps",
            "- Notion (`notion`) via MCP server `composio`",
        ] {
            assert!(prompt.contains(want), "lost {want:?}\n---\n{prompt}");
        }
    }

    /// TestPerTurnContextBlocksOmittedWhenEmpty +
    /// TestPerTurnContextBlocksOnAssignmentPath
    /// (prompt_test.go:1593–1621).
    #[test]
    fn per_turn_blocks_omitted_when_empty_and_assignment_path() {
        let prompt = build_prompt(&task(|t| t.issue_id = "issue-1".into()), "claude");
        for banned in ["## Session Continuity Notice", "## Task Initiator", "## Connected Apps"] {
            assert!(!prompt.contains(banned), "emitted {banned:?} with no data\n---\n{prompt}");
        }
        let assignment = build_prompt(
            &task(|t| {
                t.issue_id = "issue-1".into();
                t.initiator_type = "agent".into();
                t.initiator_name = "GPT-Boy".into();
            }),
            "claude",
        );
        assert!(
            assignment.contains("initiated by **GPT-Boy**, another agent in this workspace"),
            "{assignment}"
        );
    }

    /// TestTurnModeMarkersRetired (prompt_test.go:1629–1653).
    #[test]
    fn turn_mode_markers_retired() {
        let cases: Vec<(String, Task)> = vec![
            ("comment-triggered with content".into(), task(|t| { t.issue_id = "issue-1".into(); t.trigger_comment_id = "c-1".into(); t.trigger_comment_content = "please look".into(); })),
            ("comment-triggered with EMPTY content".into(), task(|t| { t.issue_id = "issue-1".into(); t.trigger_comment_id = "c-1".into(); })),
            ("assignment-triggered".into(), task(|t| t.issue_id = "issue-1".into())),
            ("assignment-triggered with handoff note".into(), task(|t| { t.issue_id = "issue-1".into(); t.handoff_note = "start with the API".into(); })),
            ("chat".into(), task(|t| t.chat_session_id = "chat-1".into())),
            ("quick-create".into(), task(|t| t.quick_create_prompt = "make an issue".into())),
            ("autopilot".into(), task(|t| t.autopilot_run_id = "run-1".into())),
        ];
        for (name, t) in cases {
            let prompt = build_prompt(&t, "claude");
            assert!(!prompt.contains("Turn mode"), "{name} carries a turn-mode marker (MUL-6417)\n{prompt}");
        }
    }

    /// TestChatChannelDeliversFilesDefaultsOffAcrossVersions
    /// (prompt_test.go:1689–1730): decoded from JSON because "the server did
    /// not send this" is the case under test.
    #[test]
    fn chat_channel_delivers_files_defaults_off() {
        let old_server_claim = r#"{
            "id": "task-1",
            "chat_session_id": "sess-1",
            "chat_channel_type": "wecom",
            "chat_message": "make me a chart"
        }"#;
        let t: Task = serde_json::from_str(old_server_claim).unwrap();
        assert_eq!(t.chat_channel_type, "wecom");
        assert!(!t.chat_channel_delivers_files);

        let out = build_chat_prompt(&t);
        assert!(!out.contains("run `cordy attachment upload <local-path>`"), "{out}");
        assert!(out.contains("You cannot attach a file to it"), "{out}");

        let delivering: Task = serde_json::from_str(
            r#"{"chat_session_id":"sess-1","chat_channel_type":"wecom","chat_channel_delivers_files":true}"#,
        )
        .unwrap();
        let out = build_chat_prompt(&delivering);
        assert!(out.contains("run `cordy attachment upload <local-path>`"), "{out}");
    }
}
