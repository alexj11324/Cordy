use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Context as _;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use patchbay_db::models::{Automation, AutomationRun};
use patchbay_db::queries::automation::{
    advance_trigger_next_run, get_automation, get_automation_trigger,
    list_schedulable_automation_triggers, touch_automation_trigger_fired_at,
};
use patchbay_service::automation::AutomationService;
use patchbay_service::cron::{next_occurrence_after_utc, next_occurrences_utc};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::spec::{JobHandler, PlanProvider, ScopeProvider};
use crate::{CatchUpMode, HandlerResult, JobSpec, LatestPlanInfo, Scope};

pub const AUTOMATION_SCHEDULE_DISPATCH_JOB: &str = "automation_schedule_dispatch";
pub const AUTOMATION_TRIGGER_SCOPE: &str = "automation_trigger";
pub const DEFAULT_AUTOMATION_SCHEDULE_TIMEZONE: &str = "UTC";

const MAX_SCHEDULE_LATENESS: Duration = Duration::from_secs(5 * 60);
const REPLAY_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

#[async_trait]
pub trait AutomationScheduleDispatcher: Send + Sync {
    async fn dispatch_automation_for_plan(
        &self,
        automation: &Automation,
        trigger_id: Uuid,
        source: &str,
        payload: &Value,
        planned_at: DateTime<Utc>,
    ) -> anyhow::Result<AutomationRun>;
}

#[async_trait]
impl AutomationScheduleDispatcher for AutomationService {
    async fn dispatch_automation_for_plan(
        &self,
        automation: &Automation,
        trigger_id: Uuid,
        source: &str,
        payload: &Value,
        planned_at: DateTime<Utc>,
    ) -> anyhow::Result<AutomationRun> {
        AutomationService::dispatch_automation_for_plan(
            self, automation, trigger_id, source, payload, planned_at,
        )
        .await
    }
}

#[derive(Clone)]
struct TriggerConfig {
    cron_expression: String,
    timezone: String,
    created_at: Option<DateTime<Utc>>,
    last_fired_at: Option<DateTime<Utc>>,
}

#[derive(Default)]
struct TriggerCache(RwLock<HashMap<String, TriggerConfig>>);

impl TriggerCache {
    fn replace(&self, next: HashMap<String, TriggerConfig>) {
        *self
            .0
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
    }

    fn get(&self, id: &str) -> Option<TriggerConfig> {
        self.0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(id)
            .cloned()
    }
}

pub fn automation_schedule_dispatch_job(
    pool: PgPool,
    dispatcher: Arc<dyn AutomationScheduleDispatcher>,
) -> JobSpec {
    let cache = Arc::new(TriggerCache::default());
    JobSpec {
        name: AUTOMATION_SCHEDULE_DISPATCH_JOB.into(),
        cadence: Duration::ZERO,
        schedule_delay: Duration::ZERO,
        catch_up_mode: CatchUpMode::LatestOnly,
        catch_up_window: REPLAY_WINDOW,
        max_plans_per_tick: 5,
        run_timeout: Duration::from_secs(2 * 60),
        stale_timeout: Duration::from_secs(5 * 60),
        heartbeat_interval: Duration::from_secs(30),
        allow_stale_reentry: true,
        max_attempts: 3,
        retry_backoff: vec![
            Duration::from_secs(60),
            Duration::from_secs(5 * 60),
            Duration::from_secs(15 * 60),
        ],
        scopes: automation_scopes(pool.clone(), cache.clone()),
        plans_for_scope: Some(automation_plans_for_scope(cache)),
        handler: automation_handler(pool, dispatcher),
    }
}

fn automation_scopes(pool: PgPool, cache: Arc<TriggerCache>) -> ScopeProvider {
    Arc::new(move |_, _| {
        let pool = pool.clone();
        let cache = cache.clone();
        Box::pin(async move {
            let rows = list_schedulable_automation_triggers(&pool)
                .await
                .context("automation scope: list schedulable triggers")?;
            let mut next = HashMap::with_capacity(rows.len());
            let mut scopes = Vec::with_capacity(rows.len());
            for row in rows {
                let (Some(id), Some(cron_expression)) = (row.id, row.cron_expression) else {
                    continue;
                };
                if cron_expression.is_empty() {
                    continue;
                }
                let id = id.to_string();
                next.insert(
                    id.clone(),
                    TriggerConfig {
                        cron_expression,
                        timezone: row
                            .timezone
                            .filter(|value| !value.is_empty())
                            .unwrap_or_else(|| DEFAULT_AUTOMATION_SCHEDULE_TIMEZONE.into()),
                        created_at: row.created_at,
                        last_fired_at: row.last_fired_at,
                    },
                );
                scopes.push(Scope::new(AUTOMATION_TRIGGER_SCOPE, id));
            }
            cache.replace(next);
            Ok(scopes)
        })
    })
}

fn automation_plans_for_scope(cache: Arc<TriggerCache>) -> PlanProvider {
    Arc::new(move |_, scope, now, latest| {
        let cache = cache.clone();
        Box::pin(async move { plans_for_trigger(&cache, &scope, now, &latest) })
    })
}

