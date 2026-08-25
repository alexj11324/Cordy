from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text()
    count = text.count(old)
    assert count == 1, f"{path}: expected one anchor, found {count}: {old[:80]!r}"
    p.write_text(text.replace(old, new, 1))


# Autopilot: reject wrong JSON types instead of silently clearing nullable prompt fields.
replace_once(
    "server-rs/crates/cordy-handler/src/autopilot.rs",
    '''    let description = raw
        .get("description")
        .map(|v| v.as_str().map(str::to_owned));
    let template = raw
        .get("issue_title_template")
        .map(|v| v.as_str().map(str::to_owned));
''',
    '''    let nullable_string = |name: &str| -> Result<Option<Option<String>>, Response> {
        match raw.get(name) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(None)),
            Some(Value::String(value)) => Ok(Some(Some(value.clone()))),
            Some(_) => Err(error_response(
                StatusCode::BAD_REQUEST,
                &format!("{name} must be a string or null"),
            )),
        }
    };
    let description = match nullable_string("description") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let template = match nullable_string("issue_title_template") {
        Ok(value) => value,
        Err(response) => return response,
    };
''',
)
replace_once(
    "server-rs/crates/cordy-handler/src/autopilot.rs",
    '''    if (type_sent || id_sent || (active && previous.status != "active"))
        && validate_assignee(
            &state,
            &context,
            &mut tx,
            &next_type,
            next_id,
            previous.workspace_id,
            active,
        )
        .await
        .is_err()
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "assignee is not ready for autopilot execution",
        );
    }
''',
    '''    if type_sent || id_sent || (active && previous.status != "active") {
        if let Err(response) = validate_assignee(
            &state,
            &context,
            &mut tx,
            &next_type,
            next_id,
            previous.workspace_id,
            active,
        )
        .await
        {
            return response;
        }
    }
''',
)

# Composio: the canonical overlay gates on agent owner, not human originator.
replace_once(
    "server-rs/crates/cordy-service/src/task_service.rs",
    '''        let originator_user_id = attr.user_id;
        let runtime_mcp_overlay = match originator_user_id {
            Some(originator) if build_overlay => {
                self.build_runtime_mcp_overlay(originator, &agent).await
            }
            _ => RuntimeMcpOverlayData::default(),
        };
''',
    '''        let originator_user_id = attr.user_id;
        let runtime_mcp_overlay = if build_overlay {
            self.build_runtime_mcp_overlay(originator_user_id.unwrap_or_else(Uuid::nil), &agent)
                .await
        } else {
            RuntimeMcpOverlayData::default()
        };
''',
)

# Chat: never expose hidden system carrier sessions to privileged chat lists.
replace_once(
    "server-rs/crates/cordy-handler/src/chat_api.rs",
    '''    if matches!(role.as_str(), "owner" | "admin") {
        return Ok(agent_ids.into_iter().collect());
    }
''',
    '''    if matches!(role.as_str(), "owner" | "admin") {
        let rows = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM agent WHERE workspace_id=$1 AND id = ANY($2::uuid[]) AND kind='user'",
        )
        .bind(workspace_id)
        .bind(agent_ids)
        .fetch_all(&state.pool)
        .await?;
        return Ok(rows.into_iter().collect());
    }
''',
)

# Chat: Mika onboarding must fail before any rows are written when the agent is unusable.
replace_once(
    "server-rs/crates/cordy-handler/src/chat_api.rs",
    '''    if target.system_key.as_deref() != Some("mika") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "onboarding can only be started with the workspace's built-in agent",
        );
    }
    let (_, user_id) = match ids(&context, &headers) {
''',
    '''    if target.system_key.as_deref() != Some("mika") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "onboarding can only be started with the workspace's built-in agent",
        );
    }
    match cordy_service::agent_ready::agent_readiness(&state.pool, &target).await {
        Ok(verdict) if verdict.blocked() => {
            return dispatch_blocked(StatusCode::CONFLICT, verdict.reason);
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, agent_id = %target.id, "Mika readiness check failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to verify the onboarding agent",
            );
        }
    }
    let (_, user_id) = match ids(&context, &headers) {
''',
)

