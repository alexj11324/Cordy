//! Canonical Work Product APIs and task-owned exact-branch discovery.
//!
//! A provider object is mirrored independently from its association. The only
//! durable association paths are an authenticated task attach, an authenticated
//! member's explicit Attach operation, or the deterministic post-run lookup
//! performed from the same task's persisted execution provenance. PR text is
//! intentionally absent from this module: titles, bodies, and branch naming
//! conventions never establish identity or authorization.

use std::collections::BTreeSet;

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use patchbay_db::models::{
    AgentTaskExecutionProvenance, AgentTaskQueue, WorkProduct, WorkProductRelation, Workspace,
};
use patchbay_db::queries::{
    agent, chat, github, issue as issue_q, project, project_resource,
    work_product as work_product_q, workspace,
};
use patchbay_middleware::workspace::WorkspaceContext;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

#[derive(Debug, Clone, Default)]
pub(crate) struct ExecutionProvenanceInput {
    pub repo_identity: String,
    pub execution_workspace: String,
    pub head_branch: String,
    pub head_sha: String,
    pub head_state: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AttachExistingRequest {
    work_product_id: Uuid,
    #[serde(default)]
    close_intent: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListUnassociatedQuery {
    page: Option<i32>,
    per_page: Option<i32>,
    query: Option<String>,
}

#[derive(Debug)]
struct AttachActor {
    event_type: String,
    actor_id: Uuid,
    task_id: Option<Uuid>,
    run_id: Option<Uuid>,
    relation_source: &'static str,
    attached_by_type: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
enum BranchDiscoveryDecision {
    Ineligible(&'static str),
    Unassociated,
    Ambiguous(&'static str),
    Associated,
}

fn classify_branch_discovery(
    head_state: &str,
    other_execution_count: usize,
    match_count: usize,
) -> BranchDiscoveryDecision {
    if head_state != "attached" {
        return BranchDiscoveryDecision::Ineligible(match head_state {
            "default" => "default_branch",
            "detached" => "detached_head",
            _ => "unknown_head_state",
        });
    }
    if other_execution_count > 0 {
        return BranchDiscoveryDecision::Ambiguous("branch_used_by_other_execution");
    }
    match match_count {
        0 => BranchDiscoveryDecision::Unassociated,
        1 => BranchDiscoveryDecision::Associated,
        _ => BranchDiscoveryDecision::Ambiguous("multiple_pull_requests_for_exact_head"),
    }
}

pub(crate) fn relation_response(relation: &WorkProductRelation) -> Value {
    json!({
        "id": relation.id,
        "workspace_id": relation.workspace_id,
        "work_product_id": relation.work_product_id,
        "issue_id": relation.issue_id,
        "task_id": relation.task_id,
        "run_id": relation.run_id,
        "relation_key": relation.relation_key,
        "relation_source": relation.relation_source,
        "attached_by_type": relation.attached_by_type,
        "attached_by_id": relation.attached_by_id,
        "attached_at": crate::timefmt::rfc3339(relation.attached_at),
        "close_intent": relation.close_intent,
        "detached_at": relation.detached_at.map(crate::timefmt::rfc3339),
        "detached_by_type": relation.detached_by_type,
        "detached_by_id": relation.detached_by_id,
        "detached_task_id": relation.detached_task_id,
        "detached_run_id": relation.detached_run_id,
    })
}

pub(crate) fn product_response(
    product: &WorkProduct,
    relation: Option<&WorkProductRelation>,
) -> Value {
    json!({
        "id": product.id,
        "workspace_id": product.workspace_id,
        "kind": product.kind,
        "provider": product.provider,
        "external_identity": product.external_identity,
        "external_url": product.external_url,
        "provider_record_type": product.provider_record_type,
        "provider_record_id": product.provider_record_id,
        "created_at": crate::timefmt::rfc3339(product.created_at),
        "updated_at": crate::timefmt::rfc3339(product.updated_at),
        "association_state": if relation.is_some() { "associated" } else { "unassociated" },
        "relation": relation.map(relation_response),
    })
}

pub(crate) fn provenance_response(provenance: &AgentTaskExecutionProvenance) -> Value {
    json!({
        "task_id": provenance.task_id,
        "workspace_id": provenance.workspace_id,
        "run_id": provenance.run_id,
        "repo_identity": provenance.repo_identity,
        "execution_workspace": provenance.execution_workspace,
        "head_branch": provenance.head_branch,
        "head_sha": provenance.head_sha,
        "head_state": provenance.head_state,
        "started_at": provenance.started_at.map(crate::timefmt::rfc3339),
        "finished_at": provenance.finished_at.map(crate::timefmt::rfc3339),
        "discovery_status": provenance.discovery_status,
        "discovery_lease_id": provenance.discovery_lease_id,
        "discovery_match_count": provenance.discovery_match_count,
        "discovery_reason": provenance.discovery_reason,
        "discovery_work_product_id": provenance.discovery_work_product_id,
        "discovery_at": provenance.discovery_at.map(crate::timefmt::rfc3339),
        "updated_at": crate::timefmt::rfc3339(provenance.updated_at),
    })
}

/// Explicitly attaches an already mirrored Work Product selected by id. The
/// id is workspace-scoped server-side; a caller cannot supply a task, run, or
/// issue owner in the body.
pub(crate) async fn attach_existing(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_issue): Path<String>,
    headers: HeaderMap,
    body: Option<Json<AttachExistingRequest>>,
) -> Response {
    let issue = match crate::issue::resolve_issue(&state, &context, &raw_issue).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let Some(Json(request)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    // Authenticate the attaching actor before reading any product metadata.
    // In particular, a task token must be resolved from the server-stamped
    // execution headers before it can even select a Work Product in its
    // workspace.
    let actor = match attach_actor(&state, &context, &headers, issue.id).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let product = match work_product_q::get_work_product_by_id(
        &state.pool,
        issue.workspace_id,
        request.work_product_id,
    )
    .await
    {
        Ok(Some(product)) => product,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "work product not found"),
        Err(error) => {
            tracing::warn!(%error, "work product lookup failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load work product",
            );
        }
    };
    if let Some(task_id) = actor.task_id {
        // A task token may retry an attach that it already registered, but it
        // may not select an arbitrary unassociated product by id. New task
        // products must go through the provider-backed URL path below, where
        // repository/head ownership is checked against persisted provenance.
        let owns_product = match work_product_q::list_relations_for_task(
            &state.pool,
            issue.workspace_id,
            task_id,
        )
        .await
        {
            Ok(relations) => relations.iter().any(|relation| {
                relation.work_product_id == product.id
                    && relation.issue_id == Some(issue.id)
                    && relation.task_id == Some(task_id)
                    && relation.run_id == actor.run_id
            }),
            Err(error) => {
                tracing::warn!(%error, task_id = %task_id, "task work product ownership lookup failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to verify task work product",
                );
            }
        };
        if !owns_product {
            return error_response(
                StatusCode::FORBIDDEN,
                "task execution cannot attach an unowned work product",
            );
        }
    }
    let relation =
        match attach_relation(&state, &issue, &product, &actor, request.close_intent).await {
            Ok(relation) => relation,
            Err(response) => return response,
        };
    publish_relation_event(&state, &issue, &product, &relation, &actor);
    if request.close_intent {
        crate::vcs_webhook::maybe_complete_issue(&state, issue.clone()).await;
    }
    (
        StatusCode::OK,
        Json(json!({
            "work_product": product_response(&product, Some(&relation)),
            "relation": relation_response(&relation),
        })),
    )
        .into_response()
}

pub(crate) async fn list_for_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_issue): Path<String>,
) -> Response {
    let issue = match crate::issue::resolve_issue(&state, &context, &raw_issue).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    match work_product_q::list_work_products_by_issue(&state.pool, issue.workspace_id, issue.id)
        .await
    {
        Ok(rows) => Json(json!({
            "work_products": rows
                .iter()
                .map(|(product, relation)| product_response(product, Some(relation)))
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "work product list failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list work products",
            )
        }
    }
}

pub(crate) async fn detach(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_issue, raw_product)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let issue = match crate::issue::resolve_issue(&state, &context, &raw_issue).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let Ok(product_id) = Uuid::parse_str(raw_product.trim()) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid work product id");
    };
    let actor = match attach_actor(&state, &context, &headers, issue.id).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let product =
        match work_product_q::get_work_product_by_id(&state.pool, issue.workspace_id, product_id)
            .await
        {
            Ok(Some(product)) => product,
            Ok(None) => return error_response(StatusCode::NOT_FOUND, "work product not found"),
            Err(error) => {
                tracing::warn!(%error, "work product lookup failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load work product",
                );
            }
        };
    let detached = match work_product_q::detach_work_product_relations(
        &state.pool,
        issue.workspace_id,
        product.id,
        issue.id,
        actor.attached_by_type,
        actor.actor_id,
        actor.task_id,
        actor.run_id,
        actor.task_id,
    )
    .await
    {
        Ok(count) => count,
        Err(error) => {
            tracing::warn!(%error, "work product detach failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to detach work product",
            );
        }
    };
    if detached == 0 {
        return error_response(StatusCode::NOT_FOUND, "work product relation not found");
    }
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::EVENT_PULL_REQUEST_UPDATED.into(),
        workspace_id: issue.workspace_id.to_string(),
        actor_type: actor.event_type,
        actor_id: actor.actor_id.to_string(),
        payload: json!({
            "work_product": product_response(&product, None),
            "linked_issue_ids": [],
            "detached": true,
        }),
        task_id: actor.task_id.map(|id| id.to_string()).unwrap_or_default(),
        ..Default::default()
    });
    // Detaching a blocking product changes the same completion predicate used
    // by webhook updates. Re-evaluate it for every successful detach so an
    // issue with only merged/closed products can finish immediately.
    crate::vcs_webhook::maybe_complete_issue(&state, issue.clone()).await;
    Json(json!({ "detached": detached })).into_response()
}

