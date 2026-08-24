//! Current-user workspace invitation reads and decisions.

use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cordy_db::models::WorkspaceInvitation;
use cordy_db::queries::invitation::{
    self, ListPendingInvitationsByWorkspaceRow, ListPendingInvitationsForUserRow,
};
use cordy_db::queries::{member, user, workspace};
use cordy_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;
use crate::workspace::MemberWithUserResponse;

#[derive(Clone, Default)]
pub struct InvitationAdmission {
    entries: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
}

const REDIS_CHECK_SCRIPT: &str = r#"
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', ARGV[1])
if redis.call('ZCARD', KEYS[1]) >= tonumber(ARGV[2]) then
    return 0
end
return 1
"#;

const REDIS_CONSUME_SCRIPT: &str = r#"
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', ARGV[2])
if redis.call('ZCARD', KEYS[1]) >= tonumber(ARGV[3]) then
    return 0
end
redis.call('ZADD', KEYS[1], ARGV[1], ARGV[5])
redis.call('EXPIRE', KEYS[1], ARGV[4])
return 1
"#;

const INVITATION_REDIS_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug)]
enum AdmissionError {
    Limited {
        retry_after: u64,
        gates: Vec<&'static str>,
    },
    Unavailable,
}

struct AdmissionGate {
    name: &'static str,
    key: String,
    limit: usize,
    window: Duration,
}

impl InvitationAdmission {
    fn gates(actor_id: Uuid, workspace_id: Uuid, email: &str) -> [AdmissionGate; 3] {
        let recipient = hex::encode(Sha256::digest(email.trim().to_ascii_lowercase().as_bytes()));
        [
            AdmissionGate {
                name: "actor",
                key: format!("mul:invitation:actor:{actor_id}"),
                limit: 10,
                window: Duration::from_secs(600),
            },
            AdmissionGate {
                name: "workspace",
                key: format!("mul:invitation:workspace:{workspace_id}"),
                limit: 50,
                window: Duration::from_secs(86_400),
            },
            AdmissionGate {
                name: "recipient",
                key: format!("mul:invitation:recipient:{recipient}"),
                limit: 6,
                window: Duration::from_secs(86_400),
            },
        ]
    }

    async fn admit(
        &self,
        redis: Option<&redis::Client>,
        actor_id: Uuid,
        workspace_id: Uuid,
        email: &str,
    ) -> Result<(), AdmissionError> {
        let gates = Self::gates(actor_id, workspace_id, email);
        if let Some(client) = redis {
            return Self::admit_redis(client, &gates).await;
        }
        self.admit_memory(&gates).await
    }

    async fn admit_memory(&self, gates: &[AdmissionGate]) -> Result<(), AdmissionError> {
        let now = Instant::now();
        let mut entries = self.entries.lock().await;
        let mut retry_after = 0_u64;
        let mut denied = Vec::new();
        for gate in gates {
            let values = entries.entry(gate.key.clone()).or_default();
            while values
                .front()
                .is_some_and(|created| now.duration_since(*created) >= gate.window)
            {
                values.pop_front();
            }
            if values.len() >= gate.limit {
                let remaining = gate
                    .window
                    .saturating_sub(now.duration_since(*values.front().unwrap()));
                retry_after = retry_after.max(remaining.as_secs().max(1));
                denied.push(gate.name);
            }
        }
        if retry_after > 0 {
            return Err(AdmissionError::Limited {
                retry_after,
                gates: denied,
            });
        }
        for gate in gates {
            entries.entry(gate.key.clone()).or_default().push_back(now);
        }
        Ok(())
    }

