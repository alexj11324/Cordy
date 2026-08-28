//! Public contact-sales inquiry endpoint.

use std::net::SocketAddr;

use axum::body::{to_bytes, Body, Bytes};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use ipnetwork::IpNetwork;
use serde::{Deserialize, Serialize};
use structured_email_address::{Config as EmailConfig, EmailAddress};

use crate::error::error_response;
use crate::state::HandlerState;

const BODY_LIMIT: usize = 16 * 1024;
const MAX_FIRST_NAME: usize = 80;
const MAX_LAST_NAME: usize = 80;
const MAX_EMAIL: usize = 254;
const MAX_COMPANY_NAME: usize = 200;
const MAX_COUNTRY_REGION: usize = 80;
const MAX_GOALS: usize = 2_000;
const HOURLY_EMAIL_CAP: i64 = 3;

const COMPANY_SIZES: &[&str] = &["1-10", "11-50", "51-200", "201-500", "501-1000", "1000+"];
const USE_CASES: &[&str] = &[
    "evaluate",
    "adopt_team",
    "self_host",
    "integrate",
    "partner",
    "other",
];
const FREE_EMAIL_DOMAINS: &[&str] = &[
    "gmail.com",
    "googlemail.com",
    "outlook.com",
    "hotmail.com",
    "live.com",
    "msn.com",
    "yahoo.com",
    "yahoo.co.uk",
    "yahoo.co.jp",
    "ymail.com",
    "icloud.com",
    "me.com",
    "mac.com",
    "aol.com",
    "protonmail.com",
    "proton.me",
    "pm.me",
    "gmx.com",
    "gmx.de",
    "mail.com",
    "zoho.com",
    "yandex.com",
    "yandex.ru",
    "qq.com",
    "163.com",
    "126.com",
    "sina.com",
    "foxmail.com",
];

pub fn router() -> Router<HandlerState> {
    Router::new().route("/api/contact-sales", post(create))
}

#[derive(Debug, Deserialize)]
struct CreateRequest {
    #[serde(default)]
    first_name: String,
    #[serde(default)]
    last_name: String,
    #[serde(default)]
    business_email: String,
    #[serde(default)]
    company_name: String,
    #[serde(default)]
    company_size: String,
    #[serde(default)]
    country_region: String,
    #[serde(default)]
    use_case: String,
    #[serde(default)]
    goals: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    consent_outreach: bool,
    #[serde(default)]
    consent_updates: bool,
}

struct ValidatedRequest {
    first_name: String,
    last_name: String,
    business_email: String,
    company_name: String,
    company_size: String,
    country_region: String,
    use_case: String,
    goals: String,
    source: String,
    consent_outreach: bool,
    consent_updates: bool,
}

#[derive(Serialize)]
struct CreateResponse {
    id: String,
    created_at: String,
}

fn required(raw: &str, field: &str, max: usize) -> Result<String, Response> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            &format!("{field} is required"),
        ));
    }
    if value.len() > max {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            &format!("{field} is too long"),
        ));
    }
    Ok(value.to_string())
}

fn canonical_business_email(raw: &str) -> Option<String> {
    let config = EmailConfig::builder()
        .allow_display_name()
        .allow_domain_literal()
        .allow_single_label_domain()
        .lowercase_all()
        .build();
    let parsed = EmailAddress::parse_with(raw.trim(), &config).ok()?;
    let email = parsed.canonical();
    (!parsed.local_part().is_empty() && !parsed.domain().is_empty()).then_some(email)
}

fn validate(request: CreateRequest) -> Result<ValidatedRequest, Response> {
    let first_name = required(&request.first_name, "first_name", MAX_FIRST_NAME)?;
    let last_name = required(&request.last_name, "last_name", MAX_LAST_NAME)?;
    let company_name = required(&request.company_name, "company_name", MAX_COMPANY_NAME)?;
    let business_email = canonical_business_email(&request.business_email)
        .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "business_email is invalid"))?;
    if business_email.len() > MAX_EMAIL {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "business_email is too long",
        ));
    }
    let domain = business_email.rsplit_once('@').map(|(_, domain)| domain);
    if domain.is_none_or(|domain| FREE_EMAIL_DOMAINS.contains(&domain)) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "please use a business email address",
        ));
    }
    let company_size = request.company_size.trim().to_string();
    if !COMPANY_SIZES.contains(&company_size.as_str()) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "company_size is invalid",
        ));
    }
    let country_region = request.country_region.trim().to_string();
    if country_region.is_empty() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "country_region is required",
        ));
    }
    if country_region.len() > MAX_COUNTRY_REGION {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "country_region is too long",
        ));
    }
    let use_case = request.use_case.trim().to_string();
    if !USE_CASES.contains(&use_case.as_str()) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "use_case is invalid",
        ));
    }
    let goals = request.goals.trim().to_string();
    if goals.len() > MAX_GOALS {
        return Err(error_response(StatusCode::BAD_REQUEST, "goals is too long"));
    }
    let source = match request.source.trim() {
        "" => "page".to_string(),
        source => source.to_string(),
    };
    Ok(ValidatedRequest {
        first_name,
        last_name,
        business_email,
        company_name,
        company_size,
        country_region,
        use_case,
        goals,
        source,
        consent_outreach: request.consent_outreach,
        consent_updates: request.consent_updates,
    })
}