pub(crate) async fn list_unassociated(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(query): Query<ListUnassociatedQuery>,
) -> Response {
    let page = query.page.unwrap_or(1);
    let per_page = query.per_page.unwrap_or(20);
    if !(1..=100_000).contains(&page) {
        return error_response(StatusCode::BAD_REQUEST, "invalid page");
    }
    if !(1..=50).contains(&per_page) {
        return error_response(StatusCode::BAD_REQUEST, "invalid per_page");
    }
    let search = query.query.unwrap_or_default().trim().to_owned();
    if search.chars().count() > 200 {
        return error_response(StatusCode::BAD_REQUEST, "query is too long");
    }
    let offset = (page - 1) * per_page;
    match work_product_q::list_unassociated_work_products(
        &state.pool,
        context.member.workspace_id,
        "pull_request",
        &search,
        per_page + 1,
        offset,
    )
    .await
    {
        Ok(mut products) => {
            let next_page = (products.len() > per_page as usize).then_some(page + 1);
            products.truncate(per_page as usize);
            Json(json!({
                "work_products": products
                    .iter()
                    .map(|product| product_response(product, None))
                    .collect::<Vec<_>>(),
                "next_page": next_page,
            }))
            .into_response()
        }
        Err(error) => {
            tracing::warn!(%error, "unassociated work product list failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list unassociated work products",
            )
        }
    }
}

pub(crate) async fn list_for_task(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_task): Path<String>,
) -> Response {
    let Ok(task_id) = Uuid::parse_str(raw_task.trim()) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid task id");
    };
    let task =
        match agent::get_agent_task_in_workspace(&state.pool, task_id, context.member.workspace_id)
            .await
        {
            Ok(Some(task)) => task,
            Ok(None) => return error_response(StatusCode::NOT_FOUND, "task not found"),
            Err(error) => {
                tracing::warn!(%error, "task work product lookup failed");
                return error_response(StatusCode::NOT_FOUND, "task not found");
            }
        };
    let provenances = match work_product_q::list_execution_provenances(
        &state.pool,
        context.member.workspace_id,
        task.id,
    )
    .await
    {
        Ok(provenance) => provenance,
        Err(error) => {
            tracing::warn!(%error, task_id = %task.id, "execution provenance lookup failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load execution provenance",
            );
        }
    };
    let products = match work_product_q::list_work_products_by_task(
        &state.pool,
        context.member.workspace_id,
        task.id,
    )
    .await
    {
        Ok(relations) => relations,
        Err(error) => {
            tracing::warn!(%error, task_id = %task.id, "task work product lookup failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list task work products",
            );
        }
    };
    Json(json!({
        "task_id": task.id,
        "provenances": provenances.iter().map(provenance_response).collect::<Vec<_>>(),
        "work_products": products
            .iter()
            .map(|(product, relation)| product_response(product, Some(relation)))
            .collect::<Vec<_>>(),
    }))
    .into_response()
}

/// Records authenticated task execution provenance. The task id comes from
/// the daemon route and workspace comes from the server-side task lookup; the
/// request only carries the local runtime's exact worktree facts.
pub(crate) async fn record_execution_provenance(
    state: &HandlerState,
    task_id: Uuid,
    workspace_id: Uuid,
    run_id: Option<Uuid>,
    input: &ExecutionProvenanceInput,
    finished: bool,
) -> anyhow::Result<AgentTaskExecutionProvenance> {
    let repo_identity = normalize_repo_identity(&input.repo_identity);
    let execution_workspace = nonempty(&input.execution_workspace);
    let head_branch = nonempty(&input.head_branch);
    let head_sha = nonempty(&input.head_sha);
    let mut head_state = match input.head_state.trim() {
        "attached" | "detached" | "default" | "unknown" => input.head_state.trim(),
        _ => "unknown",
    };
    // An omitted terminal fact means that this adapter did not have a local
    // checkout to inspect. The terminal `unknown` state is authoritative and
    // prevents an earlier branch hint from being used for discovery.
    if head_branch.is_none() && head_state == "attached" {
        head_state = "detached";
    } else if repo_identity.is_none() {
        head_state = "unknown";
    }
    work_product_q::upsert_execution_provenance(
        &state.pool,
        task_id,
        workspace_id,
        run_id,
        repo_identity.as_deref(),
        execution_workspace,
        head_branch,
        head_sha,
        head_state,
        finished,
    )
    .await
}