    async fn admit_redis(
        client: &redis::Client,
        gates: &[AdmissionGate],
    ) -> Result<(), AdmissionError> {
        use std::time::{SystemTime, UNIX_EPOCH};

        let mut conn = match tokio::time::timeout(
            INVITATION_REDIS_TIMEOUT,
            client.get_multiplexed_async_connection(),
        )
        .await
        {
            Ok(Ok(conn)) => conn,
            Ok(Err(error)) => {
                tracing::error!(%error, "invitation rate limiter unavailable");
                return Err(AdmissionError::Unavailable);
            }
            Err(_) => {
                tracing::error!("invitation rate limiter connection timed out");
                return Err(AdmissionError::Unavailable);
            }
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let now_nanos = now.as_nanos().min(i64::MAX as u128) as i64;
        let mut denied = Vec::new();
        let mut retry_after = 0_u64;
        for gate in gates {
            let cutoff =
                now_nanos.saturating_sub(gate.window.as_nanos().min(i64::MAX as u128) as i64);
            let check = redis::Script::new(REDIS_CHECK_SCRIPT);
            let operation = check
                .key(&gate.key)
                .arg(cutoff)
                .arg(gate.limit)
                .invoke_async::<i64>(&mut conn);
            let allowed = match tokio::time::timeout(INVITATION_REDIS_TIMEOUT, operation).await {
                Ok(Ok(allowed)) => allowed,
                Ok(Err(error)) => {
                    tracing::error!(%error, gate = gate.name, "invitation rate limiter unavailable");
                    return Err(AdmissionError::Unavailable);
                }
                Err(_) => {
                    tracing::error!(gate = gate.name, "invitation rate limiter check timed out");
                    return Err(AdmissionError::Unavailable);
                }
            };
            if allowed == 0 {
                denied.push(gate.name);
                retry_after = retry_after.max(gate.window.as_secs().max(1));
            }
        }
        if !denied.is_empty() {
            return Err(AdmissionError::Limited {
                retry_after,
                gates: denied,
            });
        }

        for gate in gates {
            let cutoff =
                now_nanos.saturating_sub(gate.window.as_nanos().min(i64::MAX as u128) as i64);
            let consume = redis::Script::new(REDIS_CONSUME_SCRIPT);
            let operation = consume
                .key(&gate.key)
                .arg(now_nanos)
                .arg(cutoff)
                .arg(gate.limit)
                .arg(gate.window.as_secs().saturating_mul(2).max(1))
                .arg(Uuid::new_v4().to_string())
                .invoke_async::<i64>(&mut conn);
            match tokio::time::timeout(INVITATION_REDIS_TIMEOUT, operation).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, gate = gate.name, "invitation rate limiter consume failed after successful checks; allowing bounded overshoot");
                }
                Err(_) => {
                    tracing::warn!(gate = gate.name, "invitation rate limiter consume timed out after successful checks; allowing bounded overshoot");
                }
            }
        }
        Ok(())
    }
}

fn invitation_rate_limited(retry_after: u64) -> Response {
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(serde_json::json!({
            "error": "too many invitation requests",
            "code": "invitation_rate_limited",
            "retry_after_seconds": retry_after,
        })),
    )
        .into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    if let Ok(value) = axum::http::HeaderValue::from_str(&retry_after.to_string()) {
        response
            .headers_mut()
            .insert(axum::http::header::RETRY_AFTER, value);
    }
    response
}

fn invitation_limiter_unavailable() -> Response {
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": "invitation service temporarily unavailable",
            "code": "invitation_rate_limiter_unavailable",
        })),
    )
        .into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        axum::http::header::RETRY_AFTER,
        axum::http::HeaderValue::from_static("5"),
    );
    response
}

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/invitations", get(list))
        .route("/api/invitations/{id}", get(get_one))
        .route("/api/invitations/{id}/accept", axum::routing::post(accept))
        .route(
            "/api/invitations/{id}/decline",
            axum::routing::post(decline),
        )
}

pub fn workspace_member_router() -> Router<HandlerState> {
    Router::new().route("/api/workspaces/{id}/invitations", get(list_workspace))
}

pub fn workspace_admin_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/workspaces/{id}/members", post(create))
        .route(
            "/api/workspaces/{id}/invitations/{invitation_id}",
            axum::routing::delete(revoke),
        )
}

