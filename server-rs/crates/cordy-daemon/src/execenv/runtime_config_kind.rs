//! Port of execenv/runtime_config_kind.go.
//!
//! Symbol map:
//! - taskKind (+kindIssue/kindAutopilotRunOnly/kindQuickCreate/kindChat)
//!    → TaskKind enum
//! - classifyTask                           → classify_task
//! - taskKind.hasIssueContext               → TaskKind::has_issue_context

use super::execenv::TaskContextForEnv;

/// TaskKind labels the dispatch path that the runtime brief should
/// follow for a given TaskContextForEnv. Used by
/// `buildMetaSkillContentSlim` (MUL-3560 brief; the `runtime_brief_slim`
/// flag that once gated it against a legacy verbose brief was retired in
/// MUL-4297, so this is now the only brief).
///
/// Four kinds, mutually exclusive in practice. [`classify_task`] documents the
/// tiebreak rule that applies if a future caller accidentally violates the
/// mutex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// This run operates on a real Cordy issue. It deliberately does NOT
    /// distinguish comment-triggered from assignment-triggered runs.
    ///
    /// Those were two kinds until MUL-5377. Splitting them made the rendered
    /// brief — which Claude Code loads into messages[0], ahead of the entire
    /// conversation — differ between the first (on-assign) run and every
    /// later (comment) run on the same resumed session, which invalidated the
    /// prompt cache for the whole history on every resume. Which trigger
    /// fired THIS turn is per-turn state and now travels in the per-turn user
    /// message (daemon.BuildPrompt), which is appended after the cached
    /// prefix. See runtime_config_sections.go:writeWorkflowIssue.
    Issue,
    /// An autopilot fired in run-only mode (no issue created or attached).
    AutopilotRunOnly,
    /// One-shot "create an issue from a natural-language prompt" task.
    QuickCreate,
    /// Interactive chat session, no issue.
    Chat,
}

/// ClassifyTask maps a TaskContextForEnv to the single taskKind the slim
/// brief should be assembled for. Precedence (documented for the tiebreak
/// case, although the daemon never sets two specific-kind flags at once):
/// chat → quick-create → autopilot run-only → issue.
///
/// Deliberately does not read ctx.trigger_comment_id: the classification must
/// not vary across runs of the same resumed session, or the brief's bytes
/// change and the prompt cache is lost from messages[0] onward (MUL-5377).
pub fn classify_task(ctx: &TaskContextForEnv) -> TaskKind {
    if !ctx.chat_session_id.is_empty() {
        return TaskKind::Chat;
    }
    if !ctx.quick_create_prompt.is_empty() {
        return TaskKind::QuickCreate;
    }
    if !ctx.autopilot_run_id.is_empty() {
        return TaskKind::AutopilotRunOnly;
    }
    TaskKind::Issue
}

impl TaskKind {
    /// HasIssueContext returns true for the kinds that operate on a real Cordy
    /// issue and therefore can read / pin issue-scoped state. The slim
    /// dispatcher gates these two sections on this predicate:
    ///
    ///   - Issue Metadata
    ///   - Sub-issue Creation
    ///
    /// Both are meaningless on the issue-less kinds (chat / quick-create /
    /// autopilot run-only) and would either render an empty body or steer the
    /// agent into a guaranteed-failed CLI call. Note this is a kind-based
    /// predicate, not a check on ctx.issue_id — Issue always carries an issue
    /// id by construction (the daemon refuses to dispatch it otherwise), and
    /// the other three kinds never do.
    pub fn has_issue_context(self) -> bool {
        self == TaskKind::Issue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of TestClassifyTask.
    #[test]
    fn test_classify_task() {
        let cases: Vec<(&str, TaskContextForEnv, TaskKind)> = vec![
            (
                "empty context",
                TaskContextForEnv::default(),
                TaskKind::Issue,
            ),
            (
                "issue",
                TaskContextForEnv {
                    issue_id: "iss_1".into(),
                    ..Default::default()
                },
                TaskKind::Issue,
            ),
            (
                "chat wins over everything",
                TaskContextForEnv {
                    chat_session_id: "chat_1".into(),
                    quick_create_prompt: "make".into(),
                    autopilot_run_id: "run_1".into(),
                    ..Default::default()
                },
                TaskKind::Chat,
            ),
            (
                "quick create beats autopilot",
                TaskContextForEnv {
                    quick_create_prompt: "make".into(),
                    autopilot_run_id: "run_1".into(),
                    ..Default::default()
                },
                TaskKind::QuickCreate,
            ),
            (
                "autopilot run only",
                TaskContextForEnv {
                    autopilot_run_id: "run_1".into(),
                    ..Default::default()
                },
                TaskKind::AutopilotRunOnly,
            ),
        ];
        for (name, ctx, want) in cases {
            assert_eq!(classify_task(&ctx), want, "case {name}");
        }
    }

    // Port of TestTaskKindHasIssueContext.
    #[test]
    fn test_task_kind_has_issue_context() {
        assert!(TaskKind::Issue.has_issue_context());
        assert!(!TaskKind::Chat.has_issue_context());
        assert!(!TaskKind::QuickCreate.has_issue_context());
        assert!(!TaskKind::AutopilotRunOnly.has_issue_context());
    }
}