# Chat: publish the authoritative queue ordering after "send now" commits.
replace_once(
    "server-rs/crates/cordy-handler/src/chat_api.rs",
    '''        Ok(Some(row)) => {
            if let Err(error) = tx.commit().await {
                return internal("failed to commit queued task priority")(error.into());
            }
            Json(json!({"task_id": row.task_id, "active_task_id": row.active_task_id}))
                .into_response()
        }
''',
    '''        Ok(Some(row)) => {
            if let Err(error) = tx.commit().await {
                return internal("failed to commit queued task priority")(error.into());
            }
            if let Ok(Some(task)) = agent::get_agent_task(&state.pool, row.task_id).await {
                state.tasks.broadcast_task_queued(&task).await;
            }
            Json(json!({"task_id": row.task_id, "active_task_id": row.active_task_id}))
                .into_response()
        }
''',
)

# LLM: consume the already-loaded TOML+environment config instead of rereading env only.
replace_once(
    "server-rs/crates/cordy-handler/src/state.rs",
    '''    /// Wires the internal OpenAI-compatible assist layer. Invalid retry
    /// budgets fail startup rather than silently selecting another policy.
    pub fn with_llm_from_env(self) -> anyhow::Result<Self> {
        const MAX_RETRIES: u32 = 5;
        let raw_retries = std::env::var("CORDY_LLM_MAX_RETRIES").unwrap_or_default();
        let max_retries = if raw_retries.trim().is_empty() {
            None
        } else {
            let parsed = raw_retries.trim().parse::<u32>().map_err(|_| {
                anyhow::anyhow!(
                    "CORDY_LLM_MAX_RETRIES must be an integer from 0 to {MAX_RETRIES}, got {:?}",
                    raw_retries.trim()
                )
            })?;
            anyhow::ensure!(
                parsed <= MAX_RETRIES,
                "CORDY_LLM_MAX_RETRIES must be at most {MAX_RETRIES}, got {parsed}"
            );
            Some(parsed)
        };
        let client = Arc::new(cordy_llm::Client::new(cordy_llm::Config {
            api_key: std::env::var("CORDY_LLM_API_KEY").unwrap_or_default(),
            base_url: std::env::var("CORDY_LLM_BASE_URL").unwrap_or_default(),
            default_model: std::env::var("CORDY_LLM_DEFAULT_MODEL").unwrap_or_default(),
            max_retries,
        }));
        self.llm.replace(client.clone());
        tracing::info!(
            enabled = client.enabled(),
            max_retries = client.max_retries(),
            default_model = client.default_model(),
            "llm assist policy"
        );
        Ok(self)
    }
''',
    '''    /// Wires the internal OpenAI-compatible assist layer from the loaded
    /// configuration, which already includes environment overrides.
    pub fn with_llm_from_config(
        self,
        config: &cordy_config::LlmConfig,
    ) -> anyhow::Result<Self> {
        const MAX_RETRIES: u32 = 5;
        if let Some(max_retries) = config.max_retries {
            anyhow::ensure!(
                max_retries <= MAX_RETRIES,
                "CORDY_LLM_MAX_RETRIES must be at most {MAX_RETRIES}, got {max_retries}"
            );
        }
        let client = Arc::new(cordy_llm::Client::new(cordy_llm::Config {
            api_key: config.api_key.clone().unwrap_or_default(),
            base_url: config.base_url.clone().unwrap_or_default(),
            default_model: config.default_model.clone().unwrap_or_default(),
            max_retries: config.max_retries,
        }));
        self.llm.replace(client.clone());
        tracing::info!(
            enabled = client.enabled(),
            max_retries = client.max_retries(),
            default_model = client.default_model(),
            "llm assist policy"
        );
        Ok(self)
    }
''',
)
replace_once(
    "server-rs/crates/cordy-server/src/main.rs",
    '''    .with_llm_from_env()?
''',
    '''    .with_llm_from_config(&cfg.llm)?
''',
)