#[derive(Debug, Serialize)]
struct InvitationResponse {
    id: String,
    workspace_id: String,
    inviter_id: String,
    invitee_email: String,
    invitee_user_id: Option<String>,
    role: String,
    status: String,
    created_at: String,
    updated_at: String,
    expires_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    inviter_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    inviter_email: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    workspace_name: String,
}

#[derive(Deserialize)]
struct CreateInvitationRequest {
    email: String,
    #[serde(default)]
    role: String,
}

async fn create(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    body: axum::body::Bytes,
) -> Response {
    let request: CreateInvitationRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let email = request.email.trim().to_ascii_lowercase();
    if email.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "email is required");
    }
    let role = match request.role.trim() {
        "" | "member" => "member",
        "admin" => "admin",
        "owner" => return error_response(StatusCode::BAD_REQUEST, "cannot invite as owner"),
        _ => return error_response(StatusCode::BAD_REQUEST, "invalid member role"),
    };
    let existing_user = match user::get_user_by_email(&state.pool, &email).await {
        Ok(user) => user,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load user"),
    };
    if let Some(found) = existing_user.as_ref() {
        if member::get_member_by_user_and_workspace(
            &state.pool,
            found.id,
            context.member.workspace_id,
        )
        .await
        .ok()
        .flatten()
        .is_some()
        {
            return error_response(StatusCode::CONFLICT, "user is already a member");
        }
    }
    if invitation::expire_stale_pending_invitations(
        &state.pool,
        context.member.workspace_id,
        &email,
    )
    .await
    .is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create invitation",
        );
    }
    match invitation::get_pending_invitation_by_email(
        &state.pool,
        context.member.workspace_id,
        &email,
    )
    .await
    {
        Ok(Some(_)) => {
            return error_response(
                StatusCode::CONFLICT,
                "invitation already pending for this email",
            )
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(%error, "failed to check pending invitation");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create invitation",
            );
        }
    }
    match state
        .invitation_admission
        .admit(
            state.rate_limit_client.as_ref(),
            context.member.user_id,
            context.member.workspace_id,
            &email,
        )
        .await
    {
        Ok(()) => {}
        Err(AdmissionError::Limited { retry_after, gates }) => {
            if let Some(metrics) = state.business_metrics.as_deref() {
                for gate in gates {
                    metrics.record_email_rate_limited("workspace_invitation", gate);
                }
            }
            return invitation_rate_limited(retry_after);
        }
        Err(AdmissionError::Unavailable) => return invitation_limiter_unavailable(),
    }
    let invitation = match invitation::create_invitation(
        &state.pool,
        context.member.workspace_id,
        context.member.user_id,
        &email,
        existing_user.as_ref().map(|user| user.id),
        role,
    )
    .await
    {
        Ok(Some(invitation)) => invitation,
        Err(error) if crate::workspace::unique_violation(&error) => {
            return error_response(
                StatusCode::CONFLICT,
                "invitation already pending for this email",
            )
        }
        _ => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create invitation",
            )
        }
    };
    let response = InvitationResponse::from(invitation.clone());
    let workspace_name = workspace::get_workspace(&state.pool, context.member.workspace_id)
        .await
        .ok()
        .flatten()
        .map(|workspace| workspace.name)
        .unwrap_or_default();
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::events::EVENT_INVITATION_CREATED.into(),
        workspace_id: context.workspace_id.clone(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload: serde_json::json!({"invitation": &response, "workspace_name": workspace_name}),
        ..Default::default()
    });
    let event = cordy_analytics::team_invite_sent(
        &context.member.user_id.to_string(),
        &context.workspace_id,
        &email,
        "email",
    );
    state.analytics.capture(event.clone());
    if let Some(metrics) = state.business_metrics.as_deref() {
        metrics.inc_for_event(&event);
    }
    if !workspace_name.is_empty() {
        let email_service = state.email_service.clone();
        let invitation_id = invitation.id.to_string();
        let inviter_name = user::get_user(&state.pool, context.member.user_id)
            .await
            .ok()
            .flatten()
            .map(|user| user.name)
            .unwrap_or_else(|| email.clone());
        let target = email.clone();
        tokio::spawn(async move {
            if let Err(error) = email_service
                .send_invitation_email(&target, &inviter_name, &workspace_name, &invitation_id)
                .await
            {
                tracing::warn!(%error, email = %target, "failed to send invitation email");
            }
        });
    }
    (StatusCode::CREATED, Json(response)).into_response()
}

