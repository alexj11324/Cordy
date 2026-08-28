//! Pre-migration hooks and conditions, including the business backfills.

use std::future::Future;
use std::pin::Pin;

use sqlx::{PgConnection, PgPool};

use cordy_migrate::backfill::{attribution, task_usage};

/// int64 key shared with the Go runner so mixed-version clusters serialize.
pub(crate) const MIGRATION_ADVISORY_LOCK_KEY: i64 = 7244554146635925501;

pub(crate) type HookFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;
pub(crate) type ConditionFuture<'a> =
    Pin<Box<dyn Future<Output = anyhow::Result<(bool, String)>> + Send + 'a>>;

pub(crate) type PreMigrationHook = Box<dyn for<'a> Fn(&'a PgPool) -> HookFuture<'a> + Send>;

pub(crate) type MigrationCondition =
    Box<dyn for<'a> Fn(&'a mut PgConnection) -> ConditionFuture<'a> + Send>;

/// The exact index shape that may replace a fallback. Checking only
/// extension or relation existence is unsafe: historical pg_bigm migrations
/// swallowed CREATE INDEX errors, and an interrupted concurrent build can
/// leave an INVALID relation with the expected name.
pub(crate) struct UsableIndexRequirement {
    pub index_regclass: &'static str,
    pub table_regclass: &'static str,
    pub access_method: &'static str,
    pub operator_class: &'static str,
    pub expression: &'static str,
    pub extension: &'static str,
}

pub(crate) const COMMENT_CONTENT_BIGRAM_INDEX: UsableIndexRequirement = UsableIndexRequirement {
    index_regclass: "public.idx_comment_content_bigm",
    table_regclass: "public.comment",
    access_method: "gin",
    operator_class: "gin_bigm_ops",
    expression: "lower(content)",
    extension: "pg_bigm",
};

fn cleanup_invalid_concurrent_index(index_regclass: &'static str) -> PreMigrationHook {
    Box::new(move |pool| Box::pin(cleanup_invalid_index(pool, index_regclass)))
}

/// Hooks run before a version's SQL on `migrate up`.
///
/// Port of `preMigrationHooks`: the two business backfills plus an invalid-
/// index cleanup for every up migration that builds an index concurrently.
/// The map must stay total — a new CONCURRENTLY migration without an entry
/// lets an interrupted build be mistaken for success (see PB-5999/PB-6288).
pub(crate) fn hooks_for_direction(direction: &str) -> Vec<(&'static str, PreMigrationHook)> {
    let mut hooks: Vec<(&'static str, PreMigrationHook)> = match direction {
        "up" => vec![
            (
                "103_drop_legacy_daily_rollups",
                Box::new(|pool: &PgPool| {
                    Box::pin(async move { task_usage::hook(pool).await.map(|_| ()) })
                }),
            ),
            (
                "198_agent_task_attribution_strict_constraint_validate",
                Box::new(|pool: &PgPool| {
                    Box::pin(async move { attribution::hook(pool).await.map(|_| ()) })
                }),
            ),
        ],
        _ => Vec::new(),
    };
    let cleanups: &[(&'static str, &'static str)] = match direction {
        "up" => crate::index_maps::CONCURRENT_INDEX_CLEANUPS_UP,
        "down" => crate::index_maps::CONCURRENT_INDEX_CLEANUPS_DOWN,
        _ => &[],
    };
    hooks.extend(
        cleanups
            .iter()
            .map(|&(version, index)| (version, cleanup_invalid_concurrent_index(index))),
    );
    hooks
}

/// Conditions gate whether a pending migration's SQL executes; a false result
/// still records the migration as applied. Rollbacks intentionally ignore
/// environment gates.
pub(crate) fn conditions_for_direction(direction: &str) -> Vec<(&'static str, MigrationCondition)> {
    match direction {
        "up" => vec![
            ("140_comment_content_trgm_index", when_index_not_usable()),
            (
                "371_comment_content_search_index_strategy",
                when_index_usable(),
            ),
        ],
        _ => Vec::new(),
    }
}

