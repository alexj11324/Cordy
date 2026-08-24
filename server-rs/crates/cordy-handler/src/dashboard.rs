//! Workspace dashboard rollups.

use std::collections::{HashMap, HashSet};

use axum::extract::{Extension, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Days, Duration, LocalResult, TimeZone, Utc};
use chrono_tz::Tz;
use cordy_db::models::AgentInvocationTarget;
use cordy_db::queries::{agent, agent_invocation_target, task_usage, user};
use cordy_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const RESTRICTED_AGENT_ID: &str = "__restricted_agents__";

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/dashboard/usage/daily", get(usage_daily))
        .route("/api/dashboard/usage/by-agent", get(usage_by_agent))
        .route("/api/dashboard/agent-runtime", get(agent_runtime))
        .route("/api/dashboard/runtime/daily", get(runtime_daily))
        .route("/api/dashboard/failures/daily", get(failures_daily))
        .route("/api/dashboard/failures/by-agent", get(failures_by_agent))
}

#[derive(Debug, Default, Deserialize)]
struct Params {
    days: Option<String>,
    tz: Option<String>,
    project_id: Option<String>,
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace id"))
}

fn project_id(params: &Params) -> Result<Option<Uuid>, Response> {
    match params.project_id.as_deref() {
        None | Some("") => Ok(None),
        Some(raw) => Uuid::parse_str(raw)
            .map(Some)
            .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid project_id")),
    }
}

async fn viewing_tz(state: &HandlerState, context: &WorkspaceContext, params: &Params) -> Tz {
    if let Some(tz) = params
        .tz
        .as_deref()
        .map(str::trim)
        .filter(|tz| !tz.is_empty())
        .and_then(|tz| tz.parse().ok())
    {
        return tz;
    }
    if let Ok(Some(current)) = user::get_user(&state.pool, context.member.user_id).await {
        if let Some(tz) = current
            .timezone
            .as_deref()
            .map(str::trim)
            .filter(|tz| !tz.is_empty())
            .and_then(|tz| tz.parse().ok())
        {
            return tz;
        }
    }
    chrono_tz::UTC
}

fn cutoff(params: &Params, tz: Tz, exact: bool) -> DateTime<Utc> {
    cutoff_at(params, tz, exact, Utc::now())
}

fn cutoff_at(params: &Params, tz: Tz, exact: bool, now: DateTime<Utc>) -> DateTime<Utc> {
    let days = params
        .days
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|days| (1..=365).contains(days))
        .unwrap_or(30);
    let lookback = if exact { days - 1 } else { days };
    let local = now.with_timezone(&tz);
    let target_date = local
        .date_naive()
        .checked_sub_days(Days::new(lookback as u64))
        .expect("dashboard lookback is capped at 365 days");
    let target_midnight = target_date
        .and_hms_opt(0, 0, 0)
        .expect("midnight is always a valid naive time");
    let zoned = match tz.from_local_datetime(&target_midnight) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(earlier, _) => earlier,
        LocalResult::None => (1..=180)
            .find_map(|minute| {
                tz.from_local_datetime(&(target_midnight + Duration::minutes(minute)))
                    .earliest()
            })
            .expect("IANA offset transitions are shorter than three hours"),
    };
    zoned.with_timezone(&Utc)
}

fn member_can_view(
    candidate: &cordy_db::models::Agent,
    targets: &[AgentInvocationTarget],
    user_id: Uuid,
) -> bool {
    candidate.owner_id == Some(user_id)
        || (candidate.permission_mode == "public_to"
            && targets.iter().any(|target| {
                target.target_type == "workspace"
                    || (target.target_type == "member" && target.target_id == user_id)
            }))
}