impl From<WorkspaceInvitation> for InvitationResponse {
    fn from(invitation: WorkspaceInvitation) -> Self {
        Self {
            id: invitation.id.to_string(),
            workspace_id: invitation.workspace_id.to_string(),
            inviter_id: invitation.inviter_id.to_string(),
            invitee_email: invitation.invitee_email,
            invitee_user_id: invitation.invitee_user_id.map(|id| id.to_string()),
            role: invitation.role,
            status: invitation.status,
            created_at: crate::timefmt::rfc3339(invitation.created_at),
            updated_at: crate::timefmt::rfc3339(invitation.updated_at),
            expires_at: crate::timefmt::rfc3339(invitation.expires_at),
            inviter_name: String::new(),
            inviter_email: String::new(),
            workspace_name: String::new(),
        }
    }
}

fn option_uuid(value: Option<Uuid>) -> String {
    value.map(|id| id.to_string()).unwrap_or_default()
}

fn option_time(value: Option<chrono::DateTime<chrono::Utc>>) -> String {
    value.map(crate::timefmt::rfc3339).unwrap_or_default()
}

impl From<ListPendingInvitationsForUserRow> for InvitationResponse {
    fn from(row: ListPendingInvitationsForUserRow) -> Self {
        Self {
            id: option_uuid(row.id),
            workspace_id: option_uuid(row.workspace_id),
            inviter_id: option_uuid(row.inviter_id),
            invitee_email: row.invitee_email,
            invitee_user_id: row.invitee_user_id.map(|id| id.to_string()),
            role: row.role,
            status: row.status,
            created_at: option_time(row.created_at),
            updated_at: option_time(row.updated_at),
            expires_at: option_time(row.expires_at),
            inviter_name: row.inviter_name,
            inviter_email: row.inviter_email,
            workspace_name: row.workspace_name,
        }
    }
}

impl From<ListPendingInvitationsByWorkspaceRow> for InvitationResponse {
    fn from(row: ListPendingInvitationsByWorkspaceRow) -> Self {
        Self {
            id: option_uuid(row.id),
            workspace_id: option_uuid(row.workspace_id),
            inviter_id: option_uuid(row.inviter_id),
            invitee_email: row.invitee_email,
            invitee_user_id: row.invitee_user_id.map(|id| id.to_string()),
            role: row.role,
            status: row.status,
            created_at: option_time(row.created_at),
            updated_at: option_time(row.updated_at),
            expires_at: option_time(row.expires_at),
            inviter_name: row.inviter_name,
            inviter_email: row.inviter_email,
            workspace_name: String::new(),
        }
    }
}

