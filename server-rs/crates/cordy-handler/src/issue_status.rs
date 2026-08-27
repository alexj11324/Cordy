//! Workspace issue-status catalog handlers.

use std::collections::HashSet;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::{Json, Router};
use cordy_db::models::IssueStatus;
use cordy_db::queries::issue_status as status_q;
use cordy_middleware::workspace::WorkspaceContext;
use cordy_service::issue_status;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const CATEGORIES: [&str; 7] = [
    "backlog",
    "todo",
    "in_progress",
    "in_review",
    "done",
    "blocked",
    "cancelled",
];

#[derive(Debug, Serialize)]
struct IssueStatusResponse {
    id: Uuid,
    workspace_id: Uuid,
    key: String,
    name: String,
    description: String,
    category: String,
    color: String,
    is_system: bool,
    position: f64,
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
}

impl From<IssueStatus> for IssueStatusResponse {
    fn from(status: IssueStatus) -> Self {
        Self {
            id: status.id,
            workspace_id: status.workspace_id,
            key: status.key,
            name: status.name,
            description: status.description,
            category: status.category,
            color: status.color,
            is_system: status.is_system,
            position: status.position,
            archived_at: status.archived_at.map(crate::timefmt::rfc3339),
            created_at: crate::timefmt::rfc3339(status.created_at),
            updated_at: crate::timefmt::rfc3339(status.updated_at),
        }
    }
}

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/issue-statuses", get(list).post(create))
        .route("/api/issue-statuses/", get(list).post(create))
        .route("/api/issue-statuses/reorder", patch(reorder))
        .route("/api/issue-statuses/{id}", patch(update).delete(archive))
        .route("/api/issue-statuses/{id}/", patch(update).delete(archive))
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "workspace not found"))
}

fn require_admin(context: &WorkspaceContext) -> Result<(), Response> {
    match context.member.role.as_str() {
        "owner" | "admin" => Ok(()),
        _ => Err(error_response(
            StatusCode::FORBIDDEN,
            "insufficient permissions",
        )),
    }
}

fn db_error(error: anyhow::Error, message: &'static str) -> Response {
    tracing::warn!(%error, "{message}");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, message)
}

fn unique_violation(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(|e| e.as_database_error())
        .and_then(|e| e.code())
        .is_some_and(|code| code == "23505")
}

fn normalize_color(raw: &str) -> Result<String, Response> {
    let value = raw.trim();
    if value.len() != 7
        || !value.starts_with('#')
        || !value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "color must be a hex color like #3b82f6",
        ));
    }
    Ok(value.to_ascii_lowercase())
}

fn publish(state: &HandlerState, context: &WorkspaceContext, action: &str) {
    state.bus.publish(&cordy_events::Event {
        event_type: "issue_status:changed".into(),
        workspace_id: context.workspace_id.clone(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload: json!({ "action": action }),
        ..Default::default()
    });
}

#[derive(Default, Deserialize)]
struct ListQuery {
    include_archived: Option<bool>,
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(query): Query<ListQuery>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    if let Err(error) = issue_status::ensure(&state.pool, workspace_id).await {
        tracing::warn!(%error, "failed to ensure issue status catalog");
    }
    match status_q::list_issue_status_entries(
        &state.pool,
        workspace_id,
        query.include_archived.unwrap_or(false),
    )
    .await
    {
        Ok(statuses) => Json(json!({
            "total": statuses.len(),
            "statuses": statuses.into_iter().map(IssueStatusResponse::from).collect::<Vec<_>>(),
            "categories": CATEGORIES,
        }))
        .into_response(),
        Err(error) => db_error(error, "failed to list issue statuses"),
    }
}

#[derive(Deserialize)]
struct CreateRequest {
    #[serde(default)]
    key: String,
    name: String,
    #[serde(default)]
    description: String,
    category: String,
    color: String,
}

async fn create(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<CreateRequest>,
) -> Response {
    if let Err(response) = require_admin(&context) {
        return response;
    }
    if !state
        .feature_flags
        .as_deref()
        .is_some_and(cordy_service::feature_flags::custom_issue_statuses_enabled)
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "custom issue statuses are not enabled for this deployment",
        );
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return error_response(StatusCode::BAD_REQUEST, "name must be 1-64 characters");
    }
    if request.description.chars().count() > 256 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "description must be at most 256 characters",
        );
    }
    if !issue_status::is_category(&request.category) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "category must be one of: backlog, todo, in_progress, in_review, done, blocked, cancelled",
        );
    }
    let color = match normalize_color(&request.color) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let key = if request.key.trim().is_empty() {
        issue_status::slugify_key(name)
    } else {
        issue_status::validate_key(&request.key)
    };
    let key = match key {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    match status_q::create_issue_status_entry(
        &state.pool,
        workspace_id,
        &key,
        name,
        &request.description,
        &request.category,
        &color,
    )
    .await
    {
        Ok(Some(status)) => {
            publish(&state, &context, "created");
            (StatusCode::CREATED, Json(IssueStatusResponse::from(status))).into_response()
        }
        Ok(None) => db_error(
            anyhow::anyhow!("missing returned row"),
            "failed to create issue status",
        ),
        Err(error) if unique_violation(&error) => error_response(
            StatusCode::CONFLICT,
            "a status with this key or name already exists",
        ),
        Err(error) => db_error(error, "failed to create issue status"),
    }
}

