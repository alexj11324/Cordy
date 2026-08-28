//! Per-workspace issue status catalog — port of
//! Per-workspace issue-status catalog (MUL-6243).
//!
//! MODEL. There are 7 categories and they map one-to-one onto the 7 built-in
//! statuses: a category's value IS its canonical status key. A custom status
//! declares a category and inherits that canonical's platform behavior in
//! full.
//!
//! That one-to-one correspondence is what makes this cheap. Category is
//! defined as a BEHAVIOR EQUIVALENCE CLASS — two statuses share a category
//! only when the platform treats them identically — and by that definition
//! the 7 built-ins are 7 distinct classes.
//!
//! CONSEQUENCES. [`effective`] is the identity function on built-in keys, so
//! every existing `issue.status == "todo"` comparison keeps its exact meaning
//! and `issue.status` stays the authoritative TEXT column.

use cordy_db::models::IssueStatus;
use sqlx::Executor;
use uuid::Uuid;

/// The 7 canonical status keys. Each is simultaneously a status key and the
/// name of the category it defines.
pub const BACKLOG: &str = "backlog";
pub const TODO: &str = "todo";
pub const IN_PROGRESS: &str = "in_progress";
pub const IN_REVIEW: &str = "in_review";
pub const DONE: &str = "done";
pub const BLOCKED: &str = "blocked";
pub const CANCELLED: &str = "cancelled";

/// Categories that represent work already underway. Issues in these columns
/// must always have a concrete owner so progress, blockers, and review cannot
/// become an ownerless queue.
pub fn requires_assignee(category: &str) -> bool {
    matches!(category, IN_PROGRESS | IN_REVIEW | BLOCKED)
}

/// The historical STATUS_ORDER from the frontend's static status config.
/// Category ranking copies it verbatim so a workspace with no custom statuses
/// sees a board and picker identical to before this feature.
///
/// Note the order is NOT grouped by lifecycle: in_review and done sit between
/// in_progress and blocked. That is the shipped order, and reordering it to
/// look tidier would visibly rearrange every existing user's board.
pub const CANONICAL_ORDER: [&str; 7] = [
    BACKLOG,
    TODO,
    IN_PROGRESS,
    IN_REVIEW,
    DONE,
    BLOCKED,
    CANCELLED,
];

fn canonical_rank(key: &str) -> Option<usize> {
    CANONICAL_ORDER.iter().position(|k| *k == key)
}

/// Returned when a status key is absent from a workspace's catalog, or
/// present but archived.
#[derive(Debug, thiserror::Error)]
#[error("unknown issue status")]
pub struct UnknownStatus;

/// Returns the 7 built-in status keys in display order.
pub fn canonical() -> Vec<&'static str> {
    CANONICAL_ORDER.to_vec()
}

/// Reports whether key is one of the 7 canonical statuses.
pub fn is_built_in(key: &str) -> bool {
    canonical_rank(key).is_some()
}

/// Reports whether value names a valid category. Identical to [`is_built_in`]
/// by construction — categories and canonical keys are the same set — and
/// exists so calling code can say which of the two it means.
pub fn is_category(value: &str) -> bool {
    is_built_in(value)
}

/// Returns the display rank of a category, or `CANONICAL_ORDER.len()` for an
/// unrecognized one so it sorts last instead of colliding with rank 0.
pub fn category_rank(category: &str) -> usize {
    canonical_rank(category).unwrap_or(CANONICAL_ORDER.len())
}

/// Checks a proposed custom status key against the storage constraint and the
/// reserved built-in names.
pub fn validate_key(key: &str) -> Result<String, String> {
    let key = key.trim().to_lowercase();
    if key.is_empty() {
        return Err("status key is required".to_string());
    }
    // Mirrors the issue_status.key CHECK constraint. Keys are lowercase so
    // `cordy issue status <id> human_review` is unambiguous to type.
    let valid = {
        let bytes = key.as_bytes();
        let first_ok = bytes
            .first()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        let rest_ok = bytes
            .iter()
            .skip(1)
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_');
        first_ok && rest_ok && key.len() <= 32
    };
    if !valid {
        return Err("status key must be 1-32 characters of lowercase letters, digits or underscore, starting with a letter or digit".to_string());
    }
    if is_built_in(&key) {
        return Err(format!(
            "{key:?} is a built-in status key and cannot be reused"
        ));
    }
    Ok(key)
}