async fn restricted_agents(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    workspace_id: Uuid,
) -> Result<HashSet<Uuid>, Response> {
    let agents = agent::list_all_agents_any_kind(&state.pool, workspace_id)
        .await
        .map_err(|error| {
            tracing::warn!(%error, %workspace_id, "failed to list agents for dashboard access");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to resolve agent access",
            )
        })?;
    let (actor_type, _, _) = crate::issue::mutation_actor(state, context, headers).await;
    let judge_user_agents =
        actor_type == "member" && !matches!(context.member.role.as_str(), "owner" | "admin");
    let mut targets_by_agent: HashMap<Uuid, Vec<AgentInvocationTarget>> = HashMap::new();
    if judge_user_agents {
        let targets = agent_invocation_target::list_agent_invocation_targets_by_agent_i_ds(
            &state.pool,
            agents.iter().map(|candidate| candidate.id).collect(),
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, %workspace_id, "failed to list dashboard invocation targets");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to resolve agent access",
            )
        })?;
        for target in targets {
            targets_by_agent
                .entry(target.agent_id)
                .or_default()
                .push(target);
        }
    }
    Ok(agents
        .into_iter()
        .filter(|candidate| {
            candidate.kind != "user"
                || (judge_user_agents
                    && !member_can_view(
                        candidate,
                        targets_by_agent
                            .get(&candidate.id)
                            .map(Vec::as_slice)
                            .unwrap_or_default(),
                        context.member.user_id,
                    ))
        })
        .map(|candidate| candidate.id)
        .collect())
}

