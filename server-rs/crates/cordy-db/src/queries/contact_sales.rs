//! Port of server/pkg/db/queries/contact_sales.sql (generated contact_sales.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn count_recent_contact_sales_by_email(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    business_email: &str,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT count(*) FROM contact_sales_inquiry
WHERE business_email = $1 AND created_at > now() - interval '1 hour'"#,
    )
    .bind(business_email)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn create_contact_sales_inquiry(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    first_name: &str,
    last_name: &str,
    business_email: &str,
    company_name: &str,
    company_size: &str,
    country_region: &str,
    use_case: &str,
    goals: &str,
    consent_outreach: bool,
    consent_updates: bool,
    user_agent: &str,
    submitter_ip: Option<ipnetwork::IpNetwork>,
) -> anyhow::Result<Option<ContactSalesInquiry>> {
    let row = sqlx::query(
        r#"INSERT INTO contact_sales_inquiry (
    first_name,
    last_name,
    business_email,
    company_name,
    company_size,
    country_region,
    use_case,
    goals,
    consent_outreach,
    consent_updates,
    submitter_ip,
    user_agent
)
VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
    $12::inet,
    $11
)
RETURNING id, first_name, last_name, business_email, company_name, company_size, country_region, use_case, goals, consent_outreach, consent_updates, submitter_ip, user_agent, created_at"#
    )
        .bind(first_name)
        .bind(last_name)
        .bind(business_email)
        .bind(company_name)
        .bind(company_size)
        .bind(country_region)
        .bind(use_case)
        .bind(goals)
        .bind(consent_outreach)
        .bind(consent_updates)
        .bind(user_agent)
        .bind(submitter_ip)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ContactSalesInquiry {
        id: row.try_get(0)?,
        first_name: row.try_get(1)?,
        last_name: row.try_get(2)?,
        business_email: row.try_get(3)?,
        company_name: row.try_get(4)?,
        company_size: row.try_get(5)?,
        country_region: row.try_get(6)?,
        use_case: row.try_get(7)?,
        goals: row.try_get(8)?,
        consent_outreach: row.try_get(9)?,
        consent_updates: row.try_get(10)?,
        submitter_ip: row.try_get(11)?,
        user_agent: row.try_get(12)?,
        created_at: row.try_get(13)?,
    }))
}