/// A task-token attach must describe a provider object produced by this task's
/// own execution. The task token alone is not sufficient authorization for an
/// arbitrary URL: the provider must prove the exact repository and head branch
/// captured for the task's execution workspace. Explicit registration is the
/// authority, so another task using the same branch does not invalidate a
/// separately authenticated registration.
pub(crate) async fn validate_task_explicit_product(
    state: &HandlerState,
    workspace_id: Uuid,
    task_id: Uuid,
    repo_identity: &str,
    metadata: &patchbay_ghsnapshot::PullRequestMetadata,
) -> Result<(), &'static str> {
    let provenances =
        work_product_q::list_execution_provenances(&state.pool, workspace_id, task_id)
            .await
            .map_err(|_| "execution_provenance_unavailable")?;
    if provenances.is_empty() {
        return Err("execution_provenance_unavailable");
    }
    let task = agent::get_agent_task_in_workspace(&state.pool, task_id, workspace_id)
        .await
        .map_err(|_| "task_unavailable")?
        .ok_or("task_unavailable")?;
    let workspace = workspace::get_workspace(&state.pool, workspace_id)
        .await
        .map_err(|_| "workspace_unavailable")?
        .ok_or("workspace_unavailable")?;
    if metadata.head_repo_identity.is_none() {
        return Err("provider_head_repository_unavailable");
    }
    if !head_repository_matches(metadata.head_repo_identity.as_deref(), repo_identity) {
        return Err("provider_head_repository_mismatch");
    }
    if !task_repository_is_authorized(state, &workspace, &task, repo_identity)
        .await
        .map_err(|_| "task_repository_authorization_unavailable")?
    {
        return Err("repository_not_authorized_for_workspace");
    }
    for provenance in &provenances {
        let Some(execution_workspace) = provenance.execution_workspace.as_deref() else {
            continue;
        };
        if provenance.head_state != "attached"
            || !std::path::Path::new(execution_workspace).is_absolute()
            || execution_workspace.contains('\0')
            || !task_execution_workspace_matches(
                task.work_dir.as_deref(),
                task.durable_work_dir.as_deref(),
                execution_workspace,
            )
            || provenance.repo_identity.as_deref() != Some(repo_identity)
            || metadata.branch.is_empty()
            || provenance.head_branch.as_deref() != Some(metadata.branch.as_str())
        {
            continue;
        }
        return Ok(());
    }
    if provenances
        .iter()
        .all(|provenance| provenance.head_state != "attached")
    {
        return Err("execution_head_not_attached");
    }
    if provenances.iter().any(|provenance| {
        provenance
            .execution_workspace
            .as_deref()
            .is_some_and(|path| path.contains('\0') || !std::path::Path::new(path).is_absolute())
    }) {
        return Err("execution_workspace_invalid");
    }
    if provenances.iter().any(|provenance| {
        provenance
            .execution_workspace
            .as_deref()
            .is_some_and(|path| {
                task_execution_workspace_matches(
                    task.work_dir.as_deref(),
                    task.durable_work_dir.as_deref(),
                    path,
                )
            })
    }) {
        return Err("provider_repository_or_branch_mismatch");
    }
    Err("execution_workspace_not_owned_by_task")
}