#[derive(Deserialize)]
struct UpdateRequest {
    name: Option<String>,
    description: Option<String>,
    color: Option<String>,
    position: Option<f64>,
}

async fn update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    Json(request): Json<UpdateRequest>,
) -> Response {
    if let Err(response) = require_admin(&context) {
        return response;
    }
    let Ok(id) = Uuid::parse_str(&id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid issue status id");
    };
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let Some(current) =
        (match status_q::get_issue_status_entry_by_id(&state.pool, id, workspace_id).await {
            Ok(value) => value,
            Err(error) => return db_error(error, "failed to load issue status"),
        })
    else {
        return error_response(StatusCode::NOT_FOUND, "issue status not found");
    };
    if current.is_system {
        return error_response(
            StatusCode::FORBIDDEN,
            "built-in statuses cannot be modified",
        );
    }
    if current.archived_at.is_some() {
        return error_response(StatusCode::CONFLICT, "archived statuses cannot be modified");
    }
    let name = match request.name {
        Some(value) if value.trim().is_empty() || value.trim().chars().count() > 64 => {
            return error_response(StatusCode::BAD_REQUEST, "name must be 1-64 characters");
        }
        Some(value) => Some(value.trim().to_string()),
        None => None,
    };
    if request
        .description
        .as_ref()
        .is_some_and(|v| v.chars().count() > 256)
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "description must be at most 256 characters",
        );
    }
    let color = match request.color.as_deref().map(normalize_color).transpose() {
        Ok(value) => value,
        Err(response) => return response,
    };
    match status_q::update_issue_status_entry(
        &state.pool,
        name.as_deref(),
        request.description.as_deref(),
        color.as_deref(),
        request.position,
        id,
        workspace_id,
    )
    .await
    {
        Ok(Some(status)) => {
            publish(&state, &context, "updated");
            Json(IssueStatusResponse::from(status)).into_response()
        }
        Ok(None) => error_response(StatusCode::CONFLICT, "status is no longer editable"),
        Err(error) if unique_violation(&error) => error_response(
            StatusCode::CONFLICT,
            "a status with this name already exists",
        ),
        Err(error) => db_error(error, "failed to update issue status"),
    }
}