fn date(value: Option<chrono::NaiveDate>) -> String {
    value
        .map(|value| value.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

fn id(value: Option<Uuid>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
struct UsageDaily {
    date: String,
    provider: String,
    model: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost_usd_ticks: i64,
    uncosted_input_tokens: i64,
    uncosted_output_tokens: i64,
    uncosted_cache_read_tokens: i64,
    uncosted_cache_write_tokens: i64,
    task_count: i32,
}

#[derive(Debug, Clone, Serialize)]
struct UsageByAgent {
    agent_id: String,
    provider: String,
    model: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost_usd_ticks: i64,
    uncosted_input_tokens: i64,
    uncosted_output_tokens: i64,
    uncosted_cache_read_tokens: i64,
    uncosted_cache_write_tokens: i64,
    task_count: i32,
}

#[derive(Debug, Clone, Serialize)]
struct AgentRuntime {
    agent_id: String,
    total_seconds: i64,
    task_count: i32,
    failed_count: i32,
    cancelled_count: i32,
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeDaily {
    date: String,
    total_seconds: i64,
    task_count: i32,
    failed_count: i32,
    cancelled_count: i32,
}

#[derive(Debug, Clone, Serialize)]
struct FailureDaily {
    date: String,
    failure_reason: String,
    task_count: i32,
}

#[derive(Debug, Clone, Serialize)]
struct FailureByAgent {
    agent_id: String,
    failure_reason: String,
    task_count: i32,
}

async fn usage_daily(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<Params>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let project_id = match project_id(&params) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let tz = viewing_tz(&state, &context, &params).await;
    match task_usage::list_dashboard_usage_daily(
        &state.pool,
        workspace_id,
        &tz.to_string(),
        Some(cutoff(&params, tz, false)),
        project_id,
    )
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| UsageDaily {
                    date: date(row.date),
                    provider: row.provider,
                    model: row.model,
                    input_tokens: row.input_tokens,
                    output_tokens: row.output_tokens,
                    cache_read_tokens: row.cache_read_tokens,
                    cache_write_tokens: row.cache_write_tokens,
                    cost_usd_ticks: row.cost_usd_ticks,
                    uncosted_input_tokens: row.uncosted_input_tokens,
                    uncosted_output_tokens: row.uncosted_output_tokens,
                    uncosted_cache_read_tokens: row.uncosted_cache_read_tokens,
                    uncosted_cache_write_tokens: row.uncosted_cache_write_tokens,
                    task_count: row.task_count,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list usage"),
    }
}

fn fold_usage(rows: Vec<UsageByAgent>, restricted: &HashSet<Uuid>) -> Vec<UsageByAgent> {
    let restricted: HashSet<String> = restricted.iter().map(ToString::to_string).collect();
    let mut out: Vec<UsageByAgent> = Vec::with_capacity(rows.len());
    let mut buckets: HashMap<(String, String), usize> = HashMap::new();
    for mut row in rows {
        if !restricted.contains(&row.agent_id) {
            out.push(row);
            continue;
        }
        row.agent_id = RESTRICTED_AGENT_ID.into();
        let key = (row.provider.clone(), row.model.clone());
        if let Some(index) = buckets.get(&key).copied() {
            let dst = &mut out[index];
            dst.input_tokens += row.input_tokens;
            dst.output_tokens += row.output_tokens;
            dst.cache_read_tokens += row.cache_read_tokens;
            dst.cache_write_tokens += row.cache_write_tokens;
            dst.cost_usd_ticks += row.cost_usd_ticks;
            dst.uncosted_input_tokens += row.uncosted_input_tokens;
            dst.uncosted_output_tokens += row.uncosted_output_tokens;
            dst.uncosted_cache_read_tokens += row.uncosted_cache_read_tokens;
            dst.uncosted_cache_write_tokens += row.uncosted_cache_write_tokens;
            dst.task_count += row.task_count;
        } else {
            buckets.insert(key, out.len());
            out.push(row);
        }
    }
    out
}

async fn usage_by_agent(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Query(params): Query<Params>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let project_id = match project_id(&params) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let restricted = match restricted_agents(&state, &context, &headers, workspace_id).await {
        Ok(ids) => ids,
        Err(r) => return r,
    };
    let tz = viewing_tz(&state, &context, &params).await;
    match task_usage::list_dashboard_usage_by_agent(
        &state.pool,
        workspace_id,
        Some(cutoff(&params, tz, true)),
        project_id,
    )
    .await
    {
        Ok(rows) => Json(fold_usage(
            rows.into_iter()
                .map(|row| UsageByAgent {
                    agent_id: id(row.agent_id),
                    provider: row.provider,
                    model: row.model,
                    input_tokens: row.input_tokens,
                    output_tokens: row.output_tokens,
                    cache_read_tokens: row.cache_read_tokens,
                    cache_write_tokens: row.cache_write_tokens,
                    cost_usd_ticks: row.cost_usd_ticks,
                    uncosted_input_tokens: row.uncosted_input_tokens,
                    uncosted_output_tokens: row.uncosted_output_tokens,
                    uncosted_cache_read_tokens: row.uncosted_cache_read_tokens,
                    uncosted_cache_write_tokens: row.uncosted_cache_write_tokens,
                    task_count: row.task_count,
                })
                .collect(),
            &restricted,
        ))
        .into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list usage by agent",
        ),
    }
}

fn fold_runtime(rows: Vec<AgentRuntime>, restricted: &HashSet<Uuid>) -> Vec<AgentRuntime> {
    let restricted: HashSet<String> = restricted.iter().map(ToString::to_string).collect();
    let mut out = Vec::with_capacity(rows.len());
    let mut bucket = None;
    for mut row in rows {
        if !restricted.contains(&row.agent_id) {
            out.push(row);
            continue;
        }
        row.agent_id = RESTRICTED_AGENT_ID.into();
        if let Some(index) = bucket {
            let dst: &mut AgentRuntime = &mut out[index];
            dst.total_seconds += row.total_seconds;
            dst.task_count += row.task_count;
            dst.failed_count += row.failed_count;
            dst.cancelled_count += row.cancelled_count;
        } else {
            bucket = Some(out.len());
            out.push(row);
        }
    }
    out
}

async fn agent_runtime(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Query(params): Query<Params>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let project_id = match project_id(&params) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let restricted = match restricted_agents(&state, &context, &headers, workspace_id).await {
        Ok(ids) => ids,
        Err(r) => return r,
    };
    let tz = viewing_tz(&state, &context, &params).await;
    match task_usage::list_dashboard_agent_run_time(
        &state.pool,
        workspace_id,
        Some(cutoff(&params, tz, true)),
        project_id,
    )
    .await
    {
        Ok(rows) => Json(fold_runtime(
            rows.into_iter()
                .map(|row| AgentRuntime {
                    agent_id: id(row.agent_id),
                    total_seconds: row.total_seconds,
                    task_count: row.task_count,
                    failed_count: row.failed_count,
                    cancelled_count: row.cancelled_count,
                })
                .collect(),
            &restricted,
        ))
        .into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list agent runtime",
        ),
    }
}

async fn runtime_daily(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<Params>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let project_id = match project_id(&params) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let tz = viewing_tz(&state, &context, &params).await;
    match task_usage::list_dashboard_run_time_daily(
        &state.pool,
        workspace_id,
        &tz.to_string(),
        Some(cutoff(&params, tz, false)),
        project_id,
    )
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| RuntimeDaily {
                    date: date(row.date),
                    total_seconds: row.total_seconds,
                    task_count: row.task_count,
                    failed_count: row.failed_count,
                    cancelled_count: row.cancelled_count,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list daily runtime",
        ),
    }
}

async fn failures_daily(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<Params>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let project_id = match project_id(&params) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let tz = viewing_tz(&state, &context, &params).await;
    match task_usage::list_dashboard_failures_daily(
        &state.pool,
        workspace_id,
        &tz.to_string(),
        Some(cutoff(&params, tz, false)),
        project_id,
    )
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| FailureDaily {
                    date: date(row.date),
                    failure_reason: row.failure_reason,
                    task_count: row.task_count,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list daily failures",
        ),
    }
}