/// Derives a candidate key from a display name, for callers that let an admin
/// type only a name. Returns an error when nothing usable survives.
pub fn slugify_key(name: &str) -> Result<String, String> {
    let mut b = String::new();
    let mut last_underscore = false;
    for r in name.trim().to_lowercase().chars() {
        if r.is_ascii_lowercase() || r.is_ascii_digit() {
            b.push(r);
            last_underscore = false;
        } else if !last_underscore && !b.is_empty() {
            b.push('_');
            last_underscore = true;
        }
    }
    let mut slug = b.trim_matches('_').to_string();
    if slug.len() > 32 {
        slug = slug[..32].trim_matches('_').to_string();
    }
    if slug.is_empty() {
        return Err(
            "cannot derive a status key from that name; provide one explicitly".to_string(),
        );
    }
    validate_key(&slug)
}

/// Idempotently seeds a workspace's 7 built-in statuses. Safe to call
/// concurrently — the unique (workspace_id, key) index turns a losing racer
/// into a no-op, which matters during a rolling deploy where an old pod may
/// create a workspace while a new pod seeds it.
pub async fn ensure<'e, E>(executor: E, workspace_id: Uuid) -> anyhow::Result<()>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    cordy_db::queries::issue_status::seed_issue_status_entries(executor, workspace_id).await?;
    Ok(())
}

/// Maps a status key to the canonical key whose platform behavior it carries.
/// This is THE function that keeps existing logic correct:
///
///   - a built-in key returns itself, unchanged, WITHOUT touching the
///     database, so no existing code path gains a query or changes behavior;
///   - a custom key returns its category, i.e. the canonical status it
///     inherits.
///
/// On an unresolvable key it returns the key unchanged. That is the fail-safe
/// direction: an unrecognized status matches none of the canonical
/// comparisons, so the issue is left alone rather than being swept,
/// auto-triggered, or having its autopilot run finalized on a guess.
pub async fn effective<'e, E>(executor: E, workspace_id: Uuid, status: &str) -> String
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    if is_built_in(status) {
        return status.to_string();
    }
    match cordy_db::queries::issue_status::get_issue_status_entry_by_key(
        executor,
        workspace_id,
        status,
    )
    .await
    {
        Ok(Some(entry)) => {
            if is_category(&entry.category) {
                entry.category
            } else {
                status.to_string()
            }
        }
        _ => status.to_string(),
    }
}

/// Validates that status is usable in this workspace, returning the catalog
/// entry. Write paths use this; it is the application-layer replacement for
/// the enum CHECK that migration 337 dropped.
///
/// The 7 built-in keys resolve even when the workspace has no catalog row for
/// them. The catalog EXTENDS the built-in statuses; it does not define them,
/// and a workspace has always been able to use all 7. Requiring a row would
/// mean a workspace whose seed has not landed yet — created by a pod that
/// predates this feature, or mid-rollout before migration 339 runs — could
/// not create or update an issue at all. Failing open here is limited
/// precisely to the set that was valid before this feature existed; anything
/// else still needs a row.
pub async fn resolve<'e, E>(
    executor: E,
    workspace_id: Uuid,
    status: &str,
) -> Result<IssueStatus, UnknownStatus>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let key = status.trim().to_lowercase();
    if key.is_empty() {
        return Err(UnknownStatus);
    }
    let entry = cordy_db::queries::issue_status::get_issue_status_entry_by_key(
        executor,
        workspace_id,
        &key,
    )
    .await
    .map_err(|_| UnknownStatus)?;
    match entry {
        None => {
            if is_built_in(&key) {
                Ok(built_in_entry(workspace_id, &key))
            } else {
                Err(UnknownStatus)
            }
        }
        Some(entry) => {
            if entry.archived_at.is_some() {
                // A built-in can never be archived (enforced by a table
                // constraint), so reaching here means a custom status was
                // retired.
                return Err(UnknownStatus);
            }
            Ok(entry)
        }
    }
}