# Auth: normalize signup flag, share cookie policy, and resolve private avatar URLs.
replace_once(
    "server-rs/crates/cordy-handler/src/auth.rs",
    '''            allow_signup: config.auth.allow_signup.as_deref() != Some("false"),
''',
    '''            allow_signup: config
                .auth
                .allow_signup
                .as_deref()
                .map(str::trim)
                != Some("false"),
''',
)
replace_once(
    "server-rs/crates/cordy-handler/src/auth.rs",
    '''    fn is_dev_code(&self, code: &str) -> bool {
        !self.app_env.eq_ignore_ascii_case("production")
            && is_six_digit_code(&self.dev_verification_code)
            && constant_time_eq(code.as_bytes(), self.dev_verification_code.as_bytes())
    }
}
''',
    '''    fn is_dev_code(&self, code: &str) -> bool {
        !self.app_env.eq_ignore_ascii_case("production")
            && is_six_digit_code(&self.dev_verification_code)
            && constant_time_eq(code.as_bytes(), self.dev_verification_code.as_bytes())
    }

    pub(crate) fn cookie_attributes(&self) -> (Option<String>, bool) {
        (
            cordy_auth::cookie::cookie_domain(Some(&self.cookie_domain)),
            cordy_auth::cookie::is_secure_cookie(Some(&self.frontend_origin)),
        )
    }
}
''',
)
replace_once(
    "server-rs/crates/cordy-handler/src/auth.rs",
    '''impl From<&User> for UserResponse {
    fn from(value: &User) -> Self {
        Self {
            id: value.id.to_string(),
            name: value.name.clone(),
            email: value.email.clone(),
            avatar_url: value.avatar_url.clone(),
''',
    '''impl UserResponse {
    fn from_user(state: &HandlerState, value: &User) -> Self {
        Self {
            id: value.id.to_string(),
            name: value.name.clone(),
            email: value.email.clone(),
            avatar_url: value
                .avatar_url
                .as_deref()
                .map(|url| crate::avatar::resolve_url(state, url)),
''',
)
replace_once(
    "server-rs/crates/cordy-handler/src/auth.rs",
    '''    let domain = cordy_auth::cookie::cookie_domain(Some(&state.auth_settings.cookie_domain));
    let secure = cordy_auth::cookie::is_secure_cookie(Some(&state.auth_settings.frontend_origin));
''',
    '''    let (domain, secure) = state.auth_settings.cookie_attributes();
''',
)
p = Path("server-rs/crates/cordy-handler/src/auth.rs")
text = p.read_text()
count = text.count("UserResponse::from(&current)")
assert count == 2, f"auth.rs: expected 2 login projections, found {count}"
p.write_text(text.replace("UserResponse::from(&current)", "UserResponse::from_user(state, &current)"))

# Auth: serialize per-email cooldown check + code insert under a transaction advisory lock.
replace_once(
    "server-rs/crates/cordy-handler/src/auth.rs",
    '''    if let Ok(Some(latest)) = verification_code::get_latest_code_by_email(&state.pool, &email).await
    {
        if Utc::now().signed_duration_since(latest.created_at) < Duration::seconds(60) {
            return error_response(
                StatusCode::TOO_MANY_REQUESTS,
                "please wait before requesting another code",
            );
        }
    }

    let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000));
    match verification_code::create_verification_code(
        &state.pool,
        &email,
        &code,
        Some(Utc::now() + Duration::minutes(10)),
    )
    .await
    {
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to store verification code",
            );
        }
    }
''',
    '''    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, %email, "auth: failed to start send-code transaction");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to store verification code",
            );
        }
    };
    if let Err(error) = sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("auth-send-code:{email}"))
        .execute(&mut *tx)
        .await
    {
        tracing::error!(%error, %email, "auth: failed to lock send-code cooldown");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to store verification code",
        );
    }
    match verification_code::get_latest_code_by_email(&mut *tx, &email).await {
        Ok(Some(latest))
            if Utc::now().signed_duration_since(latest.created_at) < Duration::seconds(60) =>
        {
            return error_response(
                StatusCode::TOO_MANY_REQUESTS,
                "please wait before requesting another code",
            );
        }
        Ok(_) => {}
        Err(error) => {
            tracing::error!(%error, %email, "auth: failed to inspect send-code cooldown");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to store verification code",
            );
        }
    }

    let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000));
    match verification_code::create_verification_code(
        &mut *tx,
        &email,
        &code,
        Some(Utc::now() + Duration::minutes(10)),
    )
    .await
    {
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to store verification code",
            );
        }
    }
    if let Err(error) = tx.commit().await {
        tracing::error!(%error, %email, "auth: failed to commit send-code cooldown");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to store verification code",
        );
    }
''',
)