/// Resolves the effective repository scope for a claimed task from
/// server-owned workspace, issue/chat, project, and project-resource rows.
/// Project resources are intentionally additive to workspace repositories:
/// claim delivery may narrow the task's UI payload to project repositories,
/// but authorization must still accept that same server-owned scope.
async fn task_repository_is_authorized(
    state: &HandlerState,
    workspace: &Workspace,
    task: &AgentTaskQueue,
    repo_identity: &str,
) -> anyhow::Result<bool> {
    if workspace_contains_repo(&workspace.repos, repo_identity) {
        return Ok(true);
    }

    let mut project_ids = BTreeSet::new();
    if let Some(issue_id) = task.issue_id {
        let issue = issue_q::get_issue_in_workspace(&state.pool, issue_id, workspace.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("task issue is not in the task workspace"))?;
        if let Some(project_id) = issue.project_id {
            project_ids.insert(project_id);
        }
    }
    if let Some(chat_session_id) = task.chat_session_id {
        let chat = chat::get_chat_session_in_workspace(&state.pool, chat_session_id, workspace.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("task chat session is not in the task workspace"))?;
        if let Some(project_id) = chat.project_id {
            project_ids.insert(project_id);
        }
    }

    for project_id in project_ids {
        if project::get_project_in_workspace(&state.pool, project_id, workspace.id)
            .await?
            .is_none()
        {
            return Err(anyhow::anyhow!("task project is not in the task workspace"));
        }
        for resource in project_resource::list_project_resources(&state.pool, project_id).await? {
            if resource.workspace_id != workspace.id || resource.resource_type != "github_repo" {
                continue;
            }
            let Some(url) = resource.resource_ref.get("url").and_then(Value::as_str) else {
                continue;
            };
            if normalize_repo_identity(url).as_deref() == Some(repo_identity) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Records terminal provenance and queues discovery before the asynchronous
/// provider lookup is spawned. The pending state is durable, so a restarted
/// server can drain it without treating a missing worker as success.
pub(crate) async fn queue_task_discovery(
    state: &HandlerState,
    task: &patchbay_db::models::AgentTaskQueue,
    workspace_id: Uuid,
) -> anyhow::Result<()> {
    // A task may finish without a provider checkout. Keep that outcome
    // observable with one empty, unknown provenance row; all real facts must
    // have arrived through the daemon-authenticated route before this queue is
    // entered. Never accept lifecycle-request fields as provenance.
    if work_product_q::list_execution_provenances(&state.pool, workspace_id, task.id)
        .await?
        .is_empty()
    {
        let input = ExecutionProvenanceInput::default();
        record_execution_provenance(
            state,
            task.id,
            workspace_id,
            task.autopilot_run_id,
            &input,
            true,
        )
        .await?;
    }
    if work_product_q::has_active_relation_for_task(&state.pool, workspace_id, task.id).await? {
        work_product_q::mark_task_discovery_skipped_for_explicit_relation(
            &state.pool,
            workspace_id,
            task.id,
        )
        .await?;
        tracing::debug!(
            task_id = %task.id,
            "skip branch discovery after explicit work product relation"
        );
        return Ok(());
    }
    work_product_q::mark_task_discovery_pending(&state.pool, workspace_id, task.id).await?;
    Ok(())
}

/// Processes every persisted checkout for one task. This is deliberately
/// plural: one task can check out several authorized repositories and each
/// exact workspace/head gets an independent first-discovery attempt.
pub(crate) async fn discover_pending_for_task(
    state: &HandlerState,
    task: &patchbay_db::models::AgentTaskQueue,
    workspace_id: Uuid,
) {
    match work_product_q::has_active_relation_for_task(&state.pool, workspace_id, task.id).await {
        Ok(true) => {
            if let Err(error) = work_product_q::mark_task_discovery_skipped_for_explicit_relation(
                &state.pool,
                workspace_id,
                task.id,
            )
            .await
            {
                tracing::warn!(
                    %error,
                    task_id = %task.id,
                    "mark explicit work product discovery skip failed"
                );
            }
            return;
        }
        Ok(false) => {}
        Err(error) => {
            tracing::warn!(
                %error,
                task_id = %task.id,
                "check explicit work product relation failed"
            );
            return;
        }
    }
    let provenances = match work_product_q::list_execution_provenances(
        &state.pool,
        workspace_id,
        task.id,
    )
    .await
    {
        Ok(provenances) => provenances,
        Err(error) => {
            tracing::warn!(%error, task_id = %task.id, "list pending work product provenance failed");
            return;
        }
    };
    for provenance in provenances {
        if !matches!(
            provenance.discovery_status.as_str(),
            work_product_q::DISCOVERY_PENDING | work_product_q::DISCOVERY_IN_PROGRESS
        ) {
            continue;
        }
        let provenance = match work_product_q::claim_execution_discovery(&state.pool, &provenance)
            .await
        {
            Ok(Some(provenance)) => provenance,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(%error, task_id = %task.id, "claim work product discovery failed");
                continue;
            }
        };
        discover_one_execution(state, task, &provenance).await;
    }
}

/// Drains pending rows left by a process restart. This worker only consumes
/// the durable post-run queue; webhook and poller paths never invoke it and do
/// not perform branch discovery.
pub(crate) async fn drain_pending_work_product_discovery(state: &HandlerState) {
    let tasks = match work_product_q::list_pending_execution_discovery_tasks(&state.pool, 100).await
    {
        Ok(tasks) => tasks,
        Err(error) => {
            tracing::warn!(%error, "list durable work product discovery queue failed");
            return;
        }
    };
    for (workspace_id, task_id) in tasks {
        match agent::get_agent_task_in_workspace(&state.pool, task_id, workspace_id).await {
            Ok(Some(task)) => discover_pending_for_task(state, &task, workspace_id).await,
            Ok(None) => {
                let provenances = match work_product_q::list_execution_provenances(
                    &state.pool,
                    workspace_id,
                    task_id,
                )
                .await
                {
                    Ok(provenances) => provenances,
                    Err(error) => {
                        tracing::warn!(%error, %task_id, "list orphaned work product provenance failed");
                        continue;
                    }
                };
                for provenance in provenances {
                    if let Ok(Some(provenance)) =
                        work_product_q::claim_execution_discovery(&state.pool, &provenance).await
                    {
                        record_discovery_failure(
                            state,
                            &provenance,
                            work_product_q::DISCOVERY_INELIGIBLE,
                            0,
                            Some("task_not_found"),
                        )
                        .await;
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, %task_id, "load queued work product task failed");
            }
        }
    }
}

/// Production-owned durable queue worker. The terminal callback performs the
/// short enqueue transaction synchronously; this worker owns the provider
/// lookup and drains rows left in `pending`/stale `in_progress` after restart.
pub struct WorkProductDiscoveryRuntime {
    cancel: tokio_util::sync::CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl WorkProductDiscoveryRuntime {
    pub fn start(state: HandlerState, cancel: tokio_util::sync::CancellationToken) -> Self {
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = task_cancel.cancelled() => return,
                    _ = ticker.tick() => drain_pending_work_product_discovery(&state).await,
                }
            }
        });
        Self {
            cancel,
            task: Some(task),
        }
    }

    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        let Some(mut task) = self.task.take() else {
            return;
        };
        if tokio::time::timeout(std::time::Duration::from_secs(10), &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

async fn discover_one_execution(
    state: &HandlerState,
    task: &patchbay_db::models::AgentTaskQueue,
    provenance: &AgentTaskExecutionProvenance,
) {
    let workspace_id = provenance.workspace_id;
    let Some(repo_identity) = provenance.repo_identity.as_deref() else {
        record_discovery_failure(
            state,
            provenance,
            work_product_q::DISCOVERY_INELIGIBLE,
            0,
            Some("missing_repository_identity"),
        )
        .await;
        return;
    };
    let Some(execution_workspace) = provenance.execution_workspace.as_deref() else {
        record_discovery_failure(
            state,
            provenance,
            work_product_q::DISCOVERY_INELIGIBLE,
            0,
            Some("missing_execution_workspace"),
        )
        .await;
        return;
    };
    if !std::path::Path::new(execution_workspace).is_absolute()
        || execution_workspace.contains('\0')
    {
        record_discovery_failure(
            state,
            provenance,
            work_product_q::DISCOVERY_INELIGIBLE,
            0,
            Some("execution_workspace_not_absolute"),
        )
        .await;
        return;
    }
    if !task_execution_workspace_matches(
        task.work_dir.as_deref(),
        task.durable_work_dir.as_deref(),
        execution_workspace,
    ) {
        record_discovery_failure(
            state,
            provenance,
            work_product_q::DISCOVERY_INELIGIBLE,
            0,
            Some("execution_workspace_not_owned_by_task"),
        )
        .await;
        return;
    }
    let Some(head_branch) = provenance.head_branch.as_deref() else {
        record_discovery_failure(
            state,
            provenance,
            work_product_q::DISCOVERY_INELIGIBLE,
            0,
            Some("missing_head_branch"),
        )
        .await;
        return;
    };
    let Some(head_sha) = provenance.head_sha.as_deref().and_then(nonempty) else {
        record_discovery_failure(
            state,
            provenance,
            work_product_q::DISCOVERY_INELIGIBLE,
            0,
            Some("missing_head_sha"),
        )
        .await;
        return;
    };
    if let BranchDiscoveryDecision::Ineligible(reason) =
        classify_branch_discovery(&provenance.head_state, 0, 0)
    {
        record_discovery_failure(
            state,
            provenance,
            work_product_q::DISCOVERY_INELIGIBLE,
            0,
            Some(reason),
        )
        .await;
        return;
    }
    let Ok(Some(workspace)) = workspace::get_workspace(&state.pool, workspace_id).await else {
        record_discovery_failure(
            state,
            provenance,
            work_product_q::DISCOVERY_INELIGIBLE,
            0,
            Some("workspace_not_found"),
        )
        .await;
        return;
    };
    match task_repository_is_authorized(state, &workspace, task, repo_identity).await {
        Ok(true) => {}
        Ok(false) => {
            record_discovery_failure(
                state,
                provenance,
                work_product_q::DISCOVERY_INELIGIBLE,
                0,
                Some("repository_not_authorized_for_workspace"),
            )
            .await;
            return;
        }
        Err(error) => {
            record_discovery_failure(
                state,
                provenance,
                work_product_q::DISCOVERY_INELIGIBLE,
                0,
                Some("task_repository_authorization_unavailable"),
            )
            .await;
            tracing::warn!(%error, task_id = %task.id, "branch discovery project repository authorization failed");
            return;
        }
    }

    let Some((owner, repo)) = split_repo_identity(repo_identity) else {
        record_discovery_failure(
            state,
            provenance,
            work_product_q::DISCOVERY_INELIGIBLE,
            0,
            Some("unsupported_repository_identity"),
        )
        .await;
        return;
    };
    let Some(client) = state.github_snapshots.client() else {
        record_discovery_failure(
            state,
            provenance,
            work_product_q::DISCOVERY_INELIGIBLE,
            0,
            Some("github_app_not_configured"),
        )
        .await;
        return;
    };
    // Keep the advisory lock alive on the same transaction through the branch
    // ownership check, provider lookup, mirror upsert, and durable relation.
    // This prevents two terminal deliveries from both deciding a shared head
    // is unique before either relation is committed.
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            record_discovery_failure(
                state,
                provenance,
                work_product_q::DISCOVERY_AMBIGUOUS,
                0,
                Some("discovery_transaction_unavailable"),
            )
            .await;
            tracing::warn!(%error, task_id = %task.id, "branch discovery transaction begin failed");
            return;
        }
    };
    if let Err(error) = work_product_q::lock_branch_discovery(
        &mut *transaction,
        workspace_id,
        repo_identity,
        head_branch,
    )
    .await
    {
        drop(transaction);
        record_discovery_failure(
            state,
            provenance,
            work_product_q::DISCOVERY_AMBIGUOUS,
            0,
            Some("branch_lock_failed"),
        )
        .await;
        tracing::warn!(%error, task_id = %task.id, "branch discovery lock failed");
        return;
    }
    if let Err(error) =
        work_product_q::lock_task_work_product_scope(&mut *transaction, workspace_id, task.id).await
    {
        drop(transaction);
        record_discovery_failure(
            state,
            provenance,
            work_product_q::DISCOVERY_AMBIGUOUS,
            0,
            Some("task_work_product_lock_failed"),
        )
        .await;
        tracing::warn!(%error, task_id = %task.id, "task work product lock failed");
        return;
    }

    // Fence the claimed provenance row before any terminal decision. A stale
    // worker may have been reclaimed while it was doing the preflight checks;
    // only the worker holding the current lease may write an audit result or
    // relation in this transaction.
    match work_product_q::lock_execution_discovery_lease(&mut *transaction, provenance).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::debug!(task_id = %task.id, "discovery lease was reclaimed before branch checks");
            return;
        }
        Err(error) => {
            drop(transaction);
            record_discovery_failure(
                state,
                provenance,
                work_product_q::DISCOVERY_AMBIGUOUS,
                0,
                Some("discovery_lease_lock_failed"),
            )
            .await;
            tracing::warn!(%error, task_id = %task.id, "lock work product discovery lease failed");
            return;
        }
    }

    let other_executions = match work_product_q::list_other_branch_executions(
        &mut *transaction,
        workspace_id,
        repo_identity,
        head_branch,
        head_sha,
        task.id,
    )
    .await
    {
        Ok(executions) => executions,
        Err(error) => {
            drop(transaction);
            record_discovery_failure(
                state,
                provenance,
                work_product_q::DISCOVERY_AMBIGUOUS,
                0,
                Some("active_execution_lookup_failed"),
            )
            .await;
            tracing::warn!(%error, task_id = %task.id, "branch discovery execution lookup failed");
            return;
        }
    };
    if let BranchDiscoveryDecision::Ambiguous(reason) =
        classify_branch_discovery("attached", other_executions.len(), 0)
    {
        if let Err(error) = commit_discovery_status(
            transaction,
            provenance,
            work_product_q::DISCOVERY_AMBIGUOUS,
            other_executions.len() as i32,
            Some(reason),
            None,
        )
        .await
        {
            tracing::warn!(%error, task_id = %task.id, "commit ambiguous branch discovery failed");
        }
        return;
    }

    // An explicit task registration is authoritative over discovery. Recheck
    // before any provider lookup, immediately before the durable discovery
    // transaction can create a relation.
    match work_product_q::has_active_relation_for_task(&mut *transaction, workspace_id, task.id)
        .await
    {
        Ok(true) => {
            if let Err(error) = work_product_q::mark_task_discovery_skipped_for_explicit_relation(
                &mut *transaction,
                workspace_id,
                task.id,
            )
            .await
            {
                tracing::warn!(%error, task_id = %task.id, "mark explicit work product discovery skip failed");
                return;
            }
            if let Err(error) = transaction.commit().await {
                tracing::warn!(%error, task_id = %task.id, "commit explicit work product discovery skip failed");
            }
            return;
        }
        Ok(false) => {}
        Err(error) => {
            drop(transaction);
            record_discovery_failure(
                state,
                provenance,
                work_product_q::DISCOVERY_AMBIGUOUS,
                0,
                Some("explicit_relation_lookup_failed"),
            )
            .await;
            tracing::warn!(
                %error,
                task_id = %task.id,
                "check explicit work product relation in discovery transaction failed"
            );
            return;
        }
    }

    let installations = match github::list_git_hub_installations_by_workspace(
        &mut *transaction,
        workspace_id,
    )
    .await
    {
        Ok(installations) => installations,
        Err(error) => {
            drop(transaction);
            record_discovery_failure(
                state,
                provenance,
                work_product_q::DISCOVERY_INELIGIBLE,
                0,
                Some("github_installation_lookup_failed"),
            )
            .await;
            tracing::warn!(%error, task_id = %task.id, "branch discovery installation lookup failed");
            return;
        }
    };

    let mut matches = Vec::new();
    let mut lookup_failed = false;
    let mut installation_lookup_failed = false;
    let mut head_sha_mismatch = false;
    for installation in installations {
        match client
            .pull_requests_by_head(installation.installation_id, owner, repo, head_branch)
            .await
        {
            Ok(items) => {
                for item in items {
                    if item.metadata.branch != head_branch {
                        tracing::warn!(
                            task_id = %task.id,
                            expected_branch = %head_branch,
                            returned_branch = %item.metadata.branch,
                            pr_number = item.number,
                            "branch discovery provider returned a non-exact head"
                        );
                        lookup_failed = true;
                        continue;
                    }
                    if !head_repository_matches(
                        item.metadata.head_repo_identity.as_deref(),
                        repo_identity,
                    ) {
                        tracing::warn!(
                            task_id = %task.id,
                            expected_repository = %repo_identity,
                            returned_repository = ?item.metadata.head_repo_identity,
                            pr_number = item.number,
                            "branch discovery provider returned a non-exact head repository"
                        );
                        lookup_failed = true;
                        continue;
                    }
                    if item.metadata.head_sha != head_sha {
                        tracing::warn!(
                            task_id = %task.id,
                            expected_head_sha = %head_sha,
                            returned_head_sha = %item.metadata.head_sha,
                            pr_number = item.number,
                            "branch discovery provider returned a non-exact head commit"
                        );
                        head_sha_mismatch = true;
                        continue;
                    }
                    if !matches
                        .iter()
                        .any(|found: &DiscoveredPullRequest| found.number == item.number)
                    {
                        matches.push(DiscoveredPullRequest {
                            installation_id: installation.installation_id,
                            number: item.number,
                            metadata: item.metadata,
                        });
                    }
                }
            }
            Err(error) if error.to_string().contains("status 404") => {
                tracing::debug!(
                    task_id = %task.id,
                    installation_id = installation.installation_id,
                    "branch discovery skipped installation without repository access"
                );
            }
            Err(error) => {
                // An installation can be present in the same workspace while
                // not owning this repository. Its provider lookup must not
                // suppress an exact match returned by another installation;
                // retain the failure only for the no-match audit below.
                installation_lookup_failed = true;
                tracing::warn!(
                    %error,
                    task_id = %task.id,
                    installation_id = installation.installation_id,
                    "branch discovery GitHub lookup failed"
                );
            }
        }
    }

    // The task lock is held across every provider request above. This second
    // fence closes the other ordering: an explicit attach that committed
    // before discovery acquired the lock must suppress discovery even when
    // the provider lookup itself took a long time.
    match work_product_q::has_active_relation_for_task(&mut *transaction, workspace_id, task.id)
        .await
    {
        Ok(true) => {
            if let Err(error) = work_product_q::mark_task_discovery_skipped_for_explicit_relation(
                &mut *transaction,
                workspace_id,
                task.id,
            )
            .await
            {
                drop(transaction);
                record_discovery_failure(
                    state,
                    provenance,
                    work_product_q::DISCOVERY_AMBIGUOUS,
                    0,
                    Some("explicit_relation_skip_audit_failed"),
                )
                .await;
                tracing::warn!(%error, task_id = %task.id, "mark explicit work product discovery skip failed after provider lookup");
                return;
            }
            if let Err(error) = transaction.commit().await {
                tracing::warn!(%error, task_id = %task.id, "commit explicit work product discovery skip failed after provider lookup");
            }
            return;
        }
        Ok(false) => {}
        Err(error) => {
            drop(transaction);
            record_discovery_failure(
                state,
                provenance,
                work_product_q::DISCOVERY_AMBIGUOUS,
                0,
                Some("explicit_relation_fence_failed_after_provider_lookup"),
            )
            .await;
            tracing::warn!(
                %error,
                task_id = %task.id,
                "check explicit work product relation after provider lookup failed"
            );
            return;
        }
    }

    if lookup_failed || (installation_lookup_failed && matches.is_empty()) {
        if let Err(error) = commit_discovery_status(
            transaction,
            provenance,
            work_product_q::DISCOVERY_AMBIGUOUS,
            matches.len() as i32,
            Some("github_pull_request_lookup_failed"),
            None,
        )
        .await
        {
            tracing::warn!(%error, task_id = %task.id, "commit failed branch discovery audit failed");
        }
        return;
    }
    if head_sha_mismatch {
        if let Err(error) = commit_discovery_status(
            transaction,
            provenance,
            work_product_q::DISCOVERY_AMBIGUOUS,
            matches.len() as i32,
            Some("pull_request_head_sha_mismatch"),
            None,
        )
        .await
        {
            tracing::warn!(%error, task_id = %task.id, "commit head SHA mismatch discovery failed");
        }
        return;
    }
    match classify_branch_discovery("attached", 0, matches.len()) {
        BranchDiscoveryDecision::Unassociated => {
            if let Err(error) = commit_discovery_status(
                transaction,
                provenance,
                work_product_q::DISCOVERY_UNASSOCIATED,
                0,
                Some("no_pull_request_for_exact_head"),
                None,
            )
            .await
            {
                tracing::warn!(%error, task_id = %task.id, "commit unassociated branch discovery failed");
            }
            return;
        }
        BranchDiscoveryDecision::Ambiguous(reason) => {
            if let Err(error) = commit_discovery_status(
                transaction,
                provenance,
                work_product_q::DISCOVERY_AMBIGUOUS,
                matches.len() as i32,
                Some(reason),
                None,
            )
            .await
            {
                tracing::warn!(%error, task_id = %task.id, "commit multiple branch discovery failed");
            }
            return;
        }
        BranchDiscoveryDecision::Associated => {}
        BranchDiscoveryDecision::Ineligible(_) => unreachable!("attached head state is eligible"),
    }

    let found = matches.pop().expect("one match");
    let metadata = found.metadata;
    let canonical_url = if metadata.html_url.is_empty() {
        format!("https://github.com/{owner}/{repo}/pull/{}", found.number)
    } else {
        metadata.html_url.clone()
    };
    let mirrored = match github::attach_git_hub_pull_request(
        &mut *transaction,
        workspace_id,
        Some(found.installation_id),
        owner,
        repo,
        found.number,
        &metadata.title,
        &metadata.state,
        &canonical_url,
        Some(metadata.created_at),
        Some(metadata.updated_at),
        &metadata.head_sha,
        metadata.additions,
        metadata.deletions,
        metadata.changed_files,
        nonempty(&metadata.branch),
        nonempty(&metadata.author_login),
        nonempty(&metadata.author_avatar_url),
        metadata.merged_at,
        metadata.closed_at,
        true,
    )
    .await
    {
        Ok(Some(pr)) => pr,
        Ok(None) => {
            drop(transaction);
            record_discovery_failure(
                state,
                provenance,
                work_product_q::DISCOVERY_UNASSOCIATED,
                1,
                Some("pull_request_mirror_not_written"),
            )
            .await;
            return;
        }
        Err(error) => {
            drop(transaction);
            record_discovery_failure(
                state,
                provenance,
                work_product_q::DISCOVERY_UNASSOCIATED,
                1,
                Some("pull_request_mirror_write_failed"),
            )
            .await;
            tracing::warn!(%error, task_id = %task.id, "branch discovery mirror write failed");
            return;
        }
    };
    let product = match work_product_q::upsert_work_product(
        &mut *transaction,
        workspace_id,
        "pull_request",
        "github",
        &work_product_q::external_identity_for_github(owner, repo, found.number),
        Some(&canonical_url),
        Some("github_pull_request"),
        Some(mirrored.id),
    )
    .await
    {
        Ok(product) => product,
        Err(error) => {
            drop(transaction);
            record_discovery_failure(
                state,
                provenance,
                work_product_q::DISCOVERY_UNASSOCIATED,
                1,
                Some("work_product_write_failed"),
            )
            .await;
            tracing::warn!(%error, task_id = %task.id, "branch discovery work product write failed");
            return;
        }
    };
    match work_product_q::lock_work_product(&mut *transaction, workspace_id, product.id).await {
        Ok(true) => {}
        Ok(false) => {
            drop(transaction);
            record_discovery_failure(
                state,
                provenance,
                work_product_q::DISCOVERY_UNASSOCIATED,
                1,
                Some("work_product_removed_before_relation"),
            )
            .await;
            return;
        }
        Err(error) => {
            drop(transaction);
            record_discovery_failure(
                state,
                provenance,
                work_product_q::DISCOVERY_UNASSOCIATED,
                1,
                Some("work_product_lock_failed"),
            )
            .await;
            tracing::warn!(
                %error,
                task_id = %task.id,
                "branch discovery work product lock failed"
            );
            return;
        }
    }
    let relation_key =
        work_product_q::relation_key(task.issue_id, Some(task.id), task.autopilot_run_id);
    let relation = match work_product_q::attach_work_product_relation(
        &mut *transaction,
        workspace_id,
        product.id,
        task.issue_id,
        Some(task.id),
        task.autopilot_run_id,
        &relation_key,
        work_product_q::RELATION_SOURCE_EXECUTION_BRANCH_DISCOVERY,
        "agent",
        task.agent_id,
        false,
    )
    .await
    {
        Ok(relation) => relation,
        Err(error) => {
            drop(transaction);
            record_discovery_failure(
                state,
                provenance,
                work_product_q::DISCOVERY_UNASSOCIATED,
                1,
                Some("relation_write_failed"),
            )
            .await;
            tracing::warn!(%error, task_id = %task.id, "branch discovery relation write failed");
            return;
        }
    };
    if let Err(error) = work_product_q::mark_execution_discovery(
        &mut *transaction,
        provenance,
        work_product_q::DISCOVERY_ASSOCIATED,
        1,
        Some("unique_pull_request_for_exact_head"),
        Some(product.id),
    )
    .await
    {
        drop(transaction);
        record_discovery_failure(
            state,
            provenance,
            work_product_q::DISCOVERY_UNASSOCIATED,
            1,
            Some("discovery_audit_write_failed"),
        )
        .await;
        tracing::warn!(%error, task_id = %task.id, "branch discovery audit write failed");
        return;
    }
    if let Err(error) = transaction.commit().await {
        record_discovery_failure(
            state,
            provenance,
            work_product_q::DISCOVERY_UNASSOCIATED,
            1,
            Some("discovery_transaction_commit_failed"),
        )
        .await;
        tracing::warn!(%error, task_id = %task.id, "branch discovery transaction commit failed");
        return;
    }
    let linked_issue_ids = task
        .issue_id
        .map(|id| vec![id.to_string()])
        .unwrap_or_default();
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::EVENT_PULL_REQUEST_UPDATED.into(),
        workspace_id: workspace_id.to_string(),
        actor_type: "agent".into(),
        actor_id: task.agent_id.to_string(),
        payload: json!({
            "pull_request": crate::issue_pull_request::github_model_response(mirrored, state.github_snapshots.enabled()),
            "linked_issue_ids": linked_issue_ids,
            "relation": relation_response(&relation),
        }),
        task_id: task.id.to_string(),
        ..Default::default()
    });
}

