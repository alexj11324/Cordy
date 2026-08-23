//! Port of `server/internal/daemon/thread_name.go` (lines 1–40).
//!
//! Derives the Codex thread name shown for a task: the first non-empty
//! candidate among the task's thread name, autopilot title, quick-create
//! prompt, chat message, and trigger comment content — whitespace-collapsed
//! and rune-truncated to [`CODEX_THREAD_NAME_MAX_RUNES`].
//!
//! Deviations from Go:
//! - Go's `Task` struct lives in types.go (lane A1's `types.rs`); until that
//!   lands this module takes a minimal source struct with the five fields the
//!   derivation reads.

// S9-integration: replace `ThreadNameSource` with `crate::types::Task` once
// lane A1 lands types.rs; silence dead-code until daemon wiring consumes it.
#![allow(dead_code)]

/// `codexThreadNameMaxRunes` (thread_name.go:5).
pub(crate) const CODEX_THREAD_NAME_MAX_RUNES: usize = 120;

/// The five `Task` fields `deriveTaskThreadName` reads (thread_name.go:8–14).
#[derive(Debug, Clone, Default)]
pub(crate) struct ThreadNameSource {
    pub thread_name: String,
    pub autopilot_title: String,
    pub quick_create_prompt: String,
    pub chat_message: String,
    pub trigger_comment_content: String,
}

/// `deriveTaskThreadName` (thread_name.go:7–21): first candidate whose
/// normalized form is non-empty wins.
pub(crate) fn derive_task_thread_name(task: &ThreadNameSource) -> String {
    let candidates = [
        &task.thread_name,
        &task.autopilot_title,
        &task.quick_create_prompt,
        &task.chat_message,
        &task.trigger_comment_content,
    ];
    for candidate in candidates {
        if let Some(name) = normalize_thread_name(candidate, CODEX_THREAD_NAME_MAX_RUNES) {
            return name;
        }
    }
    String::new()
}

/// `normalizeThreadName` (thread_name.go:23–40): collapse whitespace runs to
/// single spaces (`strings.Fields` + join), then truncate to `max_runes`
/// runes, appending `"..."` when truncating. Returns `None` when nothing
/// survives normalization (Go returns "").
///
/// `max_runes == 0` disables truncation; `max_runes <= 3` truncates without
/// the ellipsis marker.
fn normalize_thread_name(s: &str, max_runes: usize) -> Option<String> {
    // strings.Fields splits around Unicode whitespace; split_whitespace has
    // the same semantics.
    let normalized = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    if max_runes == 0 {
        return Some(normalized);
    }
    let rune_count = normalized.chars().count();
    if rune_count <= max_runes {
        return Some(normalized);
    }
    if max_runes <= 3 {
        return Some(normalized.chars().take(max_runes).collect());
    }
    let mut out: String = normalized.chars().take(max_runes - 3).collect();
    out.push_str("...");
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_prefers_claimed_thread_name() {
        let got = derive_task_thread_name(&ThreadNameSource {
            thread_name: "  Fix login redirect  ".into(),
            trigger_comment_content: "please look at this comment".into(),
            chat_message: "chat fallback".into(),
            ..Default::default()
        });
        assert_eq!(got, "Fix login redirect");
    }

    #[test]
    fn derive_falls_back_to_task_context() {
        let got = derive_task_thread_name(&ThreadNameSource {
            quick_create_prompt: "create issue for billing sync".into(),
            ..Default::default()
        });
        assert_eq!(got, "create issue for billing sync");
    }

    #[test]
    fn normalize_collapses_whitespace_and_truncates() {
        let input = format!(
            "first line\n\t{}",
            "x".repeat(CODEX_THREAD_NAME_MAX_RUNES + 20)
        );
        let got = normalize_thread_name(&input, CODEX_THREAD_NAME_MAX_RUNES).unwrap();
        assert!(!got.contains('\n') && !got.contains('\t'));
        assert_eq!(got.chars().count(), CODEX_THREAD_NAME_MAX_RUNES);
        assert!(got.ends_with("..."));
    }

    #[test]
    fn normalize_empty_returns_none() {
        assert_eq!(normalize_thread_name("  \n\t ", 120), None);
        assert_eq!(normalize_thread_name("", 120), None);
    }

    #[test]
    fn normalize_short_max_no_ellipsis() {
        assert_eq!(normalize_thread_name("abcdef", 3), Some("abc".to_string()));
        // maxRunes <= 0 disables truncation entirely.
        assert_eq!(normalize_thread_name("a b", 0), Some("a b".to_string()));
    }

    #[test]
    fn normalize_multibyte_runes_counted_not_bytes() {
        // 130 CJK runes must truncate at 120 runes, not at byte boundaries.
        let input = "字".repeat(130);
        let got = normalize_thread_name(&input, CODEX_THREAD_NAME_MAX_RUNES).unwrap();
        assert_eq!(got.chars().count(), CODEX_THREAD_NAME_MAX_RUNES);
        assert!(got.ends_with("..."));
        assert_eq!(got.strip_suffix("...").unwrap().chars().count(), 117);
    }
}