fn fold_failures(rows: Vec<FailureByAgent>, restricted: &HashSet<Uuid>) -> Vec<FailureByAgent> {
    let restricted: HashSet<String> = restricted.iter().map(ToString::to_string).collect();
    let mut out: Vec<FailureByAgent> = Vec::with_capacity(rows.len());
    let mut buckets: HashMap<String, usize> = HashMap::new();
    for mut row in rows {
        if !restricted.contains(&row.agent_id) {
            out.push(row);
            continue;
        }
        row.agent_id = RESTRICTED_AGENT_ID.into();
        if let Some(index) = buckets.get(&row.failure_reason).copied() {
            out[index].task_count += row.task_count;
        } else {
            buckets.insert(row.failure_reason.clone(), out.len());
            out.push(row);
        }
    }
    out
}

async fn failures_by_agent(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Query(params): Query<Params>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let project_id = match project_id(&params) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let restricted = match restricted_agents(&state, &context, &headers, workspace_id).await {
        Ok(ids) => ids,
        Err(r) => return r,
    };
    let tz = viewing_tz(&state, &context, &params).await;
    match task_usage::list_dashboard_failures_by_agent(
        &state.pool,
        workspace_id,
        Some(cutoff(&params, tz, true)),
        project_id,
    )
    .await
    {
        Ok(rows) => Json(fold_failures(
            rows.into_iter()
                .map(|row| FailureByAgent {
                    agent_id: id(row.agent_id),
                    failure_reason: row.failure_reason,
                    task_count: row.task_count,
                })
                .collect(),
            &restricted,
        ))
        .into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list failures by agent",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutoffs_preserve_daily_headroom_and_exact_agent_window() {
        let params = Params {
            days: Some("1".into()),
            tz: None,
            project_id: None,
        };
        let now = Utc.with_ymd_and_hms(2026, 8, 23, 12, 0, 0).unwrap();
        let daily = cutoff_at(&params, chrono_tz::UTC, false, now);
        let exact = cutoff_at(&params, chrono_tz::UTC, true, now);
        assert_eq!(exact - daily, Duration::days(1));
    }

    #[test]
    fn cutoffs_step_back_by_calendar_day_across_dst_boundaries() {
        let tz: Tz = "America/New_York".parse().unwrap();
        let spring = Params {
            days: Some("168".into()),
            tz: None,
            project_id: None,
        };
        let spring_now = Utc.with_ymd_and_hms(2026, 8, 23, 12, 0, 0).unwrap();
        assert_eq!(
            cutoff_at(&spring, tz, false, spring_now),
            Utc.with_ymd_and_hms(2026, 3, 8, 5, 0, 0).unwrap()
        );

        let fall = Params {
            days: Some("8".into()),
            tz: None,
            project_id: None,
        };
        let fall_now = Utc.with_ymd_and_hms(2026, 11, 9, 12, 0, 0).unwrap();
        assert_eq!(
            cutoff_at(&fall, tz, false, fall_now),
            Utc.with_ymd_and_hms(2026, 11, 1, 4, 0, 0).unwrap()
        );
    }

    #[test]
    fn restricted_rows_fold_without_losing_totals() {
        let hidden = Uuid::new_v4();
        let rows = vec![
            AgentRuntime {
                agent_id: hidden.to_string(),
                total_seconds: 3,
                task_count: 1,
                failed_count: 1,
                cancelled_count: 0,
            },
            AgentRuntime {
                agent_id: hidden.to_string(),
                total_seconds: 4,
                task_count: 2,
                failed_count: 0,
                cancelled_count: 1,
            },
        ];
        let folded = fold_runtime(rows, &HashSet::from([hidden]));
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].agent_id, RESTRICTED_AGENT_ID);
        assert_eq!(folded[0].total_seconds, 7);
        assert_eq!(folded[0].task_count, 3);
    }
}
