//! Runtime detail usage and activity reports.

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Days, Duration, LocalResult, TimeZone, Utc};
use chrono_tz::Tz;
use patchbay_db::models::AgentRuntime;
use patchbay_db::queries::{member, runtime, runtime_usage, user};
use patchbay_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/runtimes/{runtime_id}/usage", get(usage))
        .route(
            "/api/runtimes/{runtime_id}/usage/by-agent",
            get(usage_by_agent),
        )
        .route(
            "/api/runtimes/{runtime_id}/usage/by-hour",
            get(usage_by_hour),
        )
        .route("/api/runtimes/{runtime_id}/activity", get(activity))
}

#[derive(Debug, Default, Deserialize)]
struct Params {
    days: Option<String>,
    tz: Option<String>,
}

async fn load_runtime(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_id: &str,
) -> Result<AgentRuntime, Response> {
    let runtime_id = Uuid::parse_str(raw_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"))?;
    let found = runtime::get_agent_runtime(&state.pool, runtime_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "runtime not found"))?;
    member::get_member_by_user_and_workspace(
        &state.pool,
        context.member.user_id,
        found.workspace_id,
    )
    .await
    .ok()
    .flatten()
    .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "runtime not found"))?;
    Ok(found)
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

fn cutoff(params: &Params, tz: Tz, default_days: i64) -> DateTime<Utc> {
    cutoff_at(params, tz, default_days, Utc::now())
}

fn cutoff_at(params: &Params, tz: Tz, default_days: i64, now: DateTime<Utc>) -> DateTime<Utc> {
    let days = params
        .days
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|days| (1..=365).contains(days))
        .unwrap_or(default_days);
    let target_date = now
        .with_timezone(&tz)
        .date_naive()
        .checked_sub_days(Days::new(days as u64))
        .expect("runtime usage lookback is capped at 365 days");
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

#[derive(Debug, Serialize)]
struct Usage {
    runtime_id: String,
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
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
struct UsageByHour {
    hour: i32,
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

#[derive(Debug, Serialize)]
struct HourlyActivity {
    hour: i32,
    count: i32,
}

async fn usage(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    Query(params): Query<Params>,
) -> Response {
    let runtime = match load_runtime(&state, &context, &raw_id).await {
        Ok(runtime) => runtime,
        Err(response) => return response,
    };
    let tz = viewing_tz(&state, &context, &params).await;
    match runtime_usage::list_runtime_usage(
        &state.pool,
        runtime.id,
        &tz.to_string(),
        Some(cutoff(&params, tz, 90)),
    )
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| Usage {
                    runtime_id: runtime.id.to_string(),
                    date: row
                        .date
                        .map(|date| date.format("%Y-%m-%d").to_string())
                        .unwrap_or_default(),
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
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list usage"),
    }
}

async fn usage_by_agent(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    Query(params): Query<Params>,
) -> Response {
    let runtime = match load_runtime(&state, &context, &raw_id).await {
        Ok(runtime) => runtime,
        Err(response) => return response,
    };
    let tz = viewing_tz(&state, &context, &params).await;
    match runtime_usage::list_runtime_usage_by_agent(
        &state.pool,
        runtime.id,
        Some(cutoff(&params, tz, 30)),
    )
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| UsageByAgent {
                    agent_id: row.agent_id.map(|id| id.to_string()).unwrap_or_default(),
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
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list usage by agent",
        ),
    }
}

async fn usage_by_hour(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    Query(params): Query<Params>,
) -> Response {
    let runtime = match load_runtime(&state, &context, &raw_id).await {
        Ok(runtime) => runtime,
        Err(response) => return response,
    };
    let tz = viewing_tz(&state, &context, &params).await;
    match runtime_usage::get_runtime_usage_by_hour(
        &state.pool,
        runtime.id,
        &tz.to_string(),
        Some(cutoff(&params, tz, 30)),
    )
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| UsageByHour {
                    hour: row.hour,
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
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to get usage by hour",
        ),
    }
}

async fn activity(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    Query(params): Query<Params>,
) -> Response {
    let runtime = match load_runtime(&state, &context, &raw_id).await {
        Ok(runtime) => runtime,
        Err(response) => return response,
    };
    let tz = viewing_tz(&state, &context, &params).await;
    match runtime_usage::get_runtime_task_hourly_activity(&state.pool, runtime.id, &tz.to_string())
        .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| HourlyActivity {
                    hour: row.hour,
                    count: row.count,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to get task activity",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_cutoff_uses_local_calendar_days_across_dst() {
        let params = Params {
            days: Some("168".into()),
            tz: None,
        };
        let tz: Tz = "America/New_York".parse().unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 23, 12, 0, 0).unwrap();
        assert_eq!(
            cutoff_at(&params, tz, 30, now),
            Utc.with_ymd_and_hms(2026, 3, 8, 5, 0, 0).unwrap()
        );
    }

    #[test]
    fn invalid_days_falls_back_to_endpoint_default() {
        let params = Params {
            days: Some("366".into()),
            tz: None,
        };
        let now = Utc.with_ymd_and_hms(2026, 8, 23, 12, 0, 0).unwrap();
        assert_eq!(
            cutoff_at(&params, chrono_tz::UTC, 30, now),
            Utc.with_ymd_and_hms(2026, 7, 24, 0, 0, 0).unwrap()
        );
    }
}