fn when_index_usable() -> MigrationCondition {
    Box::new(|conn| {
        Box::pin(async move {
            let req = &COMMENT_CONTENT_BIGRAM_INDEX;
            if index_is_usable(conn, req).await? {
                Ok((true, String::new()))
            } else {
                Ok((
                    false,
                    format!(
                        "preferred index {} is unavailable or unusable",
                        req.index_regclass
                    ),
                ))
            }
        })
    })
}

fn when_index_not_usable() -> MigrationCondition {
    Box::new(|conn| {
        Box::pin(async move {
            let req = &COMMENT_CONTENT_BIGRAM_INDEX;
            if index_is_usable(conn, req).await? {
                Ok((
                    false,
                    format!("preferred index {} is ready", req.index_regclass),
                ))
            } else {
                Ok((true, String::new()))
            }
        })
    })
}
/// Fails closed: every property needed by the search query must match before
/// a fallback index may be skipped or removed.
async fn index_is_usable(
    conn: &mut PgConnection,
    req: &UsableIndexRequirement,
) -> anyhow::Result<bool> {
    let row: Option<bool> = sqlx::query_scalar(
        r#"
        SELECT COALESCE(
            idx.relkind = 'i'
            AND i.indisvalid
            AND i.indisready
            AND i.indislive
            AND NOT i.indisunique
            AND i.indpred IS NULL
            AND i.indexprs IS NOT NULL
            AND i.indrelid = to_regclass($2)
            AND i.indnkeyatts = 1
            AND i.indnatts = 1
            AND am.amname = $3
            AND opc.opcname = $4
            AND pg_get_indexdef(i.indexrelid, 1, FALSE) = $5
            AND EXISTS (
                SELECT 1
                FROM pg_depend dep
                JOIN pg_extension ext ON ext.oid = dep.refobjid
                WHERE dep.classid = 'pg_opclass'::regclass
                  AND dep.objid = opc.oid
                  AND dep.refclassid = 'pg_extension'::regclass
                  AND dep.deptype = 'e'
                  AND ext.extname = $6
            ),
            FALSE
        )
        FROM pg_class idx
        LEFT JOIN pg_index i ON i.indexrelid = idx.oid
        LEFT JOIN pg_am am ON am.oid = idx.relam
        LEFT JOIN pg_opclass opc ON opc.oid = (i.indclass)[0]
        WHERE idx.oid = to_regclass($1)
        "#,
    )
    .bind(req.index_regclass)
    .bind(req.table_regclass)
    .bind(req.access_method)
    .bind(req.operator_class)
    .bind(req.expression)
    .bind(req.extension)
    .fetch_optional(conn)
    .await?;

    Ok(row.unwrap_or(false))
}

/// Removes an INVALID index left by an interrupted CREATE INDEX CONCURRENTLY
/// before the migration retries. Non-index relations fail closed.
async fn cleanup_invalid_index(pool: &PgPool, index_regclass: &str) -> anyhow::Result<()> {
    let row: Option<(String, String, bool, bool)> = sqlx::query_as(
        r#"
        SELECT n.nspname, c.relname, c.relkind = 'i', COALESCE(i.indisvalid, FALSE)
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        LEFT JOIN pg_index i ON i.indexrelid = c.oid
        WHERE c.oid = to_regclass($1)
        "#,
    )
    .bind(index_regclass)
    .fetch_optional(pool)
    .await?;

    let Some((_schema, relname, is_index, is_valid)) = row else {
        return Ok(());
    };
    if !is_index {
        anyhow::bail!("relation {index_regclass:?} exists but is not an index");
    }
    if is_valid {
        return Ok(());
    }

    // Identifier quoting: Postgres identifiers cannot be bound as parameters.
    // Both components come from the catalog itself (pg_namespace/pg_class),
    // never from user input; double any embedded quotes defensively.
    let qualified = format!(
        "\"{}\".\"{}\"",
        _schema.replace('"', "\"\""),
        relname.replace('"', "\"\"")
    );
    sqlx::query(&format!("DROP INDEX CONCURRENTLY IF EXISTS {qualified}"))
        .execute(pool)
        .await?;
    tracing::warn!(index = %qualified, "removed invalid index before migration retry");
    Ok(())
}