fn plans_for_trigger(
    cache: &TriggerCache,
    scope: &Scope,
    now: DateTime<Utc>,
    latest: &LatestPlanInfo,
) -> anyhow::Result<Vec<DateTime<Utc>>> {
    let Some(config) = cache.get(&scope.id) else {
        return Ok(Vec::new());
    };

    if latest.retry_eligible(now) {
        return Ok(latest.plan_time.into_iter().collect());
    }

    let replay_floor = now - chrono::Duration::from_std(REPLAY_WINDOW)?;
    let anchor = latest
        .found
        .then_some(latest.plan_time)
        .flatten()
        .or(config.last_fired_at)
        .or(config.created_at)
        .unwrap_or(replay_floor)
        .max(replay_floor);
    let Some(latest_due) =
        next_occurrences_utc(&config.cron_expression, &config.timezone, anchor, now)?
            .into_iter()
            .next_back()
    else {
        return Ok(Vec::new());
    };

    if is_schedule_plan_stale(now, latest_due) {
        return Ok(Vec::new());
    }
    Ok(vec![latest_due])
}

fn automation_handler(
    pool: PgPool,
    dispatcher: Arc<dyn AutomationScheduleDispatcher>,
) -> JobHandler {
    Arc::new(move |_, input| {
        let pool = pool.clone();
        let dispatcher = dispatcher.clone();
        Box::pin(async move {
            let trigger_id = Uuid::parse_str(&input.scope.id)
                .context("automation handler: scope id is not a valid uuid")?;
            let Some(trigger) = get_automation_trigger(&pool, trigger_id)
                .await
                .context("load trigger")?
            else {
                return Ok(skipped("trigger_not_found"));
            };
            if !trigger.enabled || trigger.kind != "schedule" {
                return Ok(skipped("trigger_disabled"));
            }

            let Some(automation) = get_automation(&pool, trigger.automation_id)
                .await
                .context("load automation")?
            else {
                return Ok(skipped("automation_not_found"));
            };
            if automation.status != "active" {
                let mut result = skipped("automation_inactive");
                result
                    .result
                    .insert("status".into(), json!(automation.status));
                return Ok(result);
            }

            let run = dispatcher
                .dispatch_automation_for_plan(
                    &automation,
                    trigger.id,
                    "schedule",
                    &Value::Null,
                    input.plan_time,
                )
                .await
                .context("dispatch for plan")?;

            let timezone = trigger
                .timezone
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_AUTOMATION_SCHEDULE_TIMEZONE);
            let advanced = trigger
                .cron_expression
                .as_deref()
                .and_then(|cron| advanced_next_run(cron, timezone, input.plan_time, Utc::now()));
            if let Some(next) = advanced {
                let _ = advance_trigger_next_run(&pool, trigger.id, Some(next)).await;
            } else {
                let _ = touch_automation_trigger_fired_at(&pool, trigger.id).await;
            }

            Ok(HandlerResult {
                rows_affected: 1,
                result: Map::from_iter([
                    ("run_id".into(), json!(run.id.to_string())),
                    ("run_status".into(), json!(run.status)),
                ]),
            })
        })
    })
}

fn skipped(reason: &str) -> HandlerResult {
    HandlerResult {
        rows_affected: 0,
        result: Map::from_iter([("skipped_reason".into(), json!(reason))]),
    }
}

fn advanced_next_run(
    cron_expression: &str,
    timezone: &str,
    plan_time: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    next_occurrence_after_utc(cron_expression, timezone, now.max(plan_time)).ok()
}

fn is_schedule_plan_stale(now: DateTime<Utc>, plan_time: DateTime<Utc>) -> bool {
    now.signed_duration_since(plan_time)
        > chrono::Duration::from_std(MAX_SCHEDULE_LATENESS)
            .unwrap_or_else(|_| chrono::Duration::minutes(5))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    #[allow(clippy::expect_used)]
    fn at(hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 23, hour, minute, 0)
            .single()
            .expect("timestamp")
    }

    #[test]
    fn retry_keeps_the_failed_plan_time() {
        let cache = TriggerCache::default();
        cache.replace(HashMap::from([(
            "trigger".into(),
            TriggerConfig {
                cron_expression: "*/5 * * * *".into(),
                timezone: "UTC".into(),
                created_at: None,
                last_fired_at: None,
            },
        )]));
        let plan_time = at(12, 0);
        let latest = LatestPlanInfo {
            found: true,
            plan_time: Some(plan_time),
            status: "FAILED".into(),
            attempt: 1,
            max_attempts: 3,
            next_retry_at: Some(at(12, 1)),
        };
        assert_eq!(
            plans_for_trigger(
                &cache,
                &Scope::new(AUTOMATION_TRIGGER_SCOPE, "trigger"),
                at(12, 2),
                &latest,
            )
            .expect("plans"),
            vec![plan_time]
        );
    }

    #[test]
    fn latest_only_rejects_stale_catch_up() {
        let cache = TriggerCache::default();
        cache.replace(HashMap::from([(
            "trigger".into(),
            TriggerConfig {
                cron_expression: "0 * * * *".into(),
                timezone: "UTC".into(),
                created_at: Some(at(10, 0)),
                last_fired_at: None,
            },
        )]));
        assert!(plans_for_trigger(
            &cache,
            &Scope::new(AUTOMATION_TRIGGER_SCOPE, "trigger"),
            at(12, 10),
            &LatestPlanInfo::default(),
        )
        .expect("plans")
        .is_empty());
    }

    #[test]
    fn next_run_is_strictly_after_db_plan_when_local_clock_lags() {
        assert_eq!(
            advanced_next_run("*/5 * * * *", "UTC", at(12, 5), at(12, 4)),
            Some(at(12, 10))
        );
    }
}
