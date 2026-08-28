//! Per-turn prompt assembly for
//! issue / comment / chat / autopilot / quick-create tasks, plus the
//! run-scoped context blocks (PB-5377) appended after the cached prefix.
//! Shared context sections live in [`crate::runtime_config_sections`].

use crate::execenv::channel_type::{
    audience_of, channel_carries_files, channel_display_name as execenv_channel_display_name,
    surface_persists_transcript, ChatAudience, CHANNEL_TYPE_SLACK,
};
use crate::runtime_config_sections::{
    build_connected_apps_block, build_task_initiator_block,
    session_continuity_notice_channel_history, session_continuity_notice_chat_transcript,
    session_continuity_notice_issue, session_continuity_notice_unrecoverable,
};
use crate::slash_skill::extract_slash_skills;
use crate::types::{ActiveSiblingRunData, Task};

/// Legacy role signal retained for historical squad briefings.
const SQUAD_BRIEFING_MARKER: &str = "## Squad Operating Protocol";

/// `sessionContinuityNoticeFor`.
pub(crate) fn session_continuity_notice_for(task: &Task) -> &'static str {
    if task.chat_session_id.is_empty() {
        return session_continuity_notice_issue();
    }
    if task.chat_channel_type == CHANNEL_TYPE_SLACK {
        return session_continuity_notice_channel_history();
    }
    // Every other transcript-persisting surface reads back via `patchbay chat
    // history`; only a surface that never stored a transcript falls through.
    if surface_persists_transcript(&task.chat_channel_type) {
        return session_continuity_notice_chat_transcript();
    }
    session_continuity_notice_unrecoverable()
}

/// `backendResumeContinuityNotice`: "" when the prompt already carries one.
pub(crate) fn backend_resume_continuity_notice(task: &Task) -> String {
    if task.prior_session_resume_unavailable {
        return String::new();
    }
    session_continuity_notice_for(task).to_string()
}

/// `perTurnContextBlocks`.
pub(crate) fn per_turn_context_blocks(task: &Task) -> String {
    let mut b = String::new();
    b.push_str(&build_active_sibling_runs_block(
        &task.issue_id,
        &task.active_sibling_runs,
    ));
    if task.prior_session_resume_unavailable {
        b.push_str(session_continuity_notice_for(task));
    }
    b.push_str(&build_task_initiator_block(
        &task.initiator_type,
        &task.initiator_name,
        &task.initiator_email,
    ));
    b.push_str(&build_connected_apps_block(&task.connected_apps));
    b
}

fn build_active_sibling_runs_block(
    current_issue_id: &str,
    runs: &[ActiveSiblingRunData],
) -> String {
    // Sibling work is useful context only for another issue task. Rendering it
    // on chat/autopilot/quick-create creates an unactionable warning.
    if current_issue_id.is_empty() || runs.is_empty() {
        return String::new();
    }
    let mut b = String::new();
    b.push_str("## Active sibling runs\n\n");
    b.push_str("This agent has other in-flight issue tasks. Before starting overlapping code or PR work, check this issue's comment history for a claim or handoff");
    b.push_str(&format!(
        " (`patchbay issue comment list {current_issue_id} --roots-only --summary --compact --output json`)"
    ));
    b.push_str(" and inspect relevant siblings with the `run-messages` commands below — coordinate with existing work instead of opening a second PR. For writes that only record ownership or status of work already underway, use `--no-start` on `patchbay issue assign`/`update`/`status`.\n\n");
    for run in runs {
        let issue_label = if run.issue_identifier.is_empty() {
            &run.issue_id
        } else {
            &run.issue_identifier
        };
        b.push_str(&format!(
            "- {} — task `{}`, status `{}`",
            issue_label, run.task_id, run.status
        ));
        if !run.started_at.is_empty() {
            b.push_str(&format!(", started {}", run.started_at));
        } else if !run.created_at.is_empty() {
            b.push_str(&format!(", created {}", run.created_at));
        }
        let title = run.issue_title.replace(['\r', '\n'], " ");
        let title = title.trim();
        if !title.is_empty() {
            b.push_str(&format!(": {title}"));
        }
        b.push_str(&format!(
            "; inspect: `patchbay issue run-messages {}`\n",
            run.task_id
        ));
    }
    b.push('\n');
    b
}

/// `BuildPrompt`: constructs the task prompt for an agent CLI. Run-scoped
/// context is appended, never prepended (PB-5377).
pub(crate) fn build_prompt(task: Task, provider: &str) -> String {
    let mut body = build_prompt_body(&task, provider);
    let blocks = per_turn_context_blocks(&task);
    if !blocks.is_empty() {
        if !body.ends_with("\n\n") {
            body.push('\n');
        }
        body.push_str(&blocks);
    }
    body
}