/// Synthesizes the catalog row for a built-in status in a workspace whose
/// seed has not landed. It carries the fields that define behavior — key and
/// category, which are equal by construction — and is deliberately not
/// persisted: [`ensure`] owns seeding.
fn built_in_entry(workspace_id: Uuid, key: &str) -> IssueStatus {
    IssueStatus {
        id: Uuid::nil(),
        workspace_id,
        key: key.to_string(),
        name: String::new(),
        description: String::new(),
        category: key.to_string(),
        color: String::new(),
        is_system: true,
        position: 0.0,
        archived_at: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

/// Returns the workspace's usable status keys in display order, for error
/// messages and CLI validation. An unseeded workspace still reports the 7
/// built-ins, so an error message can never omit a key that [`resolve`]
/// accepts.
pub async fn active_keys<'e, E>(executor: E, workspace_id: Uuid) -> anyhow::Result<Vec<String>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let entries =
        cordy_db::queries::issue_status::list_issue_status_entries(executor, workspace_id, false)
            .await?;
    let mut keys: Vec<String> = Vec::with_capacity(entries.len() + CANONICAL_ORDER.len());
    let mut seen = std::collections::HashSet::with_capacity(entries.len());
    for e in &entries {
        keys.push(e.key.clone());
        seen.insert(e.key.clone());
    }
    for key in CANONICAL_ORDER {
        if !seen.contains(key) {
            keys.push(key.to_string());
        }
    }
    Ok(keys)
}

/// Resolves many statuses against ONE workspace's catalog using at most one
/// query, for list endpoints that would otherwise issue a lookup per row.
///
/// The built-in fast path is unchanged: a built-in key returns itself without
/// touching the catalog, so a workspace with no custom statuses still
/// performs zero queries no matter how long the list is. The catalog is
/// fetched lazily on the first custom key and reused for every row after it.
///
/// Not safe for concurrent use, and scoped to a single request: it caches the
/// catalog for its lifetime, so a long-lived resolver would serve stale
/// categories after an admin edits the catalog.
#[derive(Debug)]
pub struct Resolver {
    workspace_id: Uuid,
    categories: Option<std::collections::HashMap<String, String>>,
}

impl Resolver {
    /// Returns a resolver for one workspace. It performs no I/O.
    pub fn new(workspace_id: Uuid) -> Self {
        Self {
            workspace_id,
            categories: None,
        }
    }

    /// Mirrors the package-level [`effective`], but amortizes the catalog
    /// read across every call. Same fail-safe direction: an unresolvable key
    /// is returned unchanged rather than guessed at.
    pub async fn effective<'e, E>(&mut self, executor: E, status: &str) -> String
    where
        E: Executor<'e, Database = sqlx::Postgres>,
    {
        if is_built_in(status) {
            return status.to_string();
        }
        if self.categories.is_none() {
            match cordy_db::queries::issue_status::list_issue_status_entries(
                executor,
                self.workspace_id,
                true,
            )
            .await
            {
                Ok(entries) => {
                    let mut m = std::collections::HashMap::with_capacity(entries.len());
                    for e in entries {
                        m.insert(e.key, e.category);
                    }
                    self.categories = Some(m);
                }
                // Leave the map unset; every custom key then falls back to
                // itself, which is the same fail-safe the single-shot
                // resolver applies.
                Err(_) => self.categories = None,
            }
            // Mark as attempted even on failure? No — Go sets loaded=true
            // BEFORE the query and leaves the map nil on error, so failures
            // are cached too (one failed read per request, not per row).
            // Emulate by leaving categories as an empty map sentinel.
            if self.categories.is_none() {
                self.categories = Some(std::collections::HashMap::new());
            }
        }
        match self.categories.as_ref().and_then(|m| m.get(status)) {
            Some(category) if is_category(category) => category.clone(),
            _ => status.to_string(),
        }
    }
}

/// Turns a set of categories into the status keys that belong to them, for
/// use as an INDEXED `status = ANY(...)` predicate.
///
/// This exists instead of filtering on issue_effective_status(workspace,
/// status): wrapping the column in a function makes the (workspace_id,
/// status) index unusable, turning a two-page index read into a full
/// workspace scan. Expanding first keeps the original access path — and for a
/// workspace with no custom statuses each category expands to exactly its own
/// key, so the query is byte-for-byte the one that ran before this feature.
///
/// Archived statuses are included: archiving stops FUTURE assignment but
/// leaves existing issues in place, and those issues must still appear in
/// their category's column.
///
/// An unseeded workspace yields no rows; the categories themselves are
/// returned in that case, which is correct because a built-in key IS its own
/// category.
pub async fn expand_categories<'e, E>(
    executor: E,
    workspace_id: Uuid,
    categories: &[String],
) -> anyhow::Result<Vec<String>>
where
    E: Executor<'e, Database = sqlx::Postgres> + Copy,
{
    let valid: Vec<&String> = categories.iter().filter(|c| is_category(c)).collect();
    if valid.is_empty() {
        return Ok(Vec::new());
    }
    let valid_refs: Vec<String> = valid.iter().map(|s| (*s).clone()).collect();
    let keys = cordy_db::queries::issue_status::list_issue_status_keys_by_categories(
        executor,
        workspace_id,
        &valid_refs,
    )
    .await?;
    let mut seen = std::collections::HashSet::with_capacity(keys.len() + valid.len());
    let mut out = Vec::with_capacity(keys.len() + valid.len());
    for k in keys {
        if seen.insert(k.clone()) {
            out.push(k);
        }
    }
    // A category always contains at least its own canonical key, even if the
    // catalog row is missing (unseeded workspace, mid-rollout).
    for c in valid {
        if seen.insert((*c).clone()) {
            out.push((*c).clone());
        }
    }
    Ok(out)
}