async fn commit_discovery_status(
    mut transaction: sqlx::Transaction<'_, sqlx::Postgres>,
    provenance: &AgentTaskExecutionProvenance,
    status: &str,
    match_count: i32,
    reason: Option<&str>,
    work_product_id: Option<Uuid>,
) -> anyhow::Result<()> {
    work_product_q::mark_execution_discovery(
        &mut *transaction,
        provenance,
        status,
        match_count,
        reason,
        work_product_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn attach_actor(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    issue_id: Uuid,
) -> Result<AttachActor, Response> {
    let execution = crate::issue::trusted_agent_execution_context(state, context, headers).await;
    if crate::issue::has_task_execution_claim(headers) && execution.is_none() {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "the task execution context is not authorized",
        ));
    }
    if let Some(execution) = execution {
        if execution.issue_id != Some(issue_id) {
            return Err(error_response(
                StatusCode::FORBIDDEN,
                "a task may only attach work products to its assigned issue",
            ));
        }
        return Ok(AttachActor {
            event_type: "agent".into(),
            actor_id: execution.agent_id,
            task_id: Some(execution.task_id),
            run_id: execution.run_id,
            relation_source: work_product_q::RELATION_SOURCE_TASK_EXPLICIT,
            attached_by_type: "agent",
        });
    }
    Ok(AttachActor {
        event_type: "member".into(),
        actor_id: context.member.user_id,
        task_id: None,
        run_id: None,
        relation_source: work_product_q::RELATION_SOURCE_MANUAL_EXPLICIT,
        attached_by_type: "user",
    })
}

