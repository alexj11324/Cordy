//! Squad leader briefing — port of `server/internal/handler/squad_briefing.go`.
//! Composes the full system briefing appended to a squad leader's Instructions
//! when it claims a task on a squad-assigned issue: Operating Protocol (system
//! rules), Squad Roster (data, with literal mention markdown), and the squad's
//! own Instructions. Byte-stable with the Go text — contract tests on both
//! sides pin the compositions.

use cordy_db::models::{Agent, Squad, SquadMember, User};
use cordy_db::queries::{agent as agent_q, skill as skill_q, squad as squad_q, user as user_q};
use sqlx::PgPool;

const SQUAD_OPERATING_PROTOCOL_HEADER: &str = r#"## Squad Operating Protocol

**If you are reading this section, you have been activated as a squad LEADER
for this task — regardless of how the work reached you (direct assignment,
an @squad mention in a comment, quick-create, or autopilot).** Your job is to
**coordinate**, NOT to do the work yourself. Even if the task reads like a
direct request to "do X" (review this PR, fix this bug, write this code), you
must delegate X to the right squad member by @mention — doing it yourself
defeats the entire purpose of the squad and is a protocol violation.

Your responsibilities, in order:

1. **Read the issue** (title, description, latest comments, acceptance
   criteria) and decide which squad member is best suited to do the work.
   Match the task to each member's listed **skills** and role in the Squad
   Roster below — prefer the member whose skills cover the work.
2. **Delegate by @mention.** Post a single comment on this issue that
   @mentions the chosen member(s) and tells them what to do.
   - **Be terse.** Every Cordy agent already has full context of the
     issue (title, description, all prior comments, attachments) and
     the surrounding workspace. Do NOT restate or summarise the
     issue body, prior discussion, or known facts in your delegation
     comment — they read it themselves.
   - Say only what cannot be inferred from the issue: who you're
     picking, why them (one short clause), and any *additional*
     constraints, hints, or sequencing you want them to follow.
     Two or three sentences is usually plenty.
   - Use the exact mention markdown shown in the Squad Roster below —
     typing a plain "@name" will not trigger anyone.
3. **Record your evaluation.** After every trigger — whether you delegated,
   decided no action is needed, or encountered an error — record it:
   `cordy squad activity <issue-id> <outcome> --reason "<short reason>"`
   Outcome values: `action` (you delegated or acted),
   `no_action` (you evaluated and decided nothing is needed),
   `failed` (you hit an error).
   This is mandatory on every turn — it records your decision in the
   issue timeline so humans can see you evaluated the trigger.
4. **Stop after dispatching.** Once your delegation comment is posted
   and evaluation recorded, end your turn. Do not continue working,
   do not write code, do not open files. You will be re-triggered
   automatically when:
   - a delegated member posts an update or asks you a question;
   - a delegated member finishes and the issue moves forward;
   - someone @mentions you again on this issue.
5. **Re-evaluate on each trigger.** When you wake up again, read the new
   activity and decide whether to delegate the next step, escalate to
   the human reporter, or close the loop. If no action is needed
   (e.g. a member posted a progress update that requires no response),
   record `no_action` and exit silently."#;

/// Responsibility 6 for the case where the issue this leader was woken on is
/// assigned to THIS squad (Go `squadParentStatusOwned`). Only then does the
/// leader own the parent's status arc.
const SQUAD_PARENT_STATUS_OWNED: &str = r#"6. **Own the parent issue status.** This issue is assigned to your squad,
   so its status is yours to manage (unless Agent Identity forbids status
   changes). On the first assignment turn, move the parent to
   `in_progress` and keep it there while members work — a successful
   dispatch is not completion. On later turns, do not flip status for
   routine progress updates. When you confirm the overall goal is met, run
   `cordy issue status <issue-id> in_review` — this responsibility is
   itself the standing instruction that authorizes that change, so do it even
   when no comment asked you to. Leave `done` to a human reviewer or
   existing integrations (for example a PR with close intent that merges)."#;

/// Responsibility 6 for every other leader path (Go
/// `squadParentStatusNotOwned`): an @squad mention on an issue owned by someone
/// else (MUL-3724), and quick-create, where no issue exists yet on this turn.
const SQUAD_PARENT_STATUS_NOT_OWNED: &str = r#"6. **Do NOT change this issue's status.** This issue is not assigned to your
   squad — you were pulled in by an @mention (or this is a quick-create turn,
   where the issue does not exist yet). Its status belongs to its own
   assignee. Answer, delegate, or escalate as usual, but never run
   `cordy issue status` on it, no matter how complete the work looks
   to you."#;

const SQUAD_OPERATING_PROTOCOL_HARD_RULES: &str = r#"Hard rules:
- EVERY delegation MUST use the full mention markdown syntax
  `[@Name](mention://<type>/<UUID>)` exactly as shown in the Squad
  Roster. A plain "@name" or bare name does NOT trigger the agent —
  if you skip the mention link, the task is never delivered and the
  issue stalls. This is non-negotiable: no mention link = no delegation.