# Auth: reserve failed attempts atomically so concurrent guesses cannot exceed the budget.
replace_once(
    "server-rs/crates/cordy-db/src/queries/verification_code.rs",
    '''pub async fn increment_verification_code_attempts(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE verification_code
SET attempts = attempts + 1
WHERE id = $1"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}
''',
    '''pub async fn increment_verification_code_attempts(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE verification_code
SET attempts = attempts + 1
WHERE id = $1"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn reserve_verification_code_attempt(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<bool> {
    let r = sqlx::query(
        r#"UPDATE verification_code
SET attempts = attempts + 1
WHERE id = $1
  AND used = FALSE
  AND expires_at > now()
  AND attempts < 5"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected() == 1)
}
''',
)
replace_once(
    "server-rs/crates/cordy-handler/src/auth.rs",
    '''        let _ =
            verification_code::increment_verification_code_attempts(&state.pool, db_code.id).await;
        return error_response(StatusCode::BAD_REQUEST, "invalid or expired code");
''',
    '''        match verification_code::reserve_verification_code_attempt(&state.pool, db_code.id).await {
            Ok(true) | Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, "auth: failed to reserve verification attempt");
            }
        }
        return error_response(StatusCode::BAD_REQUEST, "invalid or expired code");
''',
)

# Logout must clear the same TOML-resolved cookie scope used at login.
replace_once(
    "server-rs/crates/cordy-handler/src/session.rs",
    '''async fn logout(State(state): State<HandlerState>) -> Response {
    let domain_raw = std::env::var("COOKIE_DOMAIN").ok();
    let domain = cordy_auth::cookie::cookie_domain(domain_raw.as_deref());
    let frontend_origin = std::env::var("FRONTEND_ORIGIN").ok();
    let secure = cordy_auth::cookie::is_secure_cookie(frontend_origin.as_deref());
    let mut headers = HeaderMap::new();
''',
    '''async fn logout(State(state): State<HandlerState>) -> Response {
    let (domain, secure) = state.auth_settings.cookie_attributes();
    let mut headers = HeaderMap::new();
''',
)

# Invitation: human-only current-user routes, configurable limits, bounded memory keys.
replace_once(
    "server-rs/crates/cordy-handler/src/invitation.rs",
    '''use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
''',
    '''use axum::extract::{Extension, Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
''',
)
replace_once(
    "server-rs/crates/cordy-handler/src/invitation.rs",
    '''impl InvitationAdmission {
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
''',
    '''impl InvitationAdmission {
    fn configured_limit(name: &str, default: usize) -> usize {
        std::env::var(name)
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(default)
    }

    fn gates(actor_id: Uuid, workspace_id: Uuid, email: &str) -> Vec<AdmissionGate> {
        let recipient = hex::encode(Sha256::digest(email.trim().to_ascii_lowercase().as_bytes()));
        [
            (
                "actor",
                format!("mul:invitation:actor:{actor_id}"),
                Self::configured_limit("RATE_LIMIT_INVITATION_ACTOR_10M", 10),
                Duration::from_secs(600),
            ),
            (
                "workspace",
                format!("mul:invitation:workspace:{workspace_id}"),
                Self::configured_limit("RATE_LIMIT_INVITATION_WORKSPACE_24H", 50),
                Duration::from_secs(86_400),
            ),
            (
                "recipient",
                format!("mul:invitation:recipient:{recipient}"),
                Self::configured_limit("RATE_LIMIT_INVITATION_RECIPIENT_24H", 6),
                Duration::from_secs(86_400),
            ),
        ]
        .into_iter()
        .filter(|(_, _, limit, _)| *limit > 0)
        .map(|(name, key, limit, window)| AdmissionGate {
            name,
            key,
            limit,
            window,
        })
        .collect()
    }
''',
)
replace_once(
    "server-rs/crates/cordy-handler/src/invitation.rs",
    '''        let gates = Self::gates(actor_id, workspace_id, email);
        if let Some(client) = redis {
''',
    '''        let gates = Self::gates(actor_id, workspace_id, email);
        if gates.is_empty() {
            return Ok(());
        }
        if let Some(client) = redis {
''',
)
replace_once(
    "server-rs/crates/cordy-handler/src/invitation.rs",
    '''        for gate in gates {
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
''',
    '''        for gate in gates {
            if let Some(values) = entries.get_mut(&gate.key) {
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
        }
        entries.retain(|_, values| !values.is_empty());
        if retry_after > 0 {
''',
)
replace_once(
    "server-rs/crates/cordy-handler/src/invitation.rs",
    '''pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/invitations", get(list))
        .route("/api/invitations/{id}", get(get_one))
        .route("/api/invitations/{id}/accept", axum::routing::post(accept))
        .route(
            "/api/invitations/{id}/decline",
            axum::routing::post(decline),
        )
}
''',
    '''pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/invitations", get(list))
        .route("/api/invitations/{id}", get(get_one))
        .route("/api/invitations/{id}/accept", axum::routing::post(accept))
        .route(
            "/api/invitations/{id}/decline",
            axum::routing::post(decline),
        )
        .route_layer(axum::middleware::from_fn(require_human_actor))
}

async fn require_human_actor(request: Request, next: Next) -> Response {
    if matches!(
        request
            .headers()
            .get("x-actor-source")
            .and_then(|value| value.to_str().ok()),
        Some("task_token" | "cloud_pat")
    ) {
        return error_response(
            StatusCode::FORBIDDEN,
            "invitation decisions are only available to human actors",
        );
    }
    next.run(request).await
}
''',
)