async fn archive(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = require_admin(&context) {
        return response;
    }
    let Ok(id) = Uuid::parse_str(&id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid issue status id");
    };
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let Some(current) =
        (match status_q::get_issue_status_entry_by_id(&state.pool, id, workspace_id).await {
            Ok(value) => value,
            Err(error) => return db_error(error, "failed to load issue status"),
        })
    else {
        return error_response(StatusCode::NOT_FOUND, "issue status not found");
    };
    if current.is_system {
        return error_response(
            StatusCode::FORBIDDEN,
            "built-in statuses cannot be archived",
        );
    }
    if current.archived_at.is_some() {
        return Json(IssueStatusResponse::from(current)).into_response();
    }
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(error) => return db_error(error.into(), "failed to archive issue status"),
    };
    let result = async {
        status_q::lock_issue_status_catalog(&mut *transaction, workspace_id).await?;
        let archived =
            status_q::archive_issue_status_entry(&mut *transaction, id, workspace_id).await?;
        transaction.commit().await?;
        anyhow::Ok(archived)
    }
    .await;
    match result {
        Ok(Some(status)) => {
            publish(&state, &context, "archived");
            Json(IssueStatusResponse::from(status)).into_response()
        }
        Ok(None) => error_response(StatusCode::CONFLICT, "status is no longer archivable"),
        Err(error) => db_error(error, "failed to archive issue status"),
    }
}

#[derive(Deserialize)]
struct ReorderRequest {
    category: String,
    ids: Vec<Uuid>,
}