fn user_id(headers: &HeaderMap) -> Result<Uuid, Response> {
    headers
        .get("x-user-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| error_response(StatusCode::UNAUTHORIZED, "user not authenticated"))
}

fn invitation_id(raw: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(raw)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid invitation id"))
}

fn belongs_to_user(invitation: &WorkspaceInvitation, user_id: Uuid, email: &str) -> bool {
    email.to_lowercase() == invitation.invitee_email || invitation.invitee_user_id == Some(user_id)
}

fn acceptance_metric_events(
    user_id: Uuid,
    workspace_id: Uuid,
    invitation: &WorkspaceInvitation,
    first_onboarding_completion: bool,
    onboarded_user: &cordy_db::models::User,
) -> Vec<cordy_analytics::Event> {
    let days_since_invite =
        (chrono::Utc::now() - invitation.created_at).num_seconds() / (24 * 60 * 60);
    let mut events = vec![cordy_analytics::team_invite_accepted(
        &user_id.to_string(),
        &workspace_id.to_string(),
        days_since_invite,
    )];
    if first_onboarding_completion {
        let onboarded_at = onboarded_user
            .onboarded_at
            .map(crate::timefmt::rfc3339)
            .unwrap_or_default();
        events.push(cordy_analytics::onboarding_completed(
            &user_id.to_string(),
            &workspace_id.to_string(),
            cordy_analytics::ONBOARDING_PATH_INVITE_ACCEPT,
            &onboarded_at,
            onboarded_user.cloud_waitlist_email.is_some(),
        ));
    }
    events
}

async fn load_user_and_invitation(
    state: &HandlerState,
    user_id: Uuid,
    invitation_id: Uuid,
) -> Result<(cordy_db::models::User, WorkspaceInvitation), Response> {
    let invitation = match invitation::get_invitation(&state.pool, invitation_id).await {
        Ok(Some(invitation)) => invitation,
        Ok(None) | Err(_) => {
            return Err(error_response(
                StatusCode::NOT_FOUND,
                "invitation not found",
            ))
        }
    };
    let user = match user::get_user(&state.pool, user_id).await {
        Ok(Some(user)) => user,
        Ok(None) | Err(_) => {
            return Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load user",
            ))
        }
    };
    if !belongs_to_user(&invitation, user_id, &user.email) {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "invitation does not belong to you",
        ));
    }
    Ok((user, invitation))
}

async fn list(State(state): State<HandlerState>, headers: HeaderMap) -> Response {
    let user_id = match user_id(&headers) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let user = match user::get_user(&state.pool, user_id).await {
        Ok(Some(user)) => user,
        Ok(None) | Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load user")
        }
    };
    match invitation::list_pending_invitations_for_user(&state.pool, user.id, &user.email).await {
        Ok(rows) => Json(
            rows.into_iter()
                .map(InvitationResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, %user_id, "failed to list invitations");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list invitations",
            )
        }
    }
}

async fn list_workspace(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    match invitation::list_pending_invitations_by_workspace(
        &state.pool,
        context.member.workspace_id,
    )
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(InvitationResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, workspace_id = %context.workspace_id, "failed to list invitations");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list invitations",
            )
        }
    }
}

#[derive(Deserialize)]
struct WorkspaceInvitationPath {
    invitation_id: String,
}

async fn revoke(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(path): Path<WorkspaceInvitationPath>,
) -> Response {
    let id = match invitation_id(&path.invitation_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let found = match invitation::get_invitation(&state.pool, id).await {
        Ok(Some(found))
            if found.workspace_id == context.member.workspace_id && found.status == "pending" =>
        {
            found
        }
        Ok(_) | Err(_) => return error_response(StatusCode::NOT_FOUND, "invitation not found"),
    };
    if let Err(error) = invitation::revoke_invitation(&state.pool, found.id).await {
        tracing::warn!(%error, invitation_id = %found.id, "failed to revoke invitation");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to revoke invitation",
        );
    }
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::events::EVENT_INVITATION_REVOKED.into(),
        workspace_id: context.workspace_id,
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload: serde_json::json!({
            "invitation_id": found.id.to_string(),
            "invitee_email": found.invitee_email,
            "invitee_user_id": found.invitee_user_id,
        }),
        ..Default::default()
    });
    StatusCode::NO_CONTENT.into_response()
}