- Do NOT restate the issue body or prior comments in your delegation —
  the assignee already has them. Repeating context is noise that
  buries the actual instruction.
- Do NOT do the implementation work yourself unless the squad has no
  other suitable members. The squad exists so work is split — bypassing
  it defeats the point.
- Do NOT @mention members who don't appear in the Squad Roster below;
  they are not part of this squad.
- One delegation comment per turn is enough. Avoid spamming multiple
  near-identical comments.
- If the squad has no member capable of the task, post a comment
  explaining the gap (and @mention the issue's reporter if possible)
  rather than silently doing the work.
- ALWAYS call `cordy squad activity` before ending your turn —
  even when the outcome is no_action.
- A child issue you create with `--status todo` and an agent assignee
  already fires that agent automatically — the assignment IS the trigger.
  If you also @mention the same agent on this parent issue for the same
  work, the agent runs twice in parallel (once from the mention, once
  from the assignment). Pick exactly one path: either delegate by
  @mention on this issue, or create a `todo` child issue assigned to
  them. Never both for the same work."#;

/// Go `squadOperatingProtocolFor`: assembles the protocol, selecting the
/// parent-status responsibility that matches this leader's actual authority
/// over the issue.
fn squad_operating_protocol_for(owns_issue_status: bool) -> String {
    let status = if owns_issue_status {
        SQUAD_PARENT_STATUS_OWNED
    } else {
        SQUAD_PARENT_STATUS_NOT_OWNED
    };
    format!("{SQUAD_OPERATING_PROTOCOL_HEADER}\n{status}\n\n{SQUAD_OPERATING_PROTOCOL_HARD_RULES}")
}

/// Go `formatMention`: emits a mention markdown string that round-trips through
/// util.ParseMentions.
fn format_mention(name: &str, mention_type: &str, id: &str) -> String {
    format!("[@{name}](mention://{mention_type}/{id})")
}

/// Go `agentSkillsRosterSegment`: "skills: a, b" when the agent has skills
/// (names pre-sorted), "no skills assigned" when it has none, "" only when the
/// lookup failed — degrading to a name+role row rather than asserting a
/// misleading "no skills".
fn agent_skills_roster_segment(
    skills_by_agent: &Option<std::collections::HashMap<String, Vec<String>>>,
    agent_id: &str,
) -> String {
    let Some(map) = skills_by_agent else {
        return String::new();
    };
    match map.get(agent_id) {
        None => "no skills assigned".to_string(),
        Some(names) if names.is_empty() => "no skills assigned".to_string(),
        Some(names) => format!("skills: {}", names.join(", ")),
    }
}

/// Go `formatRosterRow`.
fn format_roster_row(name: &str, kind: &str, role: &str, skills: &str, mention: &str) -> String {
    let mut out = format!("- {name} — {kind}");
    if !role.is_empty() {
        out.push_str(", role: \"");
        out.push_str(role);
        out.push('"');
    }
    if !skills.is_empty() {
        out.push_str(" — ");
        out.push_str(skills);
    }
    out.push_str(" — `");
    out.push_str(mention);
    out.push_str("`\n");
    out
}

/// Go `loadSquadMemberSkillNames`: one batched query for every non-leader agent
/// member's enabled skill names, keyed by agent id. `None` means the lookup
/// failed (transient DB error → rows render without a skills segment).
async fn load_squad_member_skill_names(
    pool: &PgPool,
    members: &[SquadMember],
    leader_id: &str,
) -> Option<std::collections::HashMap<String, Vec<String>>> {
    let mut ids: Vec<uuid::Uuid> = Vec::new();
    for m in members {
        if m.member_type != "agent" {
            continue;
        }
        let id = m.member_id.to_string();
        if id == leader_id {
            continue;
        }
        if ids.contains(&m.member_id) {
            continue;
        }
        ids.push(m.member_id);
    }
    if ids.is_empty() {
        return Some(std::collections::HashMap::new());
    }
    match skill_q::list_agent_skill_names_by_agent_i_ds(pool, ids).await {
        Ok(rows) => {
            let mut out: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for row in rows {
                let id = row.agent_id.map(|u| u.to_string()).unwrap_or_default();
                out.entry(id).or_default().push(row.name);
            }
            Some(out)
        }
        Err(e) => {
            tracing::warn!(error = %e, "squad roster: load member skill names failed");
            None
        }
    }
}

/// Go `renderMemberRow`: renders a single roster row, returning "" if the
/// member can't be resolved or should be skipped (e.g. archived agent).
async fn render_member_row(
    pool: &PgPool,
    m: &SquadMember,
    skills_by_agent: &Option<std::collections::HashMap<String, Vec<String>>>,
) -> String {
    let id = m.member_id.to_string();
    let role = m.role.trim();
    match m.member_type.as_str() {
        "agent" => {
            let Ok(Some(ag)) = agent_q::get_agent(pool, m.member_id).await else {
                return String::new();
            };
            if ag.archived_at.is_some() {
                return String::new();
            }
            // Agents carry skills; surfacing them lets the leader delegate by
            // capability instead of guessing from the free-text role label.
            let Agent { name, .. } = ag;
            let skills = agent_skills_roster_segment(skills_by_agent, &id);
            format_roster_row(
                &name,
                "agent",
                role,
                &skills,
                &format_mention(&name, "agent", &id),
            )
        }
        "member" => {
            let Ok(Some(User { name, .. })) = user_q::get_user(pool, m.member_id).await else {
                return String::new();
            };
            // Mention syntax for humans uses the user_id (matches the rest of
            // the product). Humans have no Cordy skills, so no skills segment.
            format_roster_row(
                &name,
                "member (human)",
                role,
                "",
                &format_mention(&name, "member", &id),
            )
        }
        _ => String::new(),
    }
}

/// Go `buildSquadRoster`: renders the "## Squad Roster" section — a leader
/// self-row plus one row per non-archived member, with literal mention
/// markdown.
async fn build_squad_roster(pool: &PgPool, squad: &Squad) -> String {
    let mut out = String::from("## Squad Roster\n\n");

    // Leader self-row. Leaders are always agents (FK enforced in schema).
    let mut leader_name = "Leader".to_string();
    if let Ok(Some(leader)) = agent_q::get_agent(pool, squad.leader_id).await {
        let Agent { name, .. } = leader;
        leader_name = name;
    }
    out.push_str("Leader (you):\n");
    out.push_str(&format!(
        "- {leader_name} — agent — `{}`\n",
        format_mention(&leader_name, "agent", &squad.leader_id.to_string())
    ));

    let members = squad_q::list_squad_members(pool, squad.id)
        .await
        .unwrap_or_default();

    let skills_by_agent =
        load_squad_member_skill_names(pool, &members, &squad.leader_id.to_string()).await;

    let mut rows: Vec<String> = Vec::with_capacity(members.len());
    for m in &members {
        // Skip the leader if they happen to also be in the member list —
        // they're already shown above and we don't want self-delegation.
        if m.member_type == "agent" && m.member_id == squad.leader_id {
            continue;
        }
        let row = render_member_row(pool, m, &skills_by_agent).await;
        if !row.is_empty() {
            rows.push(row);
        }
    }

    if rows.is_empty() {
        out.push_str("\nMembers: (none — you are the only member of this squad)\n");
        return out;
    }

    out.push_str("\nMembers:\n");
    for r in rows {
        out.push_str(&r);
    }
    out
}

/// Port of Go `buildSquadLeaderBriefing`. The returned string contains three
/// sections: Squad Operating Protocol (constant), Squad Roster (data), and
/// Squad Instructions (user-defined, omitted when empty).
///
/// `owns_issue_status` must be true only when the issue this task is bound to
/// is assigned to this very squad — it keeps status authority from leaking
/// along with the roster.
pub async fn build_squad_leader_briefing(
    pool: &PgPool,
    squad: &Squad,
    owns_issue_status: bool,
) -> String {
    let mut out = squad_operating_protocol_for(owns_issue_status);
    out.push_str("\n\n");
    out.push_str(&build_squad_roster(pool, squad).await);

    let trimmed = squad.instructions.trim();
    if !trimmed.is_empty() {
        out.push_str("\n\n## Squad Instructions (");
        out.push_str(&squad.name);
        out.push_str(")\n\n");
        out.push_str(trimmed);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_selects_parent_status_section() {
        let owned = squad_operating_protocol_for(true);
        assert!(owned.starts_with("## Squad Operating Protocol\n"));
        assert!(owned.contains("**Own the parent issue status.**"));
        assert!(owned.ends_with("Never both for the same work."));

        let guest = squad_operating_protocol_for(false);
        assert!(guest.contains("**Do NOT change this issue's status.**"));
        assert!(!guest.contains("**Own the parent issue status.**"));
    }

    #[test]
    fn mention_markdown_round_trips_shape() {
        assert_eq!(
            format_mention("Mika", "agent", "abc"),
            "[@Mika](mention://agent/abc)"
        );
    }

    #[test]
    fn roster_row_formats_role_and_skills() {
        let row = format_roster_row("Mika", "agent", "lead", "skills: go, rust", "@x");
        assert_eq!(
            row,
            "- Mika — agent, role: \"lead\" — skills: go, rust — `@x`\n"
        );
        let bare = format_roster_row("Ann", "member (human)", "", "", "@y");
        assert_eq!(bare, "- Ann — member (human) — `@y`\n");
    }
}