# Workspace: reject malformed scp hosts.
replace_once(
    "server-rs/crates/cordy-handler/src/workspace.rs",
    '''    if colon == 0 || colon + 1 == value.len() {
        return false;
    }
    !value.find('@').is_some_and(|at| at >= colon)
''',
    '''    if colon == 0 || colon + 1 == value.len() {
        return false;
    }
    let host_start = value.find('@').map_or(0, |at| at.saturating_add(1));
    if host_start >= colon || value[host_start..colon].is_empty() {
        return false;
    }
    !value.find('@').is_some_and(|at| at >= colon)
''',
)

# Workspace: redact runtime gateway tokens from workspace-wide archive events.
replace_once(
    "server-rs/crates/cordy-handler/src/workspace.rs",
    '''            object.remove("mcp_config");
            object.remove("composio_toolkit_allowlist");
            object.insert("has_custom_env".into(), serde_json::json!(env_count > 0));
''',
    '''            object.remove("mcp_config");
            object.remove("composio_toolkit_allowlist");
            if let Some(token) = object
                .get_mut("runtime_config")
                .and_then(serde_json::Value::as_object_mut)
                .and_then(|config| config.get_mut("gateway"))
                .and_then(serde_json::Value::as_object_mut)
                .and_then(|gateway| gateway.get_mut("token"))
            {
                if token.as_str().is_some_and(|value| !value.is_empty()) {
                    *token = serde_json::Value::String("***".into());
                }
            }
            object.insert("has_custom_env".into(), serde_json::json!(env_count > 0));
''',
)

# Workspace: only the transaction that actually acquired the workspace row may emit deletion side effects.
replace_once(
    "server-rs/crates/cordy-handler/src/workspace.rs",
    '''    step!(
        "lock workspace",
        workspace::lock_workspace_for_delete(&mut *tx, workspace_id)
    );
''',
    '''    match workspace::lock_workspace_for_delete(&mut *tx, workspace_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "workspace not found"),
        Err(error) => {
            tracing::warn!(%error, workspace_id = %workspace_id, step = "lock workspace", "workspace delete failed");
            return workspace_delete_error(retryable_lock_error(&error));
        }
    }
''',
)

# GitHub capability must reflect a usable client, not merely non-empty env.
replace_once(
    "server-rs/crates/cordy-handler/src/github.rs",
    '''fn browse_configured() -> bool {
    !env("GITHUB_APP_ID").is_empty() && !env("GITHUB_APP_PRIVATE_KEY").is_empty()
}
''',
    '''fn browse_configured() -> bool {
    matches!(cordy_ghsnapshot::Client::new_from_env(), Ok(Some(_)))
}
''',
)

print("first remediation batch applied")
