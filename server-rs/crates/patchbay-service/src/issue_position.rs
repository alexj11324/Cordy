//! Issue manual-ordering positions.
use sqlx::Executor;
use uuid::Uuid;

/// Returns a position that sorts before every existing issue in the
/// workspace/status column when manual sorting orders by position ASC.
pub async fn next_top_position<'e, E>(
    executor: E,
    workspace_id: Uuid,
    status: &str,
) -> anyhow::Result<f64>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let min_pos: f64 = sqlx::query_scalar(
        "SELECT COALESCE(MIN(position), 0) FROM issue WHERE workspace_id = $1 AND status = $2",
    )
    .bind(workspace_id)
    .bind(status)
    .fetch_one(executor)
    .await
    .map_err(|e| anyhow::anyhow!("query min issue position: {e}"))?;
    Ok(min_pos - 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    async fn test_pool() -> Option<sqlx::PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .ok()
    }

    /// Integration: positions descend per (workspace, status) column, and an
    /// empty column starts at -1. Skipped when DATABASE_URL is unset.
    #[tokio::test]
    async fn next_top_position_descends_per_column() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let slug = format!("pos-test-{}", Uuid::now_v7().simple());
        let ws: Uuid = sqlx::query_scalar(
            "INSERT INTO workspace (name, slug) VALUES ('pos-test', $1) RETURNING id",
        )
        .bind(&slug)
        .fetch_one(&pool)
        .await
        .unwrap();
        let creator = Uuid::now_v7();

        for (n, pos) in [(1i64, 5.0f64), (2, 3.0)] {
            sqlx::query(
                "INSERT INTO issue (id, workspace_id, title, status, number, position, creator_type, creator_id) VALUES ($1, $2, 'x', 'todo', $3, $4, 'member', $5)",
            )
            .bind(Uuid::now_v7())
            .bind(ws)
            .bind(n)
            .bind(pos)
            .bind(creator)
            .execute(&pool)
            .await
            .unwrap();
        }

        let first = next_top_position(&pool, ws, "todo").await.unwrap();
        assert_eq!(first, 2.0); // min(5,3) - 1

        // A different status column is independent.
        let done = next_top_position(&pool, ws, "done").await.unwrap();
        assert_eq!(done, -1.0); // empty column -> COALESCE 0 - 1

        // Inserting at `first` makes the next create sort below it.
        sqlx::query(
            "INSERT INTO issue (id, workspace_id, title, status, number, position, creator_type, creator_id) VALUES ($1, $2, 'y', 'todo', 3, $3, 'member', $4)",
        )
        .bind(Uuid::now_v7())
        .bind(ws)
        .bind(first)
        .bind(creator)
        .execute(&pool)
        .await
        .unwrap();
        let second = next_top_position(&pool, ws, "todo").await.unwrap();
        assert_eq!(second, 1.0);
        assert!(second < first);

        // Cleanup — the shared test DB must not accumulate fixture rows.
        sqlx::query("DELETE FROM issue WHERE workspace_id = $1")
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