/// Returns the workspace's CUSTOM status keys mapped to the category each
/// belongs to. Built-ins are deliberately absent: a built-in key IS its own
/// category, so a caller mapping key -> category only needs the exceptions.
///
/// Callers use this to build a static `CASE i.status WHEN ... ELSE i.status
/// END` scalar expression for GROUP BY. That keeps category grouping a plain
/// column rewrite rather than a per-row function call or a join, and for a
/// workspace with no custom statuses the map is empty and the CASE collapses
/// to `i.status` — byte-for-byte the expression that ran before this feature.
///
/// Archived statuses are included, for the same reason [`expand_categories`]
/// includes them: issues left on one must still group into their category.
pub async fn custom_key_categories<'e, E>(
    executor: E,
    workspace_id: Uuid,
) -> anyhow::Result<std::collections::HashMap<String, String>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let entries =
        cordy_db::queries::issue_status::list_issue_status_entries(executor, workspace_id, true)
            .await?;
    let mut out = std::collections::HashMap::with_capacity(entries.len());
    for e in entries {
        if is_built_in(&e.key) || !is_category(&e.category) {
            continue;
        }
        out.insert(e.key, e.category);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_order_is_the_shipped_board_order() {
        assert_eq!(
            canonical(),
            vec![
                "backlog",
                "todo",
                "in_progress",
                "in_review",
                "done",
                "blocked",
                "cancelled"
            ]
        );
    }

    #[test]
    fn builtin_and_category_checks_agree() {
        assert!(is_built_in("todo"));
        assert!(!is_built_in("human_review"));
        assert!(is_category("done"));
        assert!(!is_category(""));
    }

    #[test]
    fn underway_categories_require_an_assignee() {
        for category in [IN_PROGRESS, IN_REVIEW, BLOCKED] {
            assert!(requires_assignee(category), "{category}");
        }
        for category in [BACKLOG, TODO, DONE, CANCELLED] {
            assert!(!requires_assignee(category), "{category}");
        }
    }

    #[test]
    fn unknown_category_ranks_last_without_colliding() {
        assert_eq!(category_rank("backlog"), 0);
        assert_eq!(category_rank("cancelled"), 6);
        assert_eq!(category_rank("nope"), 7);
    }

    #[test]
    fn validate_key_enforces_storage_constraint() {
        assert_eq!(validate_key(" Human_Review ").unwrap(), "human_review");
        assert!(validate_key("").is_err());
        // Trim + lowercase are applied before validation, like Go.
        assert_eq!(validate_key("UPPER").unwrap(), "upper");
        assert!(validate_key("-lead").is_err());
        assert!(validate_key("has space").is_err());
        assert!(validate_key(&"a".repeat(33)).is_err());
        assert!(validate_key("todo").is_err()); // reserved
        assert!(validate_key("a_1").is_ok());
    }

    #[test]
    fn slugify_derives_keys_from_display_names() {
        assert_eq!(slugify_key("Human Review").unwrap(), "human_review");
        assert_eq!(
            slugify_key("  -- Needs Triage!! --  ").unwrap(),
            "needs_triage"
        );
        assert!(slugify_key("!!!").is_err());
        // Long names truncate to 32 with trailing underscores trimmed.
        let long = slugify_key(&"abcdefgh ".repeat(10)).unwrap();
        assert!(long.len() <= 32);
    }

    // ---- DB-backed tests (skipped without DATABASE_URL) ----

    async fn test_pool() -> Option<sqlx::PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .ok()
    }

    #[tokio::test]
    async fn effective_resolves_custom_status_to_category() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let slug = format!("istat-{}", Uuid::now_v7().simple());
        let ws: Uuid = sqlx::query_scalar(
            "INSERT INTO workspace (name, slug) VALUES ('istat', $1) RETURNING id",
        )
        .bind(&slug)
        .fetch_one(&pool)
        .await
        .unwrap();

        // Unseeded: built-ins resolve without rows; custom fails open.
        assert_eq!(effective(&pool, ws, "todo").await, "todo");
        assert_eq!(effective(&pool, ws, "weird").await, "weird");

        ensure(&pool, ws).await.unwrap();

        // Seed a custom status in the in_progress category.
        sqlx::query(
            "INSERT INTO issue_status (workspace_id, key, name, category, color) VALUES ($1, 'coding_hard', 'Coding Hard', 'in_progress', '#8b5cf6')",
        )
        .bind(ws)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(effective(&pool, ws, "coding_hard").await, "in_progress");
        assert_eq!(effective(&pool, ws, "blocked").await, "blocked");

        // resolve(): built-in resolves even pre-seed semantics hold post-seed;
        // archived custom status becomes unavailable.
        let entry = resolve(&pool, ws, "coding_hard").await.unwrap();
        assert_eq!(entry.category, "in_progress");
        sqlx::query("UPDATE issue_status SET archived_at = now() WHERE workspace_id = $1 AND key = 'coding_hard'")
            .bind(ws)
            .execute(&pool)
            .await
            .unwrap();
        assert!(resolve(&pool, ws, "coding_hard").await.is_err());

        // active_keys reports built-ins plus live customs.
        let keys = active_keys(&pool, ws).await.unwrap();
        assert!(keys.contains(&"todo".to_string()));
        assert!(!keys.contains(&"coding_hard".to_string()));

        // expand_categories maps in_progress → both keys even though the
        // custom is archived (existing issues stay put).
        let expanded = expand_categories(&pool, ws, &["in_progress".to_string()])
            .await
            .unwrap();
        assert!(expanded.contains(&"in_progress".to_string()));
        assert!(expanded.contains(&"coding_hard".to_string()));

        // custom_key_categories lists only the exception mapping.
        let map = custom_key_categories(&pool, ws).await.unwrap();
        assert_eq!(
            map.get("coding_hard").map(String::as_str),
            Some("in_progress")
        );
        assert!(!map.contains_key("todo"));

        // Resolver amortizes: same answers through the batch API.
        let mut r = Resolver::new(ws);
        assert_eq!(r.effective(&pool, "todo").await, "todo");
        assert_eq!(r.effective(&pool, "coding_hard").await, "in_progress");

        sqlx::query("DELETE FROM issue_status WHERE workspace_id = $1")
            .bind(ws)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM workspace WHERE id = $1")
            .bind(ws)
            .execute(&pool)
            .await
            .unwrap();
    }
}
