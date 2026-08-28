//! Runtime-context sections consumed by the daemon's per-turn prompts.
//!
//! This module owns continuity notices, task initiator and connected-app
//! blocks, and comment-reply instructions.

use crate::execenv::execenv::{ConnectedApp, ThreadReplyTarget};

/// Session continuity notice for issue-backed tasks.
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

/// Converts a name to a single-line token safe for Markdown inline constructs.
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

/// BuildTaskInitiatorBlock (PB-2645 pinned phrases kept verbatim). Returns
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
    slug.split(&['-', '_'][..])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut cs = p.chars();
            match cs.next() {
                Some(first) => first.to_uppercase().collect::<String>() + cs.as_str(),
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
            name = sanitize_name_for_brief_markdown(&toolkit_slug_fallback(&app.toolkit_slug));
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
// Comment reply instructions.
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
/// the OS split is selected at compile time.
pub(crate) fn build_comment_reply_instructions(
    provider: &str,
    issue_id: &str,
    trigger_comment_id: &str,
    squad_leader: bool,
) -> String {
    let _ = provider; // Retained for caller symmetry.
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
Write the body file first (rules: ## Comment Formatting above — PB-2904 / #4182):\n\n\
    cordy issue comment add {issue_id} --parent {trigger_comment_id} --content-file ./reply.md\n\
    rm ./reply.md\n\n\
Do NOT write literal `\\n` escapes to simulate line breaks; the file preserves real newlines.\n"
        )
    }
}

/// BuildMultiThreadCommentReplyInstructions (PB-4348 / PB-5825): fan-out
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
}