fn truncate_header(value: Option<&str>, max: usize) -> String {
    let value = value.unwrap_or_default();
    if value.len() <= max {
        return value.to_string();
    }
    let mut end = max;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

async fn create(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    body: Body,
) -> Response {
    let body = match bounded_body(body).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let request = match serde_json::from_slice::<CreateRequest>(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let request = match validate(request) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let count = match patchbay_db::queries::contact_sales::count_recent_contact_sales_by_email(
        &state.pool,
        &request.business_email,
    )
    .await
    {
        Ok(count) => count.unwrap_or_default(),
        Err(error) => {
            tracing::warn!(%error, "count recent contact sales failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to submit inquiry",
            );
        }
    };
    if count >= HOURLY_EMAIL_CAP {
        return error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "too many recent inquiries from this email",
        );
    }

    let submitter_ip = peer.map(|Extension(ConnectInfo(peer))| IpNetwork::from(peer.ip()));
    let user_agent = truncate_header(
        headers
            .get("user-agent")
            .and_then(|value| value.to_str().ok()),
        512,
    );
    let inquiry = match patchbay_db::queries::contact_sales::create_contact_sales_inquiry(
        &state.pool,
        &request.first_name,
        &request.last_name,
        &request.business_email,
        &request.company_name,
        &request.company_size,
        &request.country_region,
        &request.use_case,
        &request.goals,
        request.consent_outreach,
        request.consent_updates,
        &user_agent,
        submitter_ip,
    )
    .await
    {
        Ok(Some(inquiry)) => inquiry,
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to submit inquiry",
            )
        }
    };

    let event = patchbay_analytics::events::contact_sales_submitted(
        &inquiry.id.to_string(),
        &request.company_size,
        &request.country_region,
        &request.use_case,
        &request.source,
        !request.goals.is_empty(),
    );
    patchbay_metrics::business_events::record_event(
        None,
        state.business_metrics.as_deref(),
        &event,
    );
    (
        StatusCode::CREATED,
        Json(CreateResponse {
            id: inquiry.id.to_string(),
            created_at: crate::timefmt::rfc3339(inquiry.created_at),
        }),
    )
        .into_response()
}

async fn bounded_body(body: Body) -> Result<Bytes, Response> {
    to_bytes(body, BODY_LIMIT)
        .await
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid request body"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt;

    fn valid_request() -> CreateRequest {
        CreateRequest {
            first_name: " Ada ".into(),
            last_name: " Lovelace ".into(),
            business_email: "Ada <ADA@Analytical.Engine>".into(),
            company_name: " Analytical Engine ".into(),
            company_size: "11-50".into(),
            country_region: "UK".into(),
            use_case: "evaluate".into(),
            goals: " Pilot Patchbay ".into(),
            source: String::new(),
            consent_outreach: true,
            consent_updates: false,
        }
    }

    #[test]
    fn validation_canonicalizes_the_go_contract() {
        let request = validate(valid_request()).expect("valid inquiry");
        assert_eq!(request.first_name, "Ada");
        assert_eq!(request.business_email, "ada@analytical.engine");
        assert_eq!(request.company_name, "Analytical Engine");
        assert_eq!(request.goals, "Pilot Patchbay");
        assert_eq!(request.source, "page");
    }

    #[test]
    fn validation_rejects_free_email_and_closed_enums() {
        let mut request = valid_request();
        request.business_email = "ada@gmail.com".into();
        assert!(validate(request).is_err());

        let mut request = valid_request();
        request.company_size = "lots".into();
        assert!(validate(request).is_err());

        let mut request = valid_request();
        request.use_case = "surprise".into();
        assert!(validate(request).is_err());
    }

    #[test]
    fn user_agent_truncation_stays_on_a_utf8_boundary() {
        let value = format!("{}x", "好".repeat(171));
        let truncated = truncate_header(Some(&value), 512);
        assert!(truncated.len() <= 512);
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[tokio::test]
    async fn public_route_is_mounted_without_authentication() {
        let response = crate::build_router(None, None)
            .oneshot(
                Request::post("/api/contact-sales")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn body_reader_enforces_the_16_kib_boundary() {
        assert_eq!(
            bounded_body(Body::from(vec![b'x'; BODY_LIMIT]))
                .await
                .expect("body at limit")
                .len(),
            BODY_LIMIT
        );
        assert!(bounded_body(Body::from(vec![b'x'; BODY_LIMIT + 1]))
            .await
            .is_err());
    }
}