async fn attach_relation(
    state: &HandlerState,
    issue: &patchbay_db::models::Issue,
    product: &WorkProduct,
    actor: &AttachActor,
    close_intent: bool,
) -> Result<WorkProductRelation, Response> {
    let relation_key = work_product_q::relation_key(Some(issue.id), actor.task_id, actor.run_id);
    attach_work_product_relation_locked(
        state,
        issue.workspace_id,
        product.id,
        Some(issue.id),
        actor.task_id,
        actor.run_id,
        &relation_key,
        actor.relation_source,
        actor.attached_by_type,
        actor.actor_id,
        close_intent,
    )
    .await
    .map_err(|error| {
        tracing::warn!(%error, product_id = %product.id, issue_id = %issue.id, "work product relation write failed");
        error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to attach work product")
    })
}

/// Serializes explicit relation insertion with product cleanup. The product
/// lock must be held in the same transaction as the relation write; locking it
/// in a standalone pool statement would not protect the insert.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn attach_work_product_relation_locked(
    state: &HandlerState,
    workspace_id: Uuid,
    work_product_id: Uuid,
    issue_id: Option<Uuid>,
    task_id: Option<Uuid>,
    run_id: Option<Uuid>,
    relation_key: &str,
    relation_source: &str,
    attached_by_type: &str,
    attached_by_id: Uuid,
    close_intent: bool,
) -> anyhow::Result<WorkProductRelation> {
    let mut transaction = state.pool.begin().await?;
    if let Some(task_id) = task_id {
        work_product_q::lock_task_work_product_scope(&mut *transaction, workspace_id, task_id)
            .await?;
    }
    let relation = attach_work_product_relation_in_transaction(
        &mut *transaction,
        workspace_id,
        work_product_id,
        issue_id,
        task_id,
        run_id,
        relation_key,
        relation_source,
        attached_by_type,
        attached_by_id,
        close_intent,
    )
    .await?;
    transaction.commit().await?;
    Ok(relation)
}