async fn get_one(
    State(state): State<HandlerState>,
    Path(raw_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let user_id = match user_id(&headers) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let id = match invitation_id(&raw_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let (_, invitation) = match load_user_and_invitation(&state, user_id, id).await {
        Ok(values) => values,
        Err(response) => return response,
    };
    let mut response = InvitationResponse::from(invitation.clone());
    if let Ok(Some(found)) = workspace::get_workspace(&state.pool, invitation.workspace_id).await {
        response.workspace_name = found.name;
    }
    if let Ok(Some(inviter)) = user::get_user(&state.pool, invitation.inviter_id).await {
        response.inviter_name = inviter.name;
        response.inviter_email = inviter.email;
    }
    Json(response).into_response()
}

async fn accept(
    State(state): State<HandlerState>,
    Path(raw_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let user_id = match user_id(&headers) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let id = match invitation_id(&raw_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let (current_user, invitation) = match load_user_and_invitation(&state, user_id, id).await {
        Ok(values) => values,
        Err(response) => return response,
    };
    if invitation.status != "pending" {
        return error_response(StatusCode::BAD_REQUEST, "invitation is not pending");
    }
    if invitation.expires_at < chrono::Utc::now() {
        return error_response(StatusCode::GONE, "invitation has expired");
    }
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to accept invitation",
            )
        }
    };
    let accepted = match invitation::accept_invitation(&mut *transaction, invitation.id).await {
        Ok(Some(accepted)) => accepted,
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to accept invitation",
            )
        }
    };
    let joined_member = match member::create_member(
        &mut *transaction,
        accepted.workspace_id,
        current_user.id,
        &accepted.role,
    )
    .await
    {
        Ok(Some(member)) => member,
        Err(error) if crate::workspace::unique_violation(&error) => {
            return error_response(
                StatusCode::CONFLICT,
                "you are already a member of this workspace",
            )
        }
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create membership",
            )
        }
    };
    let first_onboarding_completion = current_user.onboarded_at.is_none();
    let onboarded_user = match user::mark_user_onboarded(&mut *transaction, current_user.id).await {
        Ok(Some(user)) => user,
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to mark user onboarded",
            )
        }
    };
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, invitation_id = %accepted.id, "failed to commit invitation acceptance");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to accept invitation",
        );
    }

    let member_response =
        MemberWithUserResponse::new_resolved(&state, &joined_member, &current_user);
    let workspace_id = accepted.workspace_id.to_string();
    let mut member_payload = serde_json::json!({"member": &member_response});
    if let Ok(Some(found)) = workspace::get_workspace(&state.pool, accepted.workspace_id).await {
        member_payload["workspace_name"] = serde_json::Value::String(found.name);
    }
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::events::EVENT_MEMBER_ADDED.into(),
        workspace_id: workspace_id.clone(),
        actor_type: "member".into(),
        actor_id: user_id.to_string(),
        payload: member_payload,
        ..Default::default()
    });
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::events::EVENT_INVITATION_ACCEPTED.into(),
        workspace_id: workspace_id.clone(),
        actor_type: "member".into(),
        actor_id: user_id.to_string(),
        payload: serde_json::json!({
            "invitation_id": accepted.id.to_string(),
            "member": &member_response,
        }),
        ..Default::default()
    });
    if let Some(hub) = state.daemon_hub.as_ref() {
        hub.notify_workspaces_changed(&user_id.to_string());
    }
    if let Some(metrics) = state.business_metrics.as_deref() {
        for event in acceptance_metric_events(
            user_id,
            accepted.workspace_id,
            &invitation,
            first_onboarding_completion,
            &onboarded_user,
        ) {
            metrics.inc_for_event(&event);
        }
    }
    Json(member_response).into_response()
}

