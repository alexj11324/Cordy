//! Authenticated channel-account binding token redemption.
//!
//! These routes are intentionally not workspace-scoped: the token identifies
//! the workspace and channel account while the authenticated session supplies
//! the Cordy user. Token consumption, membership validation, and binding are
//! committed in one transaction so every rejected attempt leaves the token
//! redeemable by its intended user.

use std::sync::Arc;

use axum::body::{to_bytes, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use cordy_db::queries::channel::{
    consume_channel_binding_token, consume_lark_binding_token, create_channel_user_binding,
};
use cordy_db::queries::member::get_member_by_user_and_workspace;
use cordy_protocol::EVENT_DINGTALK_ACCOUNT_BINDING_UPDATED;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::error_response;
use crate::HandlerState;

const WECOM_BODY_LIMIT: usize = 16 * 1024;

/// Deployment gates for the channel integrations. They are kept in this
/// route-local state so the routes preserve Go's 503 behavior without making
/// the shared handler state depend on every connector crate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BindingRedeemAvailability {
    pub lark: bool,
    pub slack: bool,
    pub wecom: bool,
    pub dingtalk: bool,
    pub telegram: bool,
    pub weixin: bool,
}

impl BindingRedeemAvailability {
    pub fn from_env() -> Self {
        Self {
            lark: valid_secret_key("CORDY_LARK_SECRET_KEY"),
            slack: valid_secret_key("CORDY_SLACK_SECRET_KEY"),
            wecom: valid_secret_key("CORDY_WECOM_SECRET_KEY"),
            dingtalk: valid_secret_key("CORDY_DINGTALK_SECRET_KEY"),
            telegram: valid_secret_key("CORDY_TELEGRAM_SECRET_KEY"),
            weixin: valid_secret_key("CORDY_WEIXIN_SECRET_KEY"),
        }
    }

    #[cfg(test)]
    fn all_enabled() -> Self {
        Self {
            lark: true,
            slack: true,
            wecom: true,
            dingtalk: true,
            telegram: true,
            weixin: true,
        }
    }
}

fn valid_secret_key(name: &str) -> bool {
    cordy_util::secretbox::load_key(name).is_ok()
}

/// State consumed only by this router. Mount it into the authenticated group
/// with `binding_redeem::router().with_state(BindingRedeemState::from_handler(&state))`.
#[derive(Clone)]
pub struct BindingRedeemState {
    pool: sqlx::PgPool,
    bus: Arc<cordy_events::Bus>,
    availability: BindingRedeemAvailability,
}

impl BindingRedeemState {
    pub fn new(
        pool: sqlx::PgPool,
        bus: Arc<cordy_events::Bus>,
        availability: BindingRedeemAvailability,
    ) -> Self {
        Self {
            pool,
            bus,
            availability,
        }
    }

    pub fn from_handler(state: &HandlerState) -> Self {
        Self::new(
            state.pool.clone(),
            state.bus.clone(),
            BindingRedeemAvailability::from_env(),
        )
    }
}

pub fn router() -> Router<BindingRedeemState> {
    Router::new()
        .route("/api/lark/binding/redeem", post(redeem_lark))
        .route("/api/slack/binding/redeem", post(redeem_slack))
        .route("/api/wecom/binding/redeem", post(redeem_wecom))
        .route("/api/dingtalk/binding/redeem", post(redeem_dingtalk))
        .route("/api/telegram/binding/redeem", post(redeem_telegram))
        .route("/api/weixin/binding/redeem", post(redeem_weixin))
}

#[derive(Debug, Deserialize)]
struct RedeemRequest {
    token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Lark,
    Slack,
    Wecom,
    DingTalk,
    Telegram,
    Weixin,
}

impl Channel {
    fn channel_type(self) -> &'static str {
        match self {
            Self::Lark => "feishu",
            Self::Slack => "slack",
            Self::Wecom => "wecom",
            Self::DingTalk => "dingtalk",
            Self::Telegram => "telegram",
            Self::Weixin => "weixin",
        }
    }

    fn configured(self, availability: BindingRedeemAvailability) -> bool {
        match self {
            Self::Lark => availability.lark,
            Self::Slack => availability.slack,
            Self::Wecom => availability.wecom,
            Self::DingTalk => availability.dingtalk,
            Self::Telegram => availability.telegram,
            Self::Weixin => availability.weixin,
        }
    }

    fn unavailable_message(self) -> &'static str {
        match self {
            Self::Lark => "lark integration not configured",
            Self::Slack => "slack integration not configured",
            Self::Wecom => "wecom integration not configured",
            Self::DingTalk => "dingtalk integration not configured",
            Self::Telegram => "telegram integration not configured",
            Self::Weixin => "weixin integration not configured",
        }
    }

    fn conflict_message(self) -> &'static str {
        match self {
            Self::Lark => "this Lark account is already bound to a different Patchbay user",
            Self::Slack => "this Slack account is already bound to a different Patchbay user",
            Self::Wecom => "this WeCom user is already bound to a different Patchbay user",
            Self::DingTalk => "this DingTalk account is already bound to a different Patchbay user",
            Self::Telegram => "this Telegram account is already bound to a different Patchbay user",
            Self::Weixin => "this WeChat account is already bound to a different Patchbay user",
        }
    }

    fn validates_token_channel(self) -> bool {
        // This deliberately mirrors the authoritative Go services. The three
        // newer generic-channel adapters validate the discriminator; the
        // Lark and Slack services predate that guard.
        matches!(
            self,
            Self::Wecom | Self::DingTalk | Self::Telegram | Self::Weixin
        )
    }

    fn response_key(self) -> &'static str {
        match self {
            Self::Lark => "lark_open_id",
            Self::Slack => "slack_user_id",
            Self::Wecom => "wecom_user_id",
            Self::DingTalk => "dingtalk_user_id",
            Self::Telegram => "telegram_user_id",
            Self::Weixin => "weixin_user_id",
        }
    }

    fn binding_config(self) -> serde_json::Value {
        if self == Self::Lark {
            json!({ "union_id": null })
        } else {
            json!({})
        }
    }
}