fn build_prompt_body(task: &Task, provider: &str) -> String {
    if !task.chat_session_id.is_empty() {
        return build_chat_prompt(task);
    }
    if !task.message_bus_messages.is_empty() {
        return build_message_bus_prompt(task, provider);
    }
    if !task.side_chat_parent_task_id.is_empty() {
        return build_side_chat_prompt(task, provider);
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
    b.push_str("You are running as a local coding agent for a Patchbay workspace.\n\n");
    b.push_str(&format!("Your assigned issue ID is: {}\n\n", task.issue_id));
    // Assignment handoff (PB-3375).
    if !task.handoff_note.is_empty() {
        b.push_str("You were handed this issue with a handoff note. Treat it as the assigner's scoping instruction for this run; follow it before doing anything broader, and do not reply to it as if it were a comment:\n\n");
        b.push_str(&format!("> {}\n\n", task.handoff_note));
    }
    b.push_str(&format!(
        "Start by running `patchbay issue get {} --output json` to understand your task, then complete it.\n",
        task.issue_id
    ));
    b.push_str(&format!(
        "For comment history, follow the rule in your runtime workflow file (assignment-triggered tasks treat the read as mandatory). Scan the threads first with `patchbay issue comment list {} --roots-only --summary --compact --output json`, then expand only what matters with `--thread <thread-id> --tail 30`. For `--since` incremental polling, pagination, and folding, see `patchbay issue comment list --help`.\n",
        task.issue_id
    ));
    b
}

/// A comment-triggered Side Chat is an isolated discussion branch for the
/// specifically mentioned Agent. It cannot edit the main task directly. Once
/// an edit is actually called for, the branch autonomously records one
/// structured instruction on Patchbay's provider-neutral Message Bus.
fn build_side_chat_prompt(task: &Task, provider: &str) -> String {
    let mut b = String::new();
    b.push_str(
        "You are in a Patchbay Side Chat for the Agent explicitly @mentioned in an issue comment.\n\n",
    );
    b.push_str(&format!(
        "This Side Chat is derived from main task `{}` on issue `{}`. The main task and any inherited provider history are reference-only here. Do not modify files, run mutating commands, create commits or pull requests, or continue the main task from this branch.\n\n",
        task.side_chat_parent_task_id, task.issue_id
    ));
    if !task.side_chat_root_comment_id.is_empty() {
        b.push_str(&format!(
            "Start by reading this Side Chat's durable conversation history: `patchbay issue comment list {} --thread {} --full --output json`. Use that comment thread—not the provider's fork support—as the source of truth for prior Side Chat turns.\n\n",
            task.issue_id, task.side_chat_root_comment_id
        ));
    }
    if !task.trigger_comment_content.is_empty() {
        b.push_str("The user said in this Side Chat:\n\n");
        b.push_str(&format!(
            "> {}\n\n",
            task.trigger_comment_content.trim().replace('\n', "\n> ")
        ));
    }
    b.push_str("Use your own judgment:\n");
    b.push_str("- If this is discussion, analysis, or a question, answer it only in this Side Chat. Do not contact the main task.\n");
    b.push_str("- If the conversation clearly requires a code or workspace edit, formulate the concrete next-step instruction yourself and immediately send it to the @mentioned Agent's main task. Do not ask the user to copy it, relay it, click a button, or run a command.\n\n");
    b.push_str(&format!(
        "To deliver that confirmed edit instruction, invoke `patchbay issue message-main {} --content-stdin` exactly once and pass the instruction verbatim through the command's stdin. Never interpolate the instruction into a shell argument. This queues the next turn of the @mentioned Agent's main conversation at a safe boundary; it does not inject work into this Side Chat.\n\n",
        task.side_chat_parent_task_id
    ));
    b.push_str(&format!(
        "When the main conversation's implementation state matters, inspect its durable task record with `patchbay issue run-messages {} --output json`.\n\n",
        task.side_chat_parent_task_id
    ));
    let targets = comment_reply_threads(task);
    if targets.len() >= 2 {
        b.push_str(
            &crate::runtime_config_sections::build_multi_thread_comment_reply_instructions(
                &task.issue_id,
                &targets,
                false,
            ),
        );
    } else {
        b.push_str(
            &crate::runtime_config_sections::build_comment_reply_instructions(
                provider,
                &task.issue_id,
                &task.trigger_comment_id,
                false,
            ),
        );
    }
    b
}

/// Follow-up work delivered by a Side Chat is a new turn on the exact main
/// task/session. Every provider receives the same prompt contract; native
/// session resume remains an adapter detail.
fn build_message_bus_prompt(task: &Task, provider: &str) -> String {
    let mut b = String::new();
    b.push_str("You are continuing a Patchbay main conversation after its Side Chat confirmed that implementation work is needed.\n\n");
    b.push_str(&format!(
        "Main conversation anchor task: `{}`\nIssue: `{}`\n\n",
        task.message_bus_parent_task_id, task.issue_id
    ));
    b.push_str("Patchbay Message Bus instructions, in delivery order:\n\n");
    for message in &task.message_bus_messages {
        b.push_str(&format!(
            "- From Side Chat task `{}`: {}\n",
            message.source_task_id,
            message.content.trim().replace('\n', "\n  ")
        ));
    }
    b.push_str("\nTreat these as the confirmed next steps for the same Agent and main conversation. Resume that conversation's latest provider session when available, inspect the current workspace state, carry the requested edits through to completion, and report the result in the originating issue discussion. Do not turn this back into another Side Chat and do not ask the user to relay the instruction.\n\n");
    b.push_str(&format!(
        "Start by running `patchbay issue get {} --output json`, then inspect the existing work before editing.\n\n",
        task.issue_id
    ));
    let targets = comment_reply_threads(task);
    if targets.len() >= 2 {
        b.push_str(
            &crate::runtime_config_sections::build_multi_thread_comment_reply_instructions(
                &task.issue_id,
                &targets,
                false,
            ),
        );
    } else {
        b.push_str(
            &crate::runtime_config_sections::build_comment_reply_instructions(
                provider,
                &task.issue_id,
                &task.trigger_comment_id,
                false,
            ),
        );
    }
    b
}

/// `buildCommentPrompt`: the triggering comment content is embedded directly;
/// reply instructions are re-emitted every turn so resumed sessions cannot
/// carry forward a previous turn's --parent UUID.
pub(crate) fn build_comment_prompt(task: &Task, provider: &str) -> String {
    let mut b = String::new();
    b.push_str("You are running as a local coding agent for a Patchbay workspace.\n\n");
    b.push_str(&format!("Your assigned issue ID is: {}\n\n", task.issue_id));
    if !task.trigger_comment_content.is_empty() {
        let author_label = match task.trigger_author_type.as_str() {
            "system" => "The platform".to_string(),
            "agent" => {
                let name = if task.trigger_author_name.is_empty() {
                    "another agent".to_string()
                } else {
                    task.trigger_author_name.clone()
                };
                format!("Another agent ({name})")
            }
            _ => "A user".to_string(),
        };
        b.push_str(&format!(
            "[NEW COMMENT] {author_label} just left a new comment. Focus on THIS comment — do not confuse it with previous ones:\n\n"
        ));
        b.push_str(&format!("> {}\n\n", task.trigger_comment_content));

        // PB-4195: folded comments must be addressed too.
        if !task.coalesced_comments.is_empty() {
            b.push_str(&format!(
                "This run also covers {} earlier comment(s) posted before it started — you must read and address them too, not just the one above. They may be in different threads, so each is reproduced here with its own thread:\n\n",
                task.coalesced_comments.len()
            ));
            for cc in &task.coalesced_comments {
                let author_label = match cc.author_type.as_str() {
                    "system" => "The platform".to_string(),
                    "agent" => {
                        let name = if cc.author_name.is_empty() {
                            "another agent".to_string()
                        } else {
                            cc.author_name.clone()
                        };
                        format!("Another agent ({name})")
                    }
                    _ => {
                        if !cc.author_name.is_empty() {
                            cc.author_name.clone()
                        } else {
                            "A user".to_string()
                        }
                    }
                };
                b.push_str(&format!("- comment {}", cc.id));
                if !cc.created_at.is_empty() {
                    b.push_str(&format!(" ({}, {})", author_label, cc.created_at));
                } else {
                    b.push_str(&format!(" ({})", author_label));
                }
                if !cc.thread_id.is_empty() {
                    b.push_str(&format!(" [thread {}]", cc.thread_id));
                }
                b.push_str(":\n");
                b.push_str(&format!(
                    "  > {}\n",
                    cc.content.trim().replace('\n', "\n  > ")
                ));
            }
            b.push_str(&format!(
                "\nIf you need the surrounding discussion for any of them, fetch its thread with `patchbay issue comment list {} --thread <thread-id> --tail 30 --compact --output json` using the thread id shown above.\n\n",
                task.issue_id
            ));
        } else if !task.coalesced_comment_ids.is_empty() {
            // PB-5442 replacement: per-id lookup instead of `--recent 30`.
            b.push_str(&format!(
                "This run also covers {} earlier comment(s) posted before it started — you must read and address every one of them, not just the one above: {}. They may be in DIFFERENT threads, so do not assume they share the triggering thread.\n\n",
                task.coalesced_comment_ids.len(),
                task.coalesced_comment_ids.join(", ")
            ));
            if !task.new_comments_since.is_empty() {
                b.push_str(&format!(
                    "Start with `patchbay issue comment list {} --since {} --compact --output json`. Treat that as a candidate window, not a guarantee — it also carries unrelated comments, and a retried run can carry ids older than the window. Check every id above against the result.\n\n",
                    task.issue_id, task.new_comments_since
                ));
            }
            b.push_str(&format!(
                "Fetch each id you still need directly: `patchbay issue comment list {} --thread <comment-id> --tail 30 --compact --output json`. `--thread` accepts a reply id, not just a thread root, so you do not need to know which thread the comment lives in. If it is older than those 30 replies, page back with the `Next reply cursor` values (`--before` / `--before-id`) until it appears. Do not finish this turn until every id above is accounted for.\n\n",
                task.issue_id
            ));
        }
        if task_is_squad_leader(task) {
            b.push_str(&format!(
                "⚠️ **Squad leader no_action rule:** If you decide no action is needed, call `patchbay squad activity {} no_action --reason \"...\"` and EXIT. DO NOT post any comment — not even one that says \"no action needed\" or \"exiting silently\". The squad activity call records your decision; a comment is redundant noise.\n\n",
                task.issue_id
            ));
        }
    }

    b.push_str(&format!(
        "Start by running `patchbay issue get {} --output json` to understand your task, then decide how to proceed.\n\n",
        task.issue_id
    ));
    // Comment-reading pointer: warm-with-delta → warm-resumed → cold → plain.
    if let Some(hint) = crate::runtime_config_sections::build_new_comments_hint(
        &task.issue_id,
        &task.trigger_comment_id,
        &task.trigger_thread_id,
        &task.new_comments_since,
        task.new_comment_count,
    ) {
        b.push_str(&hint);
    } else if !task.prior_session_id.is_empty() {
        b.push_str(
            &crate::runtime_config_sections::build_resumed_comments_hint(
                &task.issue_id,
                &task.trigger_comment_id,
                &task.trigger_thread_id,
            ),
        );
    } else if let Some(cold) = crate::runtime_config_sections::build_cold_comments_hint(
        &task.issue_id,
        &task.trigger_comment_id,
        &task.trigger_thread_id,
    ) {
        b.push_str(&cold);
    } else {
        b.push_str(&format!(
            "Read the discussion: scan with `patchbay issue comment list {} --roots-only --summary --compact --output json`, then expand what matters with `--thread <thread-id> --tail 30`.\n\n",
            task.issue_id
        ));
    }

    // Reply routing (PB-4348): more than one distinct root thread fans out.
    let targets = comment_reply_threads(task);
    if targets.len() >= 2 {
        b.push_str(
            &crate::runtime_config_sections::build_multi_thread_comment_reply_instructions(
                &task.issue_id,
                &targets,
                task_is_squad_leader(task),
            ),
        );
    } else {
        b.push_str(
            &crate::runtime_config_sections::build_comment_reply_instructions(
                provider,
                &task.issue_id,
                &task.trigger_comment_id,
                task_is_squad_leader(task),
            ),
        );
    }
    b
}

/// `commentReplyThreads`: groups trigger + coalesced comments by root thread,
/// first-seen order, newest comment winning its thread's reply target.
pub(crate) fn comment_reply_threads(
    task: &Task,
) -> Vec<crate::execenv::execenv::ThreadReplyTarget> {
    if task.trigger_comment_id.is_empty() {
        return Vec::new();
    }
    let thread_key = |thread_id: &str, comment_id: &str| -> String {
        if !thread_id.is_empty() {
            thread_id.to_string()
        } else {
            comment_id.to_string()
        }
    };

    let mut order: Vec<String> = Vec::new();
    let mut parent_by_thread: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut note = |thread_id: String, parent_id: String| {
        if !parent_by_thread.contains_key(&thread_id) {
            order.push(thread_id.clone());
        }
        parent_by_thread.insert(thread_id, parent_id);
    };

    // Coalesced (older) comments first.
    for cc in &task.coalesced_comments {
        note(thread_key(&cc.thread_id, &cc.id), cc.id.clone());
    }
    // The newest trigger last: always wins its own thread.
    note(
        thread_key(&task.trigger_thread_id, &task.trigger_comment_id),
        task.trigger_comment_id.clone(),
    );

    if order.len() <= 1 {
        return Vec::new();
    }
    order
        .into_iter()
        .map(|tid| crate::execenv::execenv::ThreadReplyTarget {
            thread_id: tid.clone(),
            parent_id: parent_by_thread.get(&tid).cloned().unwrap_or_default(),
        })
        .collect()
}

/// `buildChatPrompt`.
pub(crate) fn build_chat_prompt(task: &Task) -> String {
    // Legacy compatibility for historical proactive-introduction sessions.
    if task.chat_intro {
        return "You are running as a chat assistant for a Patchbay workspace.\nYou were just created, and this is the very first message in a direct chat with the person who created you. They have not written anything yet — you are opening the conversation. Send a short, warm, first-person introduction: who you are, what you're good at, and how they can work with you. Do NOT phrase it as an answer to a question or repeat any prompt back; just introduce yourself as if you reached out first.\n".to_string();
    }

    let mut b = String::new();
    b.push_str("You are running as a chat assistant for a Patchbay workspace.\n");
    match audience_of(&task.chat_channel_type, &task.chat_type) {
        ChatAudience::Group => {
            b.push_str("Audience: group room; not private; unseen members may read replies.\n\n")
        }
        ChatAudience::Unknown => b.push_str("Audience: unknown.\n\n"),
        _ => b.push_str("Audience: direct room.\n\n"),
    }
    // Channel awareness (PB-3871 / PB-4899): where the conversation lives is
    // per-branch, the no-narration rule is a third axis emitted for every type.
    if !task.chat_channel_type.is_empty() {
        let platform = channel_display_name(&task.chat_channel_type);
        b.push_str(&format!(
            "You are operating inside a {platform} conversation — not the Patchbay web app. Never look in Patchbay issues or comments for this conversation.\n"
        ));
        if task.chat_channel_type == CHANNEL_TYPE_SLACK {
            b.push_str(&format!(
                "This conversation and its history live in {platform}, NOT in Patchbay. The message below may be only what triggered you. Read the conversation with:\n"
            ));
            b.push_str("- `patchbay chat history --output json` — the channel overview: recent top-level messages, each thread tagged with a `thread_id` and `reply_count`. It does NOT expand thread contents.\n");
            b.push_str("- `patchbay chat thread [<thread_id>] --output json` — read one thread's messages; omit the id to read the thread you are in, or pass a `thread_id` from the overview to read a specific thread.\n");
            if task.chat_in_thread {
                b.push_str("You were @mentioned inside a thread: start with `patchbay chat thread` to read it; if you need the wider channel, run `patchbay chat history` and open a specific thread with `patchbay chat thread <thread_id>`.\n");
            } else {
                b.push_str("You were @mentioned at the channel top level: start with `patchbay chat history` to see the channel, then read a specific thread's contents with `patchbay chat thread <thread_id>`.\n");
            }
            b.push_str("Do these reads SILENTLY as an internal step — they are how you gather context, not part of your answer.\n");
        } else if surface_persists_transcript(&task.chat_channel_type) {
            b.push_str(&format!(
                "The conversation happens in {platform}, and Patchbay stores a transcript of it. The message below may be only what triggered you — read it back with `patchbay chat history` when you need earlier context that is not below.\n"
            ));
        } else {
            b.push_str(&format!(
                "This conversation and its history live in {platform}, NOT in Patchbay, and Patchbay has no history reader for it. Work from the context already provided to you below — no command can fetch more of this conversation. If you genuinely need earlier context that is not here, ask the user for it rather than guessing.\n"
            ));
        }
        b.push_str(&format!(
            "Reply to {platform} with the final outcome only. Do NOT narrate planned or in-progress steps (\"我先读取…\"); completed actions are part of the outcome.\n"
        ));
        b.push('\n');
    }

    // Explicitly selected skills via slash links.
    if let Some(agent) = &task.agent {
        if !agent.skills.is_empty() {
            let refs = extract_slash_skills(&task.chat_message);
            if !refs.is_empty() {
                let agent_skills: std::collections::HashMap<&str, &str> = agent
                    .skills
                    .iter()
                    .map(|s| (s.id.as_str(), s.name.as_str()))
                    .collect();
                let mut selected: Vec<&str> = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for r in refs {
                    let Some(name) = agent_skills.get(r.id.as_str()) else {
                        continue;
                    };
                    if !seen.insert(r.id.clone()) {
                        continue;
                    }
                    selected.push(name);
                }
                if !selected.is_empty() {
                    b.push_str("Explicitly selected skills:\n");
                    for name in selected {
                        b.push_str(&format!("- {name}\n"));
                    }
                    b.push('\n');
                }
            }
        }
    }

    b.push_str(&format!("User message:\n{}\n", task.chat_message));

    // Attachments listed by id + filename (signed CDN URLs expire).
    if !task.chat_message_attachments.is_empty() {
        b.push_str("\nAttachments on this message:\n");
        for a in &task.chat_message_attachments {
            if !a.content_type.is_empty() {
                b.push_str(&format!(
                    "- id={} filename={:?} content_type={}\n",
                    a.id, a.filename, a.content_type
                ));
            } else {
                b.push_str(&format!("- id={} filename={:?}\n", a.id, a.filename));
            }
        }
        b.push_str("Use `patchbay attachment download <id>` to fetch each file locally before referring to it.\n");
        b.push_str("When creating an issue that should preserve one of these attachments, pass `--attachment-id <id>` to `patchbay issue create` in addition to keeping the attachment markdown inline.\n");
    }

    // Outbound attachments: three answers, not two (PB-4899). The ONLY place
    // the verdict is stated.
    if task.chat_channel_type.is_empty() {
        b.push_str("\nTo include a file or image you produced in your reply, run `patchbay attachment upload <local-path>`. The file binds to your reply automatically and appears as an attachment card below it even if you paste nothing. The command also returns a `markdown` snippet you may paste on its own line to place the item where you want it (files render as a card, images inline).\n");
    } else if channel_carries_files(&task.chat_channel_type, task.chat_channel_delivers_files) {
        let platform = channel_display_name(&task.chat_channel_type);
        b.push_str(&format!(
            "\nTo include a file or image you produced in your reply, run `patchbay attachment upload <local-path>`. It binds to your reply and Patchbay sends it into the {platform} conversation as a separate message right after your text — there is no way to place it inline, so write your reply to read correctly with the file arriving after it.\n"
        ));
    } else {
        let platform = channel_display_name(&task.chat_channel_type);
        b.push_str(&format!(
            "\nThis reply is delivered to {platform} as text. You cannot attach a file to it: `patchbay attachment upload` binds to a Patchbay chat reply, which this is not. If you produce a file, describe it in words — never write its local path as a link, and never upload it and then write as though it arrived.\n"
        ));
    }
    b
}

/// `channelDisplayName`.
pub(crate) fn channel_display_name(channel_type: &str) -> String {
    execenv_channel_display_name(channel_type)
}

/// `buildAutopilotPrompt`.
pub(crate) fn build_autopilot_prompt(task: &Task) -> String {
    let mut b = String::new();
    b.push_str("You are running as a local coding agent for a Patchbay workspace.\n\n");
    b.push_str("This task was triggered by an Autopilot in run-only mode. There is no assigned Patchbay issue for this run.\n\n");
    b.push_str(&format!("Autopilot run ID: {}\n", task.autopilot_run_id));
    if !task.autopilot_id.is_empty() {
        b.push_str(&format!("Autopilot ID: {}\n", task.autopilot_id));
    }
    if !task.autopilot_title.is_empty() {
        b.push_str(&format!("Autopilot title: {}\n", task.autopilot_title));
    }
    if !task.autopilot_source.is_empty() {
        b.push_str(&format!("Trigger source: {}\n", task.autopilot_source));
    }
    if let Some(payload) = &task.autopilot_trigger_payload {
        let trimmed = payload.to_string().trim().to_string();
        if !trimmed.is_empty() && trimmed != "null" {
            b.push_str(&format!("Trigger payload:\n{trimmed}\n\n"));
        }
    }
    b.push_str("\nAutopilot instructions:\n");
    if !task.autopilot_description.trim().is_empty() {
        b.push_str(&task.autopilot_description);
        b.push_str("\n\n");
    } else if !task.autopilot_title.is_empty() {
        b.push_str(&format!("{}\n\n", task.autopilot_title));
    } else {
        b.push_str("No additional autopilot instructions were provided. Inspect the autopilot configuration before proceeding.\n\n");
    }
    if !task.autopilot_id.is_empty() {
        b.push_str(&format!(
            "Start by running `patchbay autopilot get {} --output json` if you need the full autopilot configuration, then complete the instructions above.\n",
            task.autopilot_id
        ));
    } else {
        b.push_str("Complete the instructions above.\n");
    }
    b
}

/// `buildQuickCreatePrompt`.
pub(crate) fn build_quick_create_prompt(task: &Task) -> String {
    let mut b = String::new();
    b.push_str("You are running as a quick-create assistant for a Patchbay workspace.\n\n");
    b.push_str("A user captured the following input via the quick-create modal. There is NO existing issue. Your job is to create a well-formed issue from this input with a single `patchbay issue create` command.\n\n");
    b.push_str(&format!("User input:\n> {}\n\n", task.quick_create_prompt));

    b.push_str("Field rules:\n\n");
    b.push_str("- **title**: required. A concise but semantically rich summary. If the input references external resources (PRs, issues, URLs), use your judgment on whether fetching the resource would produce a meaningfully better title — e.g. \"review PR #123\" → \"Review PR #123: Refactor auth module to OAuth2\". Strip filler words but preserve key semantic information.\n\n");

    b.push_str("- **description**: The description is the executing agent's primary context. Aim for high fidelity — they should grasp the user's intent as if they had read the raw input themselves. Use a two-section structure:\n\n");
    b.push_str("  1. **User request** — Faithfully restate what the user wants in their own words. Preserve specific names, identifiers, file paths, code snippets, and technical terms verbatim. Strip non-spec material before writing it (this is removal, not paraphrasing): verbal routing wrappers about creating the issue or routing it (e.g. \"create an issue\", \"分配给 X\", \"让 @X 处理\") and pure conversational fillers (e.g. \"对吧？\"). When in doubt, keep it.\n\n");
    b.push_str("     CC exception: `patchbay issue create` has no `--subscriber` flag, and the platform auto-subscribes members whose `[@Name](mention://member/<uuid>)` link appears in the description. When the user wrote \"cc @Y\", strip the verbal \"cc\" wrapper from the User request body and append a final `CC: <mention link(s)>` line to the description so the cc routing still fires.\n\n");
    b.push_str("  2. **Context** — include ONLY when the input cited external resources AND you successfully fetched them AND they produced verifiable facts worth recording. Summarize facts only (e.g. \"PR #45 changes auth to JWT\"), not interpretation or unsolicited reference implementations. If you have nothing factual to add, omit the section entirely — never use it as an apology log for resources you could not fetch.\n\n");
    b.push_str("  Hard rules: never invent requirements, implementation details, or acceptance criteria the user did not express; never reduce multi-sentence input to a single vague sentence; never echo the title.\n\n");
    b.push_str("  Passing the description: a short, single-line body with no code, quotes, backticks, `$()`, or other special characters may go inline via `--description \"...\"`. Anything multi-line, or containing code snippets / file paths / quotes / backticks / `$()` / special characters, or otherwise long — which quick-create descriptions usually are — MUST be written to `./description.md` and passed with `--description-file ./description.md`; passing rich text inline lets the shell rewrite or truncate it (PB-2904). That file MUST live inside your current working directory (e.g. `./description.md`) — never `/tmp` or any machine-shared path, where a different run may have left a stale file that would silently become this issue's description. If the file write fails for any reason, stop and fix it; never run `--description-file` against a file whose write did not succeed.\n\n");

    if !task.quick_create_priority.is_empty() {
        b.push_str(&format!(
            "- **priority**: required for this run. Pass `--priority {}`; the quick-create selection is authoritative.\n\n",
            task.quick_create_priority
        ));
    } else {
        b.push_str("- **priority**: one of `urgent`, `high`, `medium`, `low`, or omit. Map P0/P1 → urgent/high; \"asap\" → urgent. If unspecified, omit.\n\n");
    }

    b.push_str("- **assignee**:\n");
    b.push_str("    - When the user names someone (\"assign to X\" / \"@X\"), call `patchbay workspace member list --output json`, `patchbay agent list --output json`, and `patchbay squad list --output json` and find the matching entity by display name. Squads are first-class assignees too — a squad name (e.g. \"Super Human\") routes work to the squad leader, who then delegates. On a clean unambiguous match, prefer `--assignee-id <uuid>` using the `user_id` (member) or `id` (agent or squad) from that JSON — UUID matching is exact and robust to name collisions in workspaces with overlapping names. `--assignee <name>` (fuzzy) is acceptable as a fallback when names are unambiguous. On no match or ambiguous match, do NOT pass either flag — instead append a final line to the description: `Unrecognized assignee: X`.\n");
    b.push_str("    - Treat bare @-routing as an assignee directive even when the user did not write the English word \"assign\". This includes Chinese imperatives like `让 @独立团 review 这个 PR`, `给 @X 处理`, or `交给 @X`; strip the leading `@`/`＠` before matching display names. Do not keep that routing wrapper or `@Name` in the description unless it is a true CC-style notification rather than ownership. If the matched entity is a squad, pass the squad's `id` as `--assignee-id`, not the leader agent's id.\n");
    let agent_id = task
        .agent
        .as_ref()
        .map(|a| a.id.clone())
        .unwrap_or_default();
    let agent_name = task
        .agent
        .as_ref()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    if !task.squad_id.is_empty() {
        // Squad picker opened quick-create: the squad owns the flow.
        if !task.squad_name.is_empty() {
            b.push_str(&format!(
                "    - When the user did NOT name an assignee, default to the picker SQUAD {:?}: pass `--assignee-id {:?}` (the squad's UUID). The user opened quick-create with the squad selected; you (the leader agent) are running on the squad's behalf, so the squad — not you — is the expected owner. Never leave the issue unassigned, and do not assign it to your own agent UUID.\n\n",
                task.squad_name, task.squad_id
            ));
        } else {
            b.push_str(&format!(
                "    - When the user did NOT name an assignee, default to the picker SQUAD: pass `--assignee-id {:?}` (the squad's UUID). The user opened quick-create with the squad selected; you (the leader agent) are running on the squad's behalf, so the squad — not you — is the expected owner. Never leave the issue unassigned, and do not assign it to your own agent UUID.\n\n",
                task.squad_id
            ));
        }
    } else if !agent_id.is_empty() {
        b.push_str(&format!(
            "    - When the user did NOT name an assignee, default to YOURSELF: pass `--assignee-id {:?}` (your agent UUID). The picker agent is the expected owner because the user opened quick-create with you selected — never leave the issue unassigned. Use the UUID flag, not `--assignee <name>`, so the assignment is unambiguous even when other agents share part of your name.\n\n",
            agent_id
        ));
    } else if !agent_name.is_empty() {
        b.push_str(&format!(
            "    - When the user did NOT name an assignee, default to YOURSELF: pass `--assignee {:?}`. The picker agent is the expected owner because the user opened quick-create with you selected — never leave the issue unassigned.\n\n",
            agent_name
        ));
    } else {
        b.push_str("    - When the user did NOT name an assignee, default to YOURSELF (the picker agent): pass `--assignee-id <your agent UUID>` (preferred) or `--assignee <your agent name>`. Never leave the issue unassigned.\n\n");
    }

    if !task.quick_create_due_date.is_empty() {
        b.push_str(&format!(
            "- **due-date**: required for this run. Pass `--due-date {}`; the quick-create selection is authoritative.\n\n",
            task.quick_create_due_date
        ));
    }

    if !task.project_id.is_empty() {
        if !task.project_title.is_empty() {
            b.push_str(&format!(
                "- **project**: required for this run. Pass `--project {:?}` so the new issue lands in project {:?} (the user picked it in the quick-create modal). Do not infer a different project from the prompt text — the modal selection is authoritative.\n",
                task.project_id, task.project_title
            ));
        } else {
            b.push_str(&format!(
                "- **project**: required for this run. Pass `--project {:?}` so the new issue lands in the project the user picked in the quick-create modal. Do not infer a different project from the prompt text — the modal selection is authoritative.\n",
                task.project_id
            ));
        }
    } else {
        b.push_str(
            "- **project**: omit. The platform will route the issue to the workspace default.\n",
        );
    }

    if !task.parent_issue_id.is_empty() {
        if !task.parent_issue_identifier.is_empty() {
            b.push_str(&format!(
                "- **parent**: required for this run. Pass `--parent {:?}` so the new issue is filed as a sub-issue of {} (the user opened quick-create from that issue's \"Add sub issue\" entry). Do not infer a different parent from the prompt text — the modal entry point is authoritative.\n",
                task.parent_issue_id, task.parent_issue_identifier
            ));
        } else {
            b.push_str(&format!(
                "- **parent**: required for this run. Pass `--parent {:?}` so the new issue is filed as a sub-issue of the parent the user picked in the quick-create modal. Do not infer a different parent from the prompt text — the modal entry point is authoritative.\n",
                task.parent_issue_id
            ));
        }
    }
    b.push_str("- **status**: omit (defaults to `todo`).\n");
    b.push_str("- **attachments**: `--attachment` takes LOCAL file paths, never URLs. Image URLs in the user input are already markdown — keep them inline. Files you produced: see `## Output`.\n\n");

    b.push_str("Output format:\n");
    b.push_str("- Run exactly one `patchbay issue create --output json` invocation. Do not retry for any reason — even on non-zero exit. The issue may already exist; another attempt would create a duplicate.\n");
    b.push_str("- Parse the JSON response to read the created issue's `identifier` (preferred) or `id` (fallback). Do not scrape human output and do not assume any workspace issue prefix such as `PB-`; workspaces can use custom prefixes.\n");
    b.push_str("- After success, print exactly one line: `Created <identifier-or-id>: <title>` and exit. No commentary, no follow-up tool calls.\n");
    b.push_str("- Do NOT call `patchbay issue get` or `patchbay issue comment add` — there is no issue to query or comment on.\n");
    b.push_str("- On CLI error or JSON parse error, exit with the error as the only output. The platform writes a failure notification automatically.\n");
    b
}

/// `taskIsSquadLeader`: leadership is a PER-TASK role. Servers without the
/// capability fall back to the legacy briefing-marker sniff (PB-5811).
pub(crate) fn task_is_squad_leader(task: &Task) -> bool {
    if !task.leader_role_resolved {
        return task
            .agent
            .as_ref()
            .map(|a| a.instructions.contains(SQUAD_BRIEFING_MARKER))
            .unwrap_or(false);
    }
    task.is_leader_task || !task.squad_id.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentData, CoalescedCommentData, TaskMessageBusMessageData};

    fn base_task() -> Task {
        Task {
            issue_id: "issue-123".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn handoff_note_assignment_branch() {
        let note = "Only touch the login flow; do not change payments.";
        let mut t = base_task();
        t.handoff_note = note.to_string();
        let out = build_prompt(t, "claude");
        assert!(out.contains(note));
        assert!(out.contains("handoff note"));
        assert!(!out.contains("quick-create assistant"));
        assert!(out.contains("patchbay issue get issue-123"));
    }

    #[test]
    fn no_handoff_note_unchanged() {
        let out = build_prompt(base_task(), "claude");
        assert!(!out.contains("handoff note"));
    }

    #[test]
    fn assignment_prompt_lists_read_recipe() {
        let out = build_prompt(base_task(), "claude");
        assert!(out.contains("--roots-only --summary --compact"));
        assert!(out.contains("`--since` incremental polling"));
    }

    #[test]
    fn comment_prompt_embeds_trigger_and_parent() {
        let mut t = base_task();
        t.trigger_comment_id = "c-42".to_string();
        t.trigger_comment_content = "please look".to_string();
        t.trigger_author_type = "member".to_string();
        let out = build_prompt(t, "claude");
        assert!(out.contains("[NEW COMMENT] A user"));
        assert!(out.contains("> please look"));
        assert!(out.contains("--parent c-42"));
        assert!(out.contains("./reply.md"));
    }

    #[test]
    fn side_chat_keeps_discussion_isolated_and_owns_delivery_decision() {
        let mut t = base_task();
        t.trigger_comment_id = "comment-1".into();
        t.trigger_comment_content = "Should we change the retry policy?".into();
        t.side_chat_parent_task_id = "main-task-1".into();
        t.side_chat_root_comment_id = "thread-root-1".into();
        let out = build_prompt(t, "claude");
        assert!(out.contains("Patchbay Side Chat"));
        assert!(out.contains("Do not modify files"));
        assert!(out.contains("--thread thread-root-1 --full --output json"));
        assert!(out.contains(
            "patchbay issue message-main main-task-1 --content-stdin"
        ));
        assert!(out.contains(
            "pass the instruction verbatim through the command's stdin"
        ));
        assert!(out.contains("Do not ask the user to copy it"));
    }

    #[test]
    fn message_bus_continuation_names_exact_main_task_and_instruction() {
        let mut t = base_task();
        t.trigger_comment_id = "comment-1".into();
        t.message_bus_parent_task_id = "main-task-1".into();
        t.message_bus_messages = vec![TaskMessageBusMessageData {
            id: "message-1".into(),
            source_task_id: "side-chat-1".into(),
            content: "Add the bounded retry and update its test.".into(),
        }];
        let out = build_prompt(t, "claude");
        assert!(out.contains("Main conversation anchor task: `main-task-1`"));
        assert!(out.contains("From Side Chat task `side-chat-1`"));
        assert!(out.contains("Add the bounded retry"));
    }

    #[test]
    fn multi_thread_coalescing_fans_out() {
        let mut t = base_task();
        t.trigger_comment_id = "t2".to_string();
        t.trigger_thread_id = "t2".to_string();
        t.coalesced_comments = vec![CoalescedCommentData {
            id: "c1".to_string(),
            thread_id: "th1".to_string(),
            content: "first ask".to_string(),
            ..Default::default()
        }];
        let targets = comment_reply_threads(&t);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].parent_id, "c1");
        assert_eq!(targets[1].parent_id, "t2");
        let out = build_prompt(t, "claude");
        assert!(out.contains("DISTINCT threads"));
    }

    #[test]
    fn same_thread_coalescing_stays_single_parent() {
        let mut t = base_task();
        t.trigger_comment_id = "t2".to_string();
        t.trigger_thread_id = "same".to_string();
        t.coalesced_comments = vec![CoalescedCommentData {
            id: "c1".to_string(),
            thread_id: "same".to_string(),
            content: "earlier".to_string(),
            ..Default::default()
        }];
        assert!(comment_reply_threads(&t).is_empty());
    }

    #[test]
    fn autopilot_branch() {
        let mut t = base_task();
        t.autopilot_run_id = "ap-1".to_string();
        t.autopilot_title = "Nightly sweep".to_string();
        let out = build_prompt(t, "claude");
        assert!(out.contains("Autopilot run ID: ap-1"));
        assert!(out.contains("Nightly sweep"));
        assert!(!out.contains("`patchbay issue get"));
    }

    #[test]
    fn squad_leader_capability_gate() {
        let mut t = base_task();
        // Legacy: briefing marker sniff when capability absent.
        t.agent = Some(AgentData {
            instructions: "x\n## Squad Operating Protocol\ny".to_string(),
            ..Default::default()
        });
        assert!(task_is_squad_leader(&t));
        // Capability present but flags false → worker.
        t.leader_role_resolved = true;
        assert!(!task_is_squad_leader(&t));
        t.is_leader_task = true;
        assert!(task_is_squad_leader(&t));
    }

    #[test]
    fn continuity_notice_selection() {
        // Issue surface.
        let t = base_task();
        assert_eq!(
            session_continuity_notice_for(&t),
            session_continuity_notice_issue()
        );
        // Slack.
        let mut s = base_task();
        s.chat_session_id = "cs".into();
        s.chat_channel_type = CHANNEL_TYPE_SLACK.into();
        assert_eq!(
            session_continuity_notice_for(&s),
            session_continuity_notice_channel_history()
        );
        // Transcript-persisting surface (feishu).
        let mut f = base_task();
        f.chat_session_id = "cs".into();
        f.chat_channel_type = "feishu".into();
        assert_eq!(
            session_continuity_notice_for(&f),
            session_continuity_notice_chat_transcript()
        );
        // Backend suppression when the prompt already carries the notice.
        f.prior_session_resume_unavailable = true;
        assert!(backend_resume_continuity_notice(&f).is_empty());
    }
}