/// Inserts an explicit relation on a caller-owned transaction. Task-scoped
/// provider attaches use this form so the task advisory lock acquired before
/// provider lookup remains held through the Work Product row lock and relation
/// upsert. The caller owns commit/rollback.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn attach_work_product_relation_in_transaction(
    transaction: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    work_product_id: Uuid,
    issue_id: Option<Uuid>,
    task_id: Option<Uuid>,
    run_id: Option<Uuid>,
    relation_key: &str,
    relation_source: &str,
    attached_by_type: &str,
    attached_by_id: Uuid,
    close_intent: bool,
) -> anyhow::Result<WorkProductRelation> {
    if !work_product_q::lock_work_product(&mut *transaction, workspace_id, work_product_id).await? {
        anyhow::bail!("work product is not in the requested workspace");
    }
    work_product_q::attach_work_product_relation(
        &mut *transaction,
        workspace_id,
        work_product_id,
        issue_id,
        task_id,
        run_id,
        relation_key,
        relation_source,
        attached_by_type,
        attached_by_id,
        close_intent,
    )
    .await
}

fn publish_relation_event(
    state: &HandlerState,
    issue: &patchbay_db::models::Issue,
    product: &WorkProduct,
    relation: &WorkProductRelation,
    actor: &AttachActor,
) {
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::EVENT_PULL_REQUEST_UPDATED.into(),
        workspace_id: issue.workspace_id.to_string(),
        actor_type: actor.event_type.clone(),
        actor_id: actor.actor_id.to_string(),
        payload: json!({
            "work_product": product_response(product, Some(relation)),
            "linked_issue_ids": [issue.id.to_string()],
            "relation": relation_response(relation),
        }),
        task_id: actor.task_id.map(|id| id.to_string()).unwrap_or_default(),
        ..Default::default()
    });
}