#[derive(Debug)]
enum RedeemError {
    TokenInvalid,
    AlreadyAssigned,
    NotWorkspaceMember,
    Internal(anyhow::Error),
}

struct RedeemedBinding {
    workspace_id: Uuid,
    installation_id: Uuid,
    channel_user_id: String,
}

struct ConsumedToken {
    workspace_id: Uuid,
    installation_id: Uuid,
    channel_type: String,
    channel_user_id: String,
}

async fn redeem_lark(
    State(state): State<BindingRedeemState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    redeem(state, headers, body, Channel::Lark).await
}

async fn redeem_slack(
    State(state): State<BindingRedeemState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    redeem(state, headers, body, Channel::Slack).await
}

async fn redeem_wecom(
    State(state): State<BindingRedeemState>,
    request: Request<axum::body::Body>,
) -> Response {
    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, WECOM_BODY_LIMIT).await {
        Ok(body) => body,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    redeem(state, parts.headers, body, Channel::Wecom).await
}

async fn redeem_dingtalk(
    State(state): State<BindingRedeemState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    redeem(state, headers, body, Channel::DingTalk).await
}

async fn redeem_telegram(
    State(state): State<BindingRedeemState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    redeem(state, headers, body, Channel::Telegram).await
}

async fn redeem_weixin(
    State(state): State<BindingRedeemState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    redeem(state, headers, body, Channel::Weixin).await
}

async fn redeem(
    state: BindingRedeemState,
    headers: HeaderMap,
    body: Bytes,
    channel: Channel,
) -> Response {
    if !channel.configured(state.availability) {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            channel.unavailable_message(),
        );
    }
    let user_id = match authenticated_user_id(&headers) {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let request: RedeemRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.token.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "token is required");
    }

    let redeemed = match redeem_and_bind(&state.pool, channel, &request.token, user_id).await {
        Ok(redeemed) => redeemed,
        Err(RedeemError::TokenInvalid) => {
            return error_response(StatusCode::GONE, "binding token invalid or expired")
        }
        Err(RedeemError::AlreadyAssigned) => {
            return error_response(StatusCode::CONFLICT, channel.conflict_message())
        }
        Err(RedeemError::NotWorkspaceMember) => {
            return error_response(
                StatusCode::FORBIDDEN,
                "binding refused (are you a workspace member?)",
            )
        }
        Err(RedeemError::Internal(error)) => {
            tracing::error!(%error, channel = channel.channel_type(), "binding token redemption failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to redeem token");
        }
    };

    if channel == Channel::DingTalk {
        state.bus.publish(&cordy_events::Event {
            event_type: EVENT_DINGTALK_ACCOUNT_BINDING_UPDATED.into(),
            workspace_id: redeemed.workspace_id.to_string(),
            actor_type: "user".into(),
            actor_id: user_id.to_string(),
            payload: json!({ "id": redeemed.installation_id }),
            ..Default::default()
        });
    }

    let mut response = serde_json::Map::with_capacity(3);
    response.insert("workspace_id".into(), json!(redeemed.workspace_id));
    response.insert("installation_id".into(), json!(redeemed.installation_id));
    response.insert(
        channel.response_key().into(),
        json!(redeemed.channel_user_id),
    );
    Json(serde_json::Value::Object(response)).into_response()
}

fn authenticated_user_id(headers: &HeaderMap) -> Result<Uuid, Response> {
    let Some(raw) = headers.get("x-user-id") else {
        return Err(error_response(
            StatusCode::UNAUTHORIZED,
            "user not authenticated",
        ));
    };
    let raw = match raw.to_str() {
        Ok(raw) if !raw.is_empty() => raw,
        _ => {
            return Err(error_response(
                StatusCode::UNAUTHORIZED,
                "user not authenticated",
            ))
        }
    };
    Uuid::parse_str(raw).map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid user id"))
}

