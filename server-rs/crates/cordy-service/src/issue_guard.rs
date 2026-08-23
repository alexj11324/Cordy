//! Active-duplicate guard — port of `server/internal/issueguard/duplicate.go`.
//!
//! Both entry points take a transaction-scoped executor: the advisory lock
//! (`pg_advisory_xact_lock`) only lives until the surrounding transaction
//! ends, so calling these with a bare pool would release the lock before the
//! guarded insert runs and the guard would guard nothing.

use chrono::Utc;
use uuid::Uuid;

/// Lowercased, whitespace-collapsed title used for duplicate matching. Must
/// stay byte-identical to the SQL side's
/// `lower(btrim(regexp_replace(title, '[[:space:]]+', ' ', 'g')))` so the
/// lock key and the lookup row agree.
pub fn normalize_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn duplicate_message(identifier: &str, title: &str, status: &str) -> String {
    format!(
        "Active duplicate issue exists: {identifier} {title} (status: {status}). Set allow_duplicate=true or use --allow-duplicate to create another."
    )
}

/// Signals that the duplicate guard found an active issue with the same
/// (workspace, project, parent, title) tuple and allowDuplicate was false.
#[derive(Debug, Clone)]
pub struct ActiveDuplicateError {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub status: String,
}

impl std::fmt::Display for ActiveDuplicateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&duplicate_message(
            &self.identifier,
            &self.title,
            &self.status,
        ))
    }
}

impl std::error::Error for ActiveDuplicateError {}

/// Builds the error from the blocking issue row; callers render it as HTTP
/// 409 / Lark card / CLI message.
pub fn new_active_duplicate_error(
    issue: &cordy_db::models::Issue,
    issue_prefix: &str,
) -> ActiveDuplicateError {
    ActiveDuplicateError {
        id: issue.id.to_string(),
        identifier: format!("{issue_prefix}-{}", issue.number),
        title: issue.title.clone(),
        status: issue.status.clone(),
    }
}

fn lock_key(
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    parent_issue_id: Option<Uuid>,
    normalized_title: &str,
) -> String {
    [
        "issue-active-duplicate",
        &workspace_id.to_string(),
        &project_id.map(|u| u.to_string()).unwrap_or_default(),
        &parent_issue_id.map(|u| u.to_string()).unwrap_or_default(),
        normalized_title,
    ]
    .join("|")
}

fn recent_autopilot_lock_key(
    workspace_id: Uuid,
    autopilot_id: Uuid,
    project_id: Option<Uuid>,
    normalized_title: &str,
) -> String {
    [
        "autopilot-recent-duplicate",
        &workspace_id.to_string(),
        &autopilot_id.to_string(),
        &project_id.map(|u| u.to_string()).unwrap_or_default(),
        normalized_title,
    ]
    .join("|")
}

/// Takes the workspace-scoped advisory duplicate lock and, unless
/// `allow_duplicate`, looks up an active issue with the same identity tuple.
///
/// Returns `(duplicate, found)`; `found == true` means the caller must abort
/// the create (the transaction-scoped lock stays held until rollback).
// Concrete connection signature: the advisory xact lock only holds inside a
// transaction, so a bare pool would silently degrade the guard.
pub async fn lock_and_find_active_duplicate(
    executor: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    parent_issue_id: Option<Uuid>,
    title: &str,
    allow_duplicate: bool,
) -> anyhow::Result<(Option<cordy_db::models::Issue>, bool)> {
    let normalized_title = normalize_title(title);
    if normalized_title.is_empty() {
        return Ok((None, false));
    }
    cordy_db::queries::issue::lock_issue_duplicate_key(
        &mut *executor,
        &lock_key(workspace_id, project_id, parent_issue_id, &normalized_title),
    )
    .await?;
    if allow_duplicate {
        return Ok((None, false));
    }

    let duplicate = cordy_db::queries::issue::find_active_duplicate_issue(
        &mut *executor,
        workspace_id,
        project_id,
        parent_issue_id,
        &normalized_title,
    )
    .await?;
    let found = duplicate.is_some();
    Ok((duplicate, found))
}

/// Autopilot variant: blocks re-creating an issue with the same normalized
/// title that this autopilot already created inside `window`. Empty titles,
/// invalid autopilot ids and non-positive windows are no-ops.
// Concrete connection signature: the advisory xact lock only holds inside a
// transaction, so a bare pool would silently degrade the guard.
pub async fn lock_and_find_recent_autopilot_duplicate(
    executor: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    autopilot_id: Option<Uuid>,
    project_id: Option<Uuid>,
    title: &str,
    window: chrono::Duration,
) -> anyhow::Result<(Option<cordy_db::models::Issue>, bool)> {
    let Some(autopilot_id) = autopilot_id else {
        return Ok((None, false));
    };
    let normalized_title = normalize_title(title);
    if normalized_title.is_empty() || window <= chrono::Duration::zero() {
        return Ok((None, false));
    }
    cordy_db::queries::issue::lock_issue_duplicate_key(
        &mut *executor,
        &recent_autopilot_lock_key(workspace_id, autopilot_id, project_id, &normalized_title),
    )
    .await?;

    let created_after = Utc::now() - window;
    let duplicate = cordy_db::queries::issue::find_recent_autopilot_duplicate_issue(
        &mut *executor,
        workspace_id,
        autopilot_id,
        project_id,
        &normalized_title,
        Some(created_after),
    )
    .await?;
    let found = duplicate.is_some();
    Ok((duplicate, found))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_matches_the_sql_side_expression() {
        // lower(btrim(regexp_replace(title, '[[:space:]]+', ' ', 'g')))
        assert_eq!(
            normalize_title("  Fix   the\tLOGIN\nbug  "),
            "fix the login bug"
        );
        assert_eq!(normalize_title("ALREADY NORMAL"), "already normal");
        assert_eq!(normalize_title("   "), "");
        assert_eq!(normalize_title(""), "");
    }

    #[test]
    fn duplicate_message_shape_is_stable() {
        let m = duplicate_message("CORDY-12", "Fix bug", "todo");
        assert_eq!(
            m,
            "Active duplicate issue exists: CORDY-12 Fix bug (status: todo). Set allow_duplicate=true or use --allow-duplicate to create another."
        );
    }

    #[test]
    fn active_duplicate_error_displays_the_message() {
        let e = ActiveDuplicateError {
            id: "0197".into(),
            identifier: "CORDY-12".into(),
            title: "Fix bug".into(),
            status: "todo".into(),
        };
        assert_eq!(
            e.to_string(),
            duplicate_message("CORDY-12", "Fix bug", "todo")
        );
    }

    #[test]
    fn lock_keys_scope_by_every_identity_component() {
        let ws = Uuid::now_v7();
        let proj = Uuid::now_v7();
        let parent = Uuid::now_v7();
        let k1 = lock_key(ws, Some(proj), Some(parent), "t");
        // NULL project/parent render as empty segments — same shape Go's
        // UUIDToString produces for invalid pgtype.UUID.
        let k2 = lock_key(ws, None, None, "t");
        assert_ne!(k1, k2);
        assert!(k1.starts_with("issue-active-duplicate|"));
        assert_eq!(k1.matches('|').count(), 4);

        let ap = Uuid::now_v7();
        let rk = recent_autopilot_lock_key(ws, ap, Some(proj), "t");
        assert!(rk.starts_with("autopilot-recent-duplicate|"));
        assert_ne!(rk, k1);
    }
}