struct DiscoveredPullRequest {
    installation_id: i64,
    number: i32,
    metadata: patchbay_ghsnapshot::PullRequestMetadata,
}

async fn record_discovery_result(
    state: &HandlerState,
    provenance: &AgentTaskExecutionProvenance,
    status: &str,
    match_count: i32,
    reason: Option<&str>,
    work_product_id: Option<Uuid>,
) {
    if let Err(error) = work_product_q::mark_execution_discovery(
        &state.pool,
        provenance,
        status,
        match_count,
        reason,
        work_product_id,
    )
    .await
    {
        tracing::warn!(%error, task_id = %provenance.task_id, "write work product discovery audit failed");
    }
}

async fn record_discovery_failure(
    state: &HandlerState,
    provenance: &AgentTaskExecutionProvenance,
    status: &str,
    match_count: i32,
    reason: Option<&str>,
) {
    record_discovery_result(state, provenance, status, match_count, reason, None).await;
}

fn nonempty(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value.trim())
}

fn workspace_contains_repo(repos: &Value, candidate: &str) -> bool {
    repos
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|repo| repo.get("url").and_then(Value::as_str))
        .filter_map(normalize_repo_identity)
        .any(|identity| identity == candidate)
}

fn task_execution_workspace_matches(
    work_dir: Option<&str>,
    durable_work_dir: Option<&str>,
    execution_workspace: &str,
) -> bool {
    let known_paths = [work_dir, durable_work_dir]
        .into_iter()
        .flatten()
        .filter(|path| !path.trim().is_empty())
        .collect::<Vec<_>>();
    !known_paths.is_empty()
        && known_paths.iter().any(|path| {
            let task_path = std::path::Path::new(path);
            let execution_path = std::path::Path::new(execution_workspace);
            task_path.is_absolute()
                && !task_path.to_string_lossy().contains('\0')
                && execution_path.is_absolute()
                && !execution_path.to_string_lossy().contains('\0')
                // The task's cwd can be either the repository/worktree root
                // or a subdirectory selected by the workspace resource. The
                // provider reports the exact git top-level, so ownership is
                // safe in both lexical directions; component-aware
                // `starts_with` prevents `/task-1-other` prefix collisions.
                && (execution_path.starts_with(task_path)
                    || task_path.starts_with(execution_path))
        })
}

fn head_repository_matches(head_repo_identity: Option<&str>, expected: &str) -> bool {
    head_repo_identity
        .and_then(normalize_repo_identity)
        .map(|identity| identity == expected)
        .unwrap_or(false)
}

fn split_repo_identity(identity: &str) -> Option<(&str, &str)> {
    identity.split_once('/')
}

/// Normalizes only repository transport identities. It does not inspect PR
/// titles, bodies, branch names, or task identifiers.
pub(crate) fn normalize_repo_identity(raw: &str) -> Option<String> {
    let mut value = raw.trim().trim_end_matches('/').to_string();
    if let Some(rest) = value.strip_prefix("https://github.com/") {
        value = rest.to_string();
    } else if let Some(rest) = value.strip_prefix("http://github.com/") {
        value = rest.to_string();
    } else if let Some(rest) = value.strip_prefix("git@github.com:") {
        value = rest.to_string();
    } else if let Some(rest) = value.strip_prefix("ssh://git@github.com/") {
        value = rest.to_string();
    }
    value = value
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string();
    let mut parts = value.split('/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if parts.next().is_some()
        || owner.is_empty()
        || repo.is_empty()
        || !owner.bytes().all(valid_repo_byte)
        || !repo.bytes().all(valid_repo_byte)
    {
        return None;
    }
    Some(format!(
        "{}/{}",
        owner.to_ascii_lowercase(),
        repo.to_ascii_lowercase()
    ))
}

fn valid_repo_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'~')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_identity_is_transport_only_and_case_normalized() {
        assert_eq!(
            normalize_repo_identity("git@github.com:Owner/Repo.git").as_deref(),
            Some("owner/repo")
        );
        assert_eq!(
            normalize_repo_identity("https://github.com/Owner/Repo/").as_deref(),
            Some("owner/repo")
        );
        assert_eq!(normalize_repo_identity("PB-123"), None);
        assert_eq!(normalize_repo_identity("github.com/owner/repo"), None);
    }

    #[test]
    fn workspace_repository_scope_is_exact() {
        let repos = json!([
            {"url": "https://github.com/Owner/Repo.git"},
            {"url": "https://github.com/other/else.git"}
        ]);
        assert!(workspace_contains_repo(&repos, "owner/repo"));
        assert!(!workspace_contains_repo(&repos, "owner/other"));
    }

    #[test]
    fn task_execution_workspace_must_match_known_task_paths() {
        assert!(task_execution_workspace_matches(
            Some("/srv/executions/task-1"),
            None,
            "/srv/executions/task-1/worktree"
        ));
        assert!(task_execution_workspace_matches(
            Some("/srv/executions/task-1/worktree"),
            None,
            "/srv/executions/task-1"
        ));
        assert!(!task_execution_workspace_matches(
            Some("/srv/executions/task-1"),
            None,
            "/srv/executions/task-1-other/worktree"
        ));
        assert!(!task_execution_workspace_matches(
            None,
            None,
            "/srv/executions/task-1"
        ));
    }

    #[test]
    fn provider_head_repository_must_match_exact_workspace_repository() {
        assert!(head_repository_matches(
            Some("https://github.com/Owner/Repo.git"),
            "owner/repo"
        ));
        assert!(!head_repository_matches(Some("owner/fork"), "owner/repo"));
        assert!(!head_repository_matches(None, "owner/repo"));
    }

    #[test]
    fn branch_discovery_requires_an_attached_non_default_head() {
        assert_eq!(
            classify_branch_discovery("default", 0, 1),
            BranchDiscoveryDecision::Ineligible("default_branch")
        );
        assert_eq!(
            classify_branch_discovery("detached", 0, 1),
            BranchDiscoveryDecision::Ineligible("detached_head")
        );
        assert_eq!(
            classify_branch_discovery("unknown", 0, 1),
            BranchDiscoveryDecision::Ineligible("unknown_head_state")
        );
    }

    #[test]
    fn branch_discovery_only_associates_one_safe_match() {
        assert_eq!(
            classify_branch_discovery("attached", 0, 0),
            BranchDiscoveryDecision::Unassociated
        );
        assert_eq!(
            classify_branch_discovery("attached", 0, 1),
            BranchDiscoveryDecision::Associated
        );
        assert_eq!(
            classify_branch_discovery("attached", 0, 2),
            BranchDiscoveryDecision::Ambiguous("multiple_pull_requests_for_exact_head")
        );
        assert_eq!(
            classify_branch_discovery("attached", 1, 1),
            BranchDiscoveryDecision::Ambiguous("branch_used_by_other_execution")
        );
    }
}