async fn redeem_and_bind(
    pool: &sqlx::PgPool,
    channel: Channel,
    raw_token: &str,
    cordy_user_id: Uuid,
) -> Result<RedeemedBinding, RedeemError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(anyhow::Error::from)
        .map_err(RedeemError::Internal)?;
    let token_hash = hex::encode(Sha256::digest(raw_token.as_bytes()));
    let row = if channel == Channel::Lark {
        let row = consume_lark_binding_token(&mut *tx, &token_hash)
            .await
            .map_err(RedeemError::Internal)?
            .ok_or(RedeemError::TokenInvalid)?;
        ConsumedToken {
            workspace_id: row.workspace_id,
            installation_id: row.installation_id,
            channel_type: channel.channel_type().to_string(),
            channel_user_id: row.lark_open_id,
        }
    } else {
        let row = consume_channel_binding_token(&mut *tx, &token_hash)
            .await
            .map_err(RedeemError::Internal)?
            .ok_or(RedeemError::TokenInvalid)?;
        ConsumedToken {
            workspace_id: row.workspace_id,
            installation_id: row.installation_id,
            channel_type: row.channel_type,
            channel_user_id: row.channel_user_id,
        }
    };

    if channel.validates_token_channel() && row.channel_type != channel.channel_type() {
        return Err(RedeemError::TokenInvalid);
    }
    let member = get_member_by_user_and_workspace(&mut *tx, cordy_user_id, row.workspace_id)
        .await
        .map_err(RedeemError::Internal)?;
    if member.is_none() {
        return Err(RedeemError::NotWorkspaceMember);
    }

    let binding = create_channel_user_binding(
        &mut *tx,
        row.workspace_id,
        cordy_user_id,
        row.installation_id,
        channel.channel_type(),
        &row.channel_user_id,
        &channel.binding_config(),
    )
    .await
    .map_err(RedeemError::Internal)?;
    if binding.is_none() {
        return Err(RedeemError::AlreadyAssigned);
    }

    tx.commit()
        .await
        .map_err(anyhow::Error::from)
        .map_err(RedeemError::Internal)?;
    Ok(RedeemedBinding {
        workspace_id: row.workspace_id,
        installation_id: row.installation_id,
        channel_user_id: row.channel_user_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    fn lazy_pool() -> sqlx::PgPool {
        sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap()
    }

    fn test_app(availability: BindingRedeemAvailability) -> Router {
        let state = BindingRedeemState::new(
            lazy_pool(),
            Arc::new(cordy_events::Bus::new()),
            availability,
        );
        router().with_state(state)
    }

    async fn response_json(response: Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn disabled_integrations_return_channel_specific_service_unavailable() {
        let cases = [
            ("lark", "lark integration not configured"),
            ("slack", "slack integration not configured"),
            ("wecom", "wecom integration not configured"),
            ("dingtalk", "dingtalk integration not configured"),
            ("telegram", "telegram integration not configured"),
            ("weixin", "weixin integration not configured"),
        ];
        for (channel, message) in cases {
            let response = test_app(BindingRedeemAvailability::default())
                .oneshot(
                    Request::post(format!("/api/{channel}/binding/redeem"))
                        .header("x-user-id", Uuid::new_v4().to_string())
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"token":"secret"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(response_json(response).await["error"], message);
        }
    }

    #[tokio::test]
    async fn enabled_routes_validate_auth_and_body_before_database_access() {
        let app = test_app(BindingRedeemAvailability::all_enabled());
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/lark/binding/redeem")
                    .body(Body::from(r#"{"token":"secret"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .clone()
            .oneshot(
                Request::post("/api/slack/binding/redeem")
                    .header("x-user-id", "not-a-uuid")
                    .body(Body::from(r#"{"token":"secret"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        for body in ["not json", r#"{"token":""}"#] {
            let response = app
                .clone()
                .oneshot(
                    Request::post("/api/telegram/binding/redeem")
                        .header("x-user-id", Uuid::new_v4().to_string())
                        .header("content-type", "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn wecom_rejects_body_above_the_go_sixteen_kib_limit() {
        let response = test_app(BindingRedeemAvailability::all_enabled())
            .oneshot(
                Request::post("/api/wecom/binding/redeem")
                    .header("x-user-id", Uuid::new_v4().to_string())
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"token":"{}"}}"#,
                        "x".repeat(WECOM_BODY_LIMIT)
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(response).await["error"],
            "invalid request body"
        );
    }

    #[test]
    fn channel_wire_contracts_match_go() {
        let cases = [
            (Channel::Lark, "feishu", "lark_open_id", false),
            (Channel::Slack, "slack", "slack_user_id", false),
            (Channel::Wecom, "wecom", "wecom_user_id", true),
            (Channel::DingTalk, "dingtalk", "dingtalk_user_id", true),
            (Channel::Telegram, "telegram", "telegram_user_id", true),
        ];
        for (channel, channel_type, response_key, validates_type) in cases {
            assert_eq!(channel.channel_type(), channel_type);
            assert_eq!(channel.response_key(), response_key);
            assert_eq!(channel.validates_token_channel(), validates_type);
        }
    }
}
