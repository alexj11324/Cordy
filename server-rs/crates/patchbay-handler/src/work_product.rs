//! Canonical Work Product APIs and task-owned exact-branch discovery.
//!
//! A provider object is mirrored independently from its association. The only
//! durable association paths are an authenticated task attach, an authenticated
//! member's explicit Attach operation, or the deterministic post-run lookup
//! performed from the same task's persisted execution provenance. PR text is
//! intentionally absent from this module: titles, bodies, and branch naming
//! conventions never establish identity or authorization.

use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use patchbay_db::models::{AgentTaskExecutionProvenance, WorkProduct, WorkProductRelation};
use patchbay_db::queries::{agent, github, work_product as work_product_q, workspace};
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
struct AttachExistingRequest {
    work_product_id: Uuid,
    #[serde(default)]
    close_intent: bool,
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

pub(crate) fn provenance_response(
    provenance: Option<&AgentTaskExecutionProvenance>,
) -> Value {
    let Some(provenance) = provenance else {
        return json!(null);
    };
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
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load work product");
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
    let relation = match attach_relation(
        &state,
        &issue,
        &product,
        &actor,
        request.close_intent,
    )
    .await
    {
        Ok(relation) => relation,
        Err(response) => return response,
    };
    publish_relation_event(&state, &issue, &product, &relation, &actor);
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
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list work products")
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
    let product = match work_product_q::get_work_product_by_id(
        &state.pool,
        issue.workspace_id,
        product_id,
    )
    .await
    {
        Ok(Some(product)) => product,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "work product not found"),
        Err(error) => {
            tracing::warn!(%error, "work product lookup failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load work product");
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
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to detach work product");
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
    Json(json!({ "detached": detached })).into_response()
}

pub(crate) async fn list_unassociated(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    match work_product_q::list_unassociated_work_products(
        &state.pool,
        context.member.workspace_id,
    )
    .await
    {
        Ok(products) => Json(json!({
            "work_products": products
                .iter()
                .map(|product| product_response(product, None))
                .collect::<Vec<_>>(),
        }))
        .into_response(),
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
    let task = match agent::get_agent_task_in_workspace(
        &state.pool,
        task_id,
        context.member.workspace_id,
    )
    .await
    {
        Ok(Some(task)) => task,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "task not found"),
        Err(error) => {
            tracing::warn!(%error, "task work product lookup failed");
            return error_response(StatusCode::NOT_FOUND, "task not found");
        }
    };
    let provenance = match work_product_q::get_execution_provenance(
        &state.pool,
        context.member.workspace_id,
        task.id,
    )
    .await
    {
        Ok(provenance) => provenance,
        Err(error) => {
            tracing::warn!(%error, task_id = %task.id, "execution provenance lookup failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load execution provenance");
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
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list task work products");
        }
    };
    Json(json!({
        "task_id": task.id,
        "provenance": provenance_response(provenance.as_ref()),
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
    let mut head_state = match input.head_state.as_str() {
        "attached" | "detached" | "default" | "unknown" => input.head_state.as_str(),
        _ => "unknown",
    };
    if head_branch.is_none() {
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
    let provenance = work_product_q::get_execution_provenance(&state.pool, workspace_id, task_id)
        .await
        .map_err(|_| "execution_provenance_unavailable")?
        .ok_or("execution_provenance_unavailable")?;
    if provenance.head_state != "attached" {
        return Err("execution_head_not_attached");
    }
    let Some(execution_workspace) = provenance.execution_workspace.as_deref() else {
        return Err("execution_workspace_unavailable");
    };
    if !std::path::Path::new(execution_workspace).is_absolute()
        || execution_workspace.contains('\0')
    {
        return Err("execution_workspace_invalid");
    }
    let task = agent::get_agent_task_in_workspace(&state.pool, task_id, workspace_id)
        .await
        .map_err(|_| "task_unavailable")?
        .ok_or("task_unavailable")?;
    if !task_execution_workspace_matches(
        task.work_dir.as_deref(),
        task.durable_work_dir.as_deref(),
        execution_workspace,
    ) {
        return Err("execution_workspace_not_owned_by_task");
    }
    let workspace = workspace::get_workspace(&state.pool, workspace_id)
        .await
        .map_err(|_| "workspace_unavailable")?
        .ok_or("workspace_unavailable")?;
    if !workspace_contains_repo(&workspace.repos, repo_identity) {
        return Err("repository_not_authorized_for_workspace");
    }
    if provenance.repo_identity.as_deref() != Some(repo_identity)
        || metadata.branch.is_empty()
        || provenance.head_branch.as_deref() != Some(metadata.branch.as_str())
    {
        return Err("provider_repository_or_branch_mismatch");
    }
    if metadata.head_repo_identity.is_none() {
        return Err("provider_head_repository_unavailable");
    }
    if !head_repository_matches(metadata.head_repo_identity.as_deref(), repo_identity) {
        return Err("provider_head_repository_mismatch");
    }
    Ok(())
}

/// Best-effort terminal discovery. No error from GitHub or the relation write
/// changes the already-recorded task outcome; every branch is written to the
/// provenance audit row so the UI can distinguish no match from uncertainty.
pub(crate) async fn discover_after_task(
    state: &HandlerState,
    task: &patchbay_db::models::AgentTaskQueue,
    workspace_id: Uuid,
    input: &ExecutionProvenanceInput,
) {
    let provenance = match record_execution_provenance(
        state,
        task.id,
        workspace_id,
        task.autopilot_run_id,
        input,
        true,
    )
    .await
    {
        Ok(provenance) => provenance,
        Err(error) => {
            tracing::warn!(%error, task_id = %task.id, "record terminal execution provenance failed");
            return;
        }
    };
    let workspace_id = provenance.workspace_id;

    let existing = match work_product_q::list_relations_for_task(&state.pool, workspace_id, task.id)
        .await
    {
        Ok(relations) => relations,
        Err(error) => {
            record_discovery_failure(
                state,
                workspace_id,
                task.id,
                work_product_q::DISCOVERY_INELIGIBLE,
                0,
                Some("relation_lookup_failed"),
            )
            .await;
            tracing::warn!(%error, task_id = %task.id, "skip branch discovery after relation lookup failure");
            return;
        }
    };
    if let Some(relation) = existing.first() {
        // A replay must not overwrite the original discovery outcome. The
        // first terminal delivery may have recorded a branch discovery (or a
        // deliberate unassociated/ambiguous result); later deliveries only
        // observe the durable relation and return.
        if provenance.discovery_status == work_product_q::DISCOVERY_NOT_ATTEMPTED {
            record_discovery_result(
                state,
                workspace_id,
                task.id,
                work_product_q::DISCOVERY_ASSOCIATED,
                0,
                Some("explicit_relation_already_exists"),
                Some(relation.work_product_id),
            )
            .await;
        }
        return;
    }

    // Discovery is a one-shot post-run operation. Once its outcome is audited,
    // later webhook/poller deliveries must never query GitHub by branch again.
    if provenance.discovery_status != work_product_q::DISCOVERY_NOT_ATTEMPTED {
        return;
    }

    let Some(repo_identity) = provenance.repo_identity.as_deref() else {
        record_discovery_failure(
            state,
            workspace_id,
            task.id,
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
            workspace_id,
            task.id,
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
            workspace_id,
            task.id,
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
            workspace_id,
            task.id,
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
            workspace_id,
            task.id,
            work_product_q::DISCOVERY_INELIGIBLE,
            0,
            Some("missing_head_branch"),
        )
        .await;
        return;
    };
    if let BranchDiscoveryDecision::Ineligible(reason) =
        classify_branch_discovery(&provenance.head_state, 0, 0)
    {
        record_discovery_failure(
            state,
            workspace_id,
            task.id,
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
            workspace_id,
            task.id,
            work_product_q::DISCOVERY_INELIGIBLE,
            0,
            Some("workspace_not_found"),
        )
        .await;
        return;
    };
    if !workspace_contains_repo(&workspace.repos, repo_identity) {
        record_discovery_failure(
            state,
            workspace_id,
            task.id,
            work_product_q::DISCOVERY_INELIGIBLE,
            0,
            Some("repository_not_authorized_for_workspace"),
        )
        .await;
        return;
    }
    if let Ok(other_executions) = work_product_q::list_other_branch_executions(
        &state.pool,
        workspace_id,
        repo_identity,
        head_branch,
        task.id,
    )
    .await
    {
        if let BranchDiscoveryDecision::Ambiguous(reason) =
            classify_branch_discovery("attached", other_executions.len(), 0)
        {
            record_discovery_failure(
                state,
                workspace_id,
                task.id,
                work_product_q::DISCOVERY_AMBIGUOUS,
                other_executions.len() as i32,
                Some(reason),
            )
            .await;
            return;
        }
    } else {
        record_discovery_failure(
            state,
            workspace_id,
            task.id,
            work_product_q::DISCOVERY_AMBIGUOUS,
            0,
            Some("active_execution_lookup_failed"),
        )
        .await;
        return;
    }

    let Some((owner, repo)) = split_repo_identity(repo_identity) else {
        record_discovery_failure(
            state,
            workspace_id,
            task.id,
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
            workspace_id,
            task.id,
            work_product_q::DISCOVERY_INELIGIBLE,
            0,
            Some("github_app_not_configured"),
        )
        .await;
        return;
    };
    let installations = match github::list_git_hub_installations_by_workspace(
        &state.pool,
        workspace_id,
    )
    .await
    {
        Ok(installations) => installations,
        Err(error) => {
            record_discovery_failure(
                state,
                workspace_id,
                task.id,
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
                    if !matches.iter().any(|found: &DiscoveredPullRequest| {
                        found.number == item.number
                    }) {
                        matches.push(DiscoveredPullRequest {
                            installation_id: installation.installation_id,
                            number: item.number,
                            metadata: item.metadata,
                        });
                    }
                }
            }
            Err(error) => {
                lookup_failed = true;
                tracing::warn!(
                    %error,
                    task_id = %task.id,
                    installation_id = installation.installation_id,
                    "branch discovery GitHub lookup failed"
                );
            }
        }
    }
    if lookup_failed {
        record_discovery_failure(
            state,
            workspace_id,
            task.id,
            work_product_q::DISCOVERY_AMBIGUOUS,
            matches.len() as i32,
            Some("github_pull_request_lookup_failed"),
        )
        .await;
        return;
    }
    match classify_branch_discovery("attached", 0, matches.len()) {
        BranchDiscoveryDecision::Unassociated => {
            record_discovery_failure(
                state,
                workspace_id,
                task.id,
                work_product_q::DISCOVERY_UNASSOCIATED,
                0,
                Some("no_pull_request_for_exact_head"),
            )
            .await;
        }
        BranchDiscoveryDecision::Ambiguous(reason) => {
            record_discovery_failure(
                state,
                workspace_id,
                task.id,
                work_product_q::DISCOVERY_AMBIGUOUS,
                matches.len() as i32,
                Some(reason),
            )
            .await;
        }
        BranchDiscoveryDecision::Associated => {
            let found = matches.pop().expect("one match");
            let metadata = found.metadata;
            let canonical_url = if metadata.html_url.is_empty() {
                format!("https://github.com/{owner}/{repo}/pull/{}", found.number)
            } else {
                metadata.html_url.clone()
            };
            let mirrored = match github::attach_git_hub_pull_request(
                &state.pool,
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
                    record_discovery_failure(
                        state,
                        workspace_id,
                        task.id,
                        work_product_q::DISCOVERY_UNASSOCIATED,
                        1,
                        Some("pull_request_mirror_not_written"),
                    )
                    .await;
                    return;
                }
                Err(error) => {
                    tracing::warn!(%error, task_id = %task.id, "branch discovery mirror write failed");
                    record_discovery_failure(
                        state,
                        workspace_id,
                        task.id,
                        work_product_q::DISCOVERY_UNASSOCIATED,
                        1,
                        Some("pull_request_mirror_write_failed"),
                    )
                    .await;
                    return;
                }
            };
            let product = match work_product_q::upsert_work_product(
                &state.pool,
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
                    tracing::warn!(%error, task_id = %task.id, "branch discovery work product write failed");
                    record_discovery_failure(
                        state,
                        workspace_id,
                        task.id,
                        work_product_q::DISCOVERY_UNASSOCIATED,
                        1,
                        Some("work_product_write_failed"),
                    )
                    .await;
                    return;
                }
            };
            let relation_key = work_product_q::relation_key(task.issue_id, Some(task.id), task.autopilot_run_id);
            let relation = match work_product_q::attach_work_product_relation(
                &state.pool,
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
                    tracing::warn!(%error, task_id = %task.id, "branch discovery relation write failed");
                    record_discovery_failure(
                        state,
                        workspace_id,
                        task.id,
                        work_product_q::DISCOVERY_UNASSOCIATED,
                        1,
                        Some("relation_write_failed"),
                    )
                    .await;
                    return;
                }
            };
            record_discovery_result(
                state,
                workspace_id,
                task.id,
                work_product_q::DISCOVERY_ASSOCIATED,
                1,
                Some("unique_pull_request_for_exact_head"),
                Some(product.id),
            )
            .await;
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
        BranchDiscoveryDecision::Ineligible(_) => unreachable!("attached head state is eligible"),
    }
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
    work_product_q::attach_work_product_relation(
        &state.pool,
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
    workspace_id: Uuid,
    task_id: Uuid,
    status: &str,
    match_count: i32,
    reason: Option<&str>,
    work_product_id: Option<Uuid>,
) {
    if let Err(error) = work_product_q::mark_execution_discovery(
        &state.pool,
        workspace_id,
        task_id,
        status,
        match_count,
        reason,
        work_product_id,
    )
    .await
    {
        tracing::warn!(%error, %task_id, "write work product discovery audit failed");
    }
}

async fn record_discovery_failure(
    state: &HandlerState,
    workspace_id: Uuid,
    task_id: Uuid,
    status: &str,
    match_count: i32,
    reason: Option<&str>,
) {
    record_discovery_result(
        state,
        workspace_id,
        task_id,
        status,
        match_count,
        reason,
        None,
    )
    .await;
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
            let path = std::path::Path::new(path);
            path.is_absolute()
                && !path.to_string_lossy().contains('\0')
                && path.starts_with(execution_workspace)
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
    value = value.trim_end_matches('/').trim_end_matches(".git").to_string();
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
    Some(format!("{}/{}", owner.to_ascii_lowercase(), repo.to_ascii_lowercase()))
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
            Some("/srv/executions/task-1/worktree"),
            None,
            "/srv/executions/task-1"
        ));
        assert!(!task_execution_workspace_matches(
            Some("/srv/executions/task-1/worktree"),
            None,
            "/srv/other"
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