async fn decline(
    State(state): State<HandlerState>,
    Path(raw_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let user_id = match user_id(&headers) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let id = match invitation_id(&raw_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let (_, invitation) = match load_user_and_invitation(&state, user_id, id).await {
        Ok(values) => values,
        Err(response) => return response,
    };
    if invitation.status != "pending" {
        return error_response(StatusCode::BAD_REQUEST, "invitation is not pending");
    }
    let declined = match invitation::decline_invitation(&state.pool, invitation.id).await {
        Ok(Some(declined)) => declined,
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to decline invitation",
            )
        }
    };
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::events::EVENT_INVITATION_DECLINED.into(),
        workspace_id: declined.workspace_id.to_string(),
        actor_type: "member".into(),
        actor_id: user_id.to_string(),
        payload: serde_json::json!({
            "invitation_id": declined.id.to_string(),
            "invitee_email": declined.invitee_email,
        }),
        ..Default::default()
    });
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_accepts_case_folded_email_or_bound_user() {
        let invitation = WorkspaceInvitation {
            id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            inviter_id: Uuid::nil(),
            invitee_email: "alex@example.com".into(),
            invitee_user_id: None,
            role: "member".into(),
            status: "pending".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now(),
        };
        assert!(belongs_to_user(
            &invitation,
            Uuid::new_v4(),
            "Alex@Example.com"
        ));
        assert!(!belongs_to_user(
            &invitation,
            Uuid::new_v4(),
            "other@example.com"
        ));
    }

    #[test]
    fn base_response_omits_list_only_enrichment() {
        let response = InvitationResponse::from(WorkspaceInvitation {
            id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            inviter_id: Uuid::nil(),
            invitee_email: "alex@example.com".into(),
            invitee_user_id: None,
            role: "member".into(),
            status: "pending".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now(),
        });
        let value = serde_json::to_value(response).unwrap();
        assert!(value.get("inviter_name").is_none());
        assert_eq!(value["invitee_user_id"], serde_json::Value::Null);
    }

    #[test]
    fn acceptance_metrics_include_first_invite_onboarding_completion() {
        let user_id = Uuid::new_v4();
        let workspace_id = Uuid::new_v4();
        let now = chrono::Utc::now();
        let invitation = WorkspaceInvitation {
            id: Uuid::new_v4(),
            workspace_id,
            inviter_id: Uuid::new_v4(),
            invitee_email: "alex@example.com".into(),
            invitee_user_id: Some(user_id),
            role: "member".into(),
            status: "accepted".into(),
            created_at: now - chrono::Duration::days(2),
            updated_at: now,
            expires_at: now + chrono::Duration::days(1),
        };
        let onboarded_user = cordy_db::models::User {
            id: user_id,
            name: "Alex".into(),
            email: "alex@example.com".into(),
            avatar_url: None,
            created_at: now,
            updated_at: now,
            onboarded_at: Some(now),
            onboarding_questionnaire: serde_json::json!({}),
            cloud_waitlist_email: None,
            cloud_waitlist_reason: None,
            starter_content_state: None,
            language: None,
            profile_description: String::new(),
            timezone: None,
        };
        let events =
            acceptance_metric_events(user_id, workspace_id, &invitation, true, &onboarded_user);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].name, cordy_analytics::EVENT_TEAM_INVITE_ACCEPTED);
        assert_eq!(events[1].name, cordy_analytics::EVENT_ONBOARDING_COMPLETED);
    }

    #[test]
    fn workspace_list_response_includes_inviter_enrichment() {
        let response = InvitationResponse::from(ListPendingInvitationsByWorkspaceRow {
            id: Some(Uuid::nil()),
            workspace_id: Some(Uuid::nil()),
            inviter_id: Some(Uuid::nil()),
            invitee_email: "invitee@example.com".into(),
            invitee_user_id: None,
            role: "member".into(),
            status: "pending".into(),
            created_at: Some(chrono::Utc::now()),
            updated_at: Some(chrono::Utc::now()),
            expires_at: Some(chrono::Utc::now()),
            inviter_name: "Alex".into(),
            inviter_email: "alex@example.com".into(),
        });
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["inviter_name"], "Alex");
        assert_eq!(value["inviter_email"], "alex@example.com");
        assert!(value.get("workspace_name").is_none());
    }

    #[tokio::test]
    async fn invitation_admission_consumes_all_three_budgets_atomically() {
        let admission = InvitationAdmission::default();
        let actor_id = Uuid::new_v4();
        let workspace_id = Uuid::new_v4();
        for _ in 0..6 {
            assert!(admission
                .admit(None, actor_id, workspace_id, " recipient@example.com ")
                .await
                .is_ok());
        }
        assert!(admission
            .admit(None, actor_id, workspace_id, "RECIPIENT@example.com")
            .await
            .is_err());
        assert!(admission
            .admit(None, actor_id, workspace_id, "another@example.com")
            .await
            .is_ok());
    }
}