async fn reorder(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<ReorderRequest>,
) -> Response {
    if let Err(response) = require_admin(&context) {
        return response;
    }
    if !issue_status::is_category(&request.category) {
        return error_response(StatusCode::BAD_REQUEST, "invalid category");
    }
    if request.ids.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "ids must not be empty");
    }
    if request.ids.iter().copied().collect::<HashSet<_>>().len() != request.ids.len() {
        return error_response(StatusCode::BAD_REQUEST, "duplicate ids");
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(error) => return db_error(error.into(), "failed to reorder issue statuses"),
    };
    let result = async {
        status_q::lock_issue_status_catalog_shared(&mut *transaction, workspace_id).await?;
        let active = status_q::list_active_custom_issue_status_entries(
            &mut *transaction,
            workspace_id,
            &request.category,
        )
        .await?;
        let wanted = request.ids.iter().copied().collect::<HashSet<_>>();
        let actual = active.iter().map(|entry| entry.id).collect::<HashSet<_>>();
        if wanted != actual {
            anyhow::bail!("catalog_changed");
        }
        let affected =
            status_q::reorder_issue_status_entries(&mut *transaction, workspace_id, request.ids)
                .await?;
        if affected as usize != active.len() {
            anyhow::bail!("catalog_changed");
        }
        let statuses =
            status_q::list_issue_status_entries(&mut *transaction, workspace_id, true).await?;
        transaction.commit().await?;
        anyhow::Ok(statuses)
    }
    .await;
    match result {
        Ok(statuses) => {
            publish(&state, &context, "reordered");
            Json(json!({ "total": statuses.len(), "statuses": statuses.into_iter().map(IssueStatusResponse::from).collect::<Vec<_>>(), "categories": CATEGORIES }))
                .into_response()
        }
        Err(error) if error.to_string() == "catalog_changed" => error_response(
            StatusCode::CONFLICT,
            "ids must name every active custom status in the category",
        ),
        Err(error) => db_error(error, "failed to reorder issue statuses"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use cordy_auth::pat_cache::PatCache;
    use cordy_db::models::Member;
    use cordy_service::feature_flags::FlagSource;
    use cordy_service::issue_service::{IssueCreateError, IssueCreateOpts, IssueCreateParams};
    use http_body_util::BodyExt as _;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt as _;

    struct TestFlags(bool);

    impl FlagSource for TestFlags {
        fn is_enabled(&self, key: &str, default: bool) -> bool {
            assert_eq!(key, cordy_service::feature_flags::CUSTOM_ISSUE_STATUSES);
            assert!(!default, "custom status rollout must fail closed");
            self.0
        }
    }

    fn context(workspace_id: Uuid, role: &str) -> WorkspaceContext {
        WorkspaceContext {
            workspace_id: workspace_id.to_string(),
            member: Member {
                created_at: chrono::Utc::now(),
                id: Uuid::now_v7(),
                role: role.to_string(),
                user_id: Uuid::now_v7(),
                workspace_id,
            },
        }
    }

    fn state(pool: sqlx::PgPool, custom_statuses: bool) -> HandlerState {
        let mut state = HandlerState::new(pool, PatCache::disabled(), None);
        state.feature_flags = Some(Arc::new(TestFlags(custom_statuses)));
        state
    }

    async fn body(response: Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("response body")
            .to_bytes();
        let json = serde_json::from_slice(&bytes).expect("JSON response");
        (status, json)
    }

    async fn issue_request(
        state: HandlerState,
        context: WorkspaceContext,
        method: axum::http::Method,
        uri: String,
        value: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let request = axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(value.to_string()))
            .expect("issue request");
        let response = crate::issue::router()
            .with_state(state)
            .layer(Extension(context))
            .oneshot(request)
            .await
            .expect("issue response");
        body(response).await
    }

    async fn test_pool() -> sqlx::PgPool {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required for issue-status production contract tests");
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect issue-status production contract database")
    }

    async fn create_workspace(pool: &sqlx::PgPool) -> Uuid {
        let slug = format!("issue-status-contract-{}", Uuid::now_v7().simple());
        sqlx::query_scalar(
            "INSERT INTO workspace (name, slug) VALUES ('issue status contract', $1) RETURNING id",
        )
        .bind(slug)
        .fetch_one(pool)
        .await
        .expect("create workspace")
    }

    async fn delete_workspace(pool: &sqlx::PgPool, workspace_id: Uuid) {
        sqlx::query("DELETE FROM issue WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(pool)
            .await
            .expect("delete issues");
        sqlx::query("DELETE FROM issue_status WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(pool)
            .await
            .expect("delete issue statuses");
        sqlx::query("DELETE FROM workspace WHERE id = $1")
            .bind(workspace_id)
            .execute(pool)
            .await
            .expect("delete workspace");
    }

    async fn wait_for_shared_catalog_lock(pool: &sqlx::PgPool, workspace_id: Uuid) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let acquired: bool = sqlx::query_scalar(
                    "SELECT pg_try_advisory_lock(hashtextextended($1::uuid::text || ':issue_status', 0))",
                )
                .bind(workspace_id)
                .fetch_one(pool)
                .await
                .expect("probe catalog lock");
                if !acquired {
                    return;
                }
                sqlx::query(
                    "SELECT pg_advisory_unlock(hashtextextended($1::uuid::text || ':issue_status', 0))",
                )
                .bind(workspace_id)
                .execute(pool)
                .await
                .expect("release catalog lock probe");
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("production create did not acquire the shared catalog lock");
    }

    #[test]
    fn validates_colors() {
        assert_eq!(normalize_color("#3B82F6").unwrap(), "#3b82f6");
        assert!(normalize_color("blue").is_err());
    }

    #[tokio::test]
    async fn write_policy_fails_before_database_access() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://localhost/cordy-policy-test")
            .expect("lazy pool");
        let workspace_id = Uuid::now_v7();

        let (status, value) = body(
            create(
                State(state(pool.clone(), true)),
                Extension(context(workspace_id, "member")),
                Json(CreateRequest {
                    key: "triage".into(),
                    name: "Triage".into(),
                    description: String::new(),
                    category: "todo".into(),
                    color: "#3b82f6".into(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(value, json!({"error": "insufficient permissions"}));

        let (status, value) = body(
            create(
                State(state(pool, false)),
                Extension(context(workspace_id, "admin")),
                Json(CreateRequest {
                    key: "triage".into(),
                    name: "Triage".into(),
                    description: String::new(),
                    category: "todo".into(),
                    color: "#3b82f6".into(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            value,
            json!({"error": "custom issue statuses are not enabled for this deployment"})
        );
    }

    #[tokio::test]
    async fn production_catalog_contract_is_atomic_and_emits_only_committed_changes() {
        let pool = test_pool().await;
        let workspace_id = create_workspace(&pool).await;
        let state = state(pool.clone(), true);
        let events = Arc::new(Mutex::new(Vec::<cordy_events::Event>::new()));
        let recorded = events.clone();
        state.bus.subscribe("issue_status:changed", move |event| {
            recorded.lock().expect("event log").push(event.clone());
        });

        // Reads are available to ordinary members and self-heal all seven
        // built-ins for workspaces created by an older server.
        let (status, value) = body(
            list(
                State(state.clone()),
                Extension(context(workspace_id, "member")),
                Query(ListQuery::default()),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["total"], 7);
        assert_eq!(value["categories"].as_array().expect("categories").len(), 7);
        assert!(events.lock().expect("event log").is_empty());

        let admin = context(workspace_id, "admin");
        let create_status = |key: &str, name: &str| CreateRequest {
            key: key.into(),
            name: name.into(),
            description: "Needs human judgment".into(),
            category: "in_progress".into(),
            color: "#8B5CF6".into(),
        };
        let (status, first) = body(
            create(
                State(state.clone()),
                Extension(admin.clone()),
                Json(create_status("human_review", "Human Review")),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(first["key"], "human_review");
        assert_eq!(first["category"], "in_progress");
        assert_eq!(first["color"], "#8b5cf6");
        let first_id = Uuid::parse_str(first["id"].as_str().expect("first id")).unwrap();

        let (status, duplicate) = body(
            create(
                State(state.clone()),
                Extension(admin.clone()),
                Json(create_status("human_review", "Another Name")),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            duplicate,
            json!({"error": "a status with this key or name already exists"})
        );
        assert_eq!(events.lock().expect("event log").len(), 1);

        let (status, second) = body(
            create(
                State(state.clone()),
                Extension(admin.clone()),
                Json(create_status("waiting_review", "Waiting Review")),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let second_id = Uuid::parse_str(second["id"].as_str().expect("second id")).unwrap();

        let built_in: Uuid = sqlx::query_scalar(
            "SELECT id FROM issue_status WHERE workspace_id = $1 AND key = 'in_progress'",
        )
        .bind(workspace_id)
        .fetch_one(&pool)
        .await
        .expect("built-in status");
        let (status, immutable) = body(
            archive(
                State(state.clone()),
                Extension(admin.clone()),
                Path(built_in.to_string()),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            immutable,
            json!({"error": "built-in statuses cannot be archived"})
        );

        // A partial order is rejected without writing positions or emitting.
        let positions_before: Vec<(Uuid, f64)> =
            sqlx::query_as("SELECT id, position FROM issue_status WHERE id = ANY($1) ORDER BY id")
                .bind(vec![first_id, second_id])
                .fetch_all(&pool)
                .await
                .expect("positions before rejected reorder");
        let (status, rejected) = body(
            reorder(
                State(state.clone()),
                Extension(admin.clone()),
                Json(ReorderRequest {
                    category: "in_progress".into(),
                    ids: vec![first_id],
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            rejected,
            json!({"error": "ids must name every active custom status in the category"})
        );
        let positions_after: Vec<(Uuid, f64)> =
            sqlx::query_as("SELECT id, position FROM issue_status WHERE id = ANY($1) ORDER BY id")
                .bind(vec![first_id, second_id])
                .fetch_all(&pool)
                .await
                .expect("positions after rejected reorder");
        assert_eq!(positions_after, positions_before);
        assert_eq!(events.lock().expect("event log").len(), 2);

        let (status, _) = body(
            reorder(
                State(state.clone()),
                Extension(admin.clone()),
                Json(ReorderRequest {
                    category: "in_progress".into(),
                    ids: vec![second_id, first_id],
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let ordered: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM issue_status WHERE workspace_id = $1 AND category = 'in_progress' AND is_system = FALSE AND archived_at IS NULL ORDER BY position, key",
        )
        .bind(workspace_id)
        .fetch_all(&pool)
        .await
        .expect("committed order");
        assert_eq!(ordered, vec![second_id, first_id]);

        let (status, archived) = body(
            archive(
                State(state.clone()),
                Extension(admin),
                Path(first_id.to_string()),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(archived["archived_at"].is_string());
        assert!(issue_status::resolve(&pool, workspace_id, "human_review")
            .await
            .is_err());
        assert_eq!(
            issue_status::effective(&pool, workspace_id, "human_review").await,
            "in_progress"
        );

        let actions: Vec<String> = events
            .lock()
            .expect("event log")
            .iter()
            .map(|event| {
                assert_eq!(event.workspace_id, workspace_id.to_string());
                event.payload["action"]
                    .as_str()
                    .expect("action")
                    .to_string()
            })
            .collect();
        assert_eq!(actions, ["created", "created", "reordered", "archived"]);

        let (status, active) = body(
            list(
                State(state.clone()),
                Extension(context(workspace_id, "member")),
                Query(ListQuery::default()),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(active["total"], 8);
        let (status, all) = body(
            list(
                State(state),
                Extension(context(workspace_id, "member")),
                Query(ListQuery {
                    include_archived: Some(true),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(all["total"], 9);

        delete_workspace(&pool, workspace_id).await;
    }

    #[tokio::test]
    async fn issue_create_and_archive_race_has_only_the_two_go_outcomes() {
        let pool = test_pool().await;
        let workspace_id = create_workspace(&pool).await;
        issue_status::ensure(&pool, workspace_id)
            .await
            .expect("seed catalog");
        let state = state(pool.clone(), true);
        let admin = context(workspace_id, "admin");
        let creator_id = admin.member.user_id;

        let insert_status = |key: &'static str, name: &'static str| {
            let pool = pool.clone();
            async move {
                status_q::create_issue_status_entry(
                    &pool,
                    workspace_id,
                    key,
                    name,
                    "",
                    "in_progress",
                    "#8b5cf6",
                )
                .await
                .expect("create custom status")
                .expect("custom status row")
            }
        };
        let issue_params = |title: &str, status: &str| IssueCreateParams {
            workspace_id,
            title: title.into(),
            status: status.into(),
            priority: "none".into(),
            creator_type: "member".into(),
            creator_id,
            allow_duplicate: true,
            ..IssueCreateParams::default()
        };

        // Archive wins: the real IssueService re-resolves beneath its shared
        // catalog lock and cannot commit a future assignment.
        let archived_first = insert_status("archive_first", "Archive First").await;
        assert_eq!(
            archive(
                State(state.clone()),
                Extension(admin.clone()),
                Path(archived_first.id.to_string()),
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert!(matches!(
            state
                .issues
                .create(
                    issue_params("must not commit", "archive_first"),
                    IssueCreateOpts::default(),
                )
                .await,
            Err(IssueCreateError::StatusUnavailable)
        ));

        // Issue write wins: a shared catalog lock permits the real create
        // transaction and holds the archive's exclusive lock until commit.
        // The later archive retires only future use; the committed issue keeps
        // its custom key and therefore its category behavior.
        let issue_first = insert_status("issue_first", "Issue First").await;
        let mut row_blocker = pool.begin().await.expect("workspace row blocker");
        sqlx::query("SELECT id FROM workspace WHERE id = $1 FOR UPDATE")
            .bind(workspace_id)
            .fetch_one(&mut *row_blocker)
            .await
            .expect("lock workspace row");
        let create_state = state.clone();
        let create_params = issue_params("commits before archive", "issue_first");
        let pending_create = tokio::spawn(async move {
            create_state
                .issues
                .create(create_params, IssueCreateOpts::default())
                .await
        });
        wait_for_shared_catalog_lock(&pool, workspace_id).await;
        let archive_state = state.clone();
        let archive_admin = admin.clone();
        let archive_id = issue_first.id;
        let mut pending_archive = tokio::spawn(async move {
            archive(
                State(archive_state),
                Extension(archive_admin),
                Path(archive_id.to_string()),
            )
            .await
        });

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut pending_archive)
                .await
                .is_err(),
            "archive must wait for the production create's shared catalog lock"
        );
        row_blocker.commit().await.expect("release workspace row");
        let created = pending_create
            .await
            .expect("create task")
            .expect("issue create wins")
            .issue
            .expect("created issue");
        assert_eq!(
            pending_archive.await.expect("archive task").status(),
            StatusCode::OK
        );
        let persisted_status: String = sqlx::query_scalar("SELECT status FROM issue WHERE id = $1")
            .bind(created.id)
            .fetch_one(&pool)
            .await
            .expect("persisted issue");
        assert_eq!(persisted_status, "issue_first");
        assert_eq!(
            issue_status::effective(&pool, workspace_id, &persisted_status).await,
            "in_progress"
        );
        assert!(matches!(
            state
                .issues
                .create(
                    issue_params("future assignment rejected", "issue_first"),
                    IssueCreateOpts::default(),
                )
                .await,
            Err(IssueCreateError::StatusUnavailable)
        ));

        // The production PUT path has the same two deterministic outcomes.
        // Holding the exclusive catalog lock lets its preflight read succeed,
        // then archives before the in-transaction shared-lock recheck.
        let archive_before_update =
            insert_status("archive_before_update", "Archive Before Update").await;
        let mut exclusive = pool.begin().await.expect("exclusive lock transaction");
        status_q::lock_issue_status_catalog(&mut *exclusive, workspace_id)
            .await
            .expect("exclusive catalog lock");
        let update_state = state.clone();
        let update_context = admin.clone();
        let issue_id = created.id;
        let mut pending_update = tokio::spawn(async move {
            issue_request(
                update_state,
                update_context,
                axum::http::Method::PUT,
                format!("/api/issues/{issue_id}"),
                json!({"status": "archive_before_update"}),
            )
            .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut pending_update)
                .await
                .is_err(),
            "update must wait for the exclusive catalog holder"
        );
        status_q::archive_issue_status_entry(
            &mut *exclusive,
            archive_before_update.id,
            workspace_id,
        )
        .await
        .expect("archive update target")
        .expect("archived update target");
        exclusive.commit().await.expect("commit archive first");
        let (status, conflict) = pending_update.await.expect("update task");
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            conflict,
            json!({"error": "the target status was archived while this request was in flight; reload the status list and retry"})
        );

        // Conversely, the real PUT may commit beneath a shared lock before
        // archive proceeds. Batch update calls the same apply_issue_update
        // transaction; exercise that public route while the status is live.
        let update_before_archive =
            insert_status("update_before_archive", "Update Before Archive").await;
        let mut row_blocker = pool.begin().await.expect("issue row blocker");
        sqlx::query("SELECT id FROM issue WHERE id = $1 FOR UPDATE")
            .bind(created.id)
            .fetch_one(&mut *row_blocker)
            .await
            .expect("lock issue row");
        let update_state = state.clone();
        let update_context = admin.clone();
        let issue_id = created.id;
        let mut pending_update = tokio::spawn(async move {
            issue_request(
                update_state,
                update_context,
                axum::http::Method::PUT,
                format!("/api/issues/{issue_id}"),
                json!({"status": "update_before_archive"}),
            )
            .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut pending_update)
                .await
                .is_err(),
            "update must wait for the issue-row holder after taking its shared catalog lock"
        );
        let archive_state = state.clone();
        let archive_admin = admin.clone();
        let archive_id = update_before_archive.id;
        let mut pending_archive = tokio::spawn(async move {
            archive(
                State(archive_state),
                Extension(archive_admin),
                Path(archive_id.to_string()),
            )
            .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut pending_archive)
                .await
                .is_err(),
            "archive must wait for the update-side shared lock"
        );
        row_blocker.commit().await.expect("release issue row");
        let (status, updated) = pending_update.await.expect("update-winner task");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["status"], "update_before_archive");
        assert_eq!(
            pending_archive
                .await
                .expect("update-winner archive task")
                .status(),
            StatusCode::OK
        );
        let persisted_status: String = sqlx::query_scalar("SELECT status FROM issue WHERE id = $1")
            .bind(created.id)
            .fetch_one(&pool)
            .await
            .expect("updated issue status");
        assert_eq!(persisted_status, "update_before_archive");

        let batch_target = insert_status("batch_target", "Batch Target").await;
        let (status, batch) = issue_request(
            state,
            admin,
            axum::http::Method::POST,
            "/api/issues/batch-update".into(),
            json!({
                "issue_ids": [created.id.to_string()],
                "updates": {"status": batch_target.key},
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(batch, json!({"updated": 1}));

        delete_workspace(&pool, workspace_id).await;
    }
}
