//! Unified Phase 1 authorization boundary.
//!
//! The public [`Authorizer`] trait is deliberately independent from handler
//! and daemon code so a future OpenFGA/SpiceDB adapter can replace the
//! PostgreSQL evaluator without changing enforcement consumers.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub const POLICY_VERSION: &str = "phase1-2026-08-29";
pub const MAX_DELEGATION_DEPTH: i32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalType {
    User,
    Team,
    AgentDefinition,
    TaskRun,
    DeviceRuntime,
    Service,
    System,
}

impl PrincipalType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Team => "team",
            Self::AgentDefinition => "agent_definition",
            Self::TaskRun => "task_run",
            Self::DeviceRuntime => "device_runtime",
            Self::Service => "service",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRole {
    Owner,
    Admin,
    Member,
    /// Reserved deny-by-default boundary. Phase 1 does not persist Guest.
    Guest,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Action(pub String);

impl Action {
    pub const AGENT_INVOKE: &'static str = "agent.invoke";
    pub const CREDENTIAL_USE: &'static str = "credential.use";
    pub const CREDENTIAL_READ_SECRET: &'static str = "credential.read_secret";
    pub const RUNTIME_READ: &'static str = "runtime.read";
    pub const RUNTIME_UPDATE: &'static str = "runtime.update";
    pub const RUNTIME_USE: &'static str = "runtime.use";
    pub const TASK_READ: &'static str = "task.read";
    pub const TASK_UPDATE: &'static str = "task.update";
    pub const RESOURCE_READ: &'static str = "resource.read";
    pub const RESOURCE_USE: &'static str = "resource.use";
    pub const WORKSPACE_MANAGE: &'static str = "workspace.manage";

    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceType(pub String);

impl ResourceType {
    pub const AGENT_DEFINITION: &'static str = "agent_definition";
    pub const CREDENTIAL: &'static str = "credential";
    pub const RUNTIME: &'static str = "runtime";
    pub const LOCAL_DIRECTORY: &'static str = "local_directory";
    pub const PROJECT_RESOURCE: &'static str = "project_resource";
    pub const TASK_RUN: &'static str = "task_run";
    pub const WORKSPACE: &'static str = "workspace";

    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    pub principal_type: PrincipalType,
    pub id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub resource_type: ResourceType,
    pub id: Option<Uuid>,
    pub workspace_id: Uuid,
    pub owner_id: Option<Uuid>,
    #[serde(default)]
    pub attributes: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthorizationContext {
    pub workspace_role: Option<WorkspaceRole>,
    pub on_behalf_of_user_id: Option<Uuid>,
    pub via_agent_id: Option<Uuid>,
    pub device_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub lease_id: Option<Uuid>,
    #[serde(default)]
    pub team_ids: Vec<Uuid>,
    pub approval_id: Option<Uuid>,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    pub action: String,
    pub resource_type: String,
    /// UUID text, `*`, or `$task`. `$task` binds to the lease's task id.
    pub resource_id: String,
}

impl Capability {
    pub fn exact(action: &str, resource_type: &str, resource_id: Uuid) -> Self {
        Self {
            action: action.to_string(),
            resource_type: resource_type.to_string(),
            resource_id: resource_id.to_string(),
        }
    }

    pub fn wildcard(action: &str, resource_type: &str) -> Self {
        Self {
            action: action.to_string(),
            resource_type: resource_type.to_string(),
            resource_id: "*".to_string(),
        }
    }

    pub fn task(action: &str) -> Self {
        Self {
            action: action.to_string(),
            resource_type: ResourceType::TASK_RUN.to_string(),
            resource_id: "$task".to_string(),
        }
    }

    fn covers(&self, action: &str, resource_type: &str, resource_id: Option<Uuid>, task_id: Uuid) -> bool {
        self.action == action
            && self.resource_type == resource_type
            && match self.resource_id.as_str() {
                "*" => true,
                "$task" => resource_id == Some(task_id),
                exact => resource_id.is_some_and(|id| id.to_string() == exact),
            }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationHop {
    pub lease_id: Uuid,
    pub task_id: Uuid,
    pub principal_id: Option<Uuid>,
    pub depth: i32,
    pub fence: i64,
    pub scope: Vec<Capability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationRequest {
    pub principal: Principal,
    pub action: Action,
    pub resource: Resource,
    #[serde(default)]
    pub context: AuthorizationContext,
    #[serde(default)]
    pub delegation_chain: Vec<DelegationHop>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionEffect {
    Allow,
    Deny,
    RequireApproval,
}

impl DecisionEffect {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::RequireApproval => "require_approval",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Obligation {
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub effect: DecisionEffect,
    pub reason: String,
    #[serde(default)]
    pub matched_grants: Vec<Uuid>,
    pub policy_version: String,
    #[serde(default)]
    pub obligations: Vec<Obligation>,
    pub audit_id: Option<Uuid>,
}

impl Decision {
    pub fn is_allowed(&self) -> bool {
        self.effect == DecisionEffect::Allow
    }

    fn new(effect: DecisionEffect, reason: impl Into<String>) -> Self {
        Self {
            effect,
            reason: reason.into(),
            matched_grants: Vec::new(),
            policy_version: POLICY_VERSION.to_string(),
            obligations: Vec::new(),
            audit_id: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthorizationError {
    #[error("authorization store unavailable: {0}")]
    Store(#[from] sqlx::Error),
    #[error("invalid authorization data: {0}")]
    InvalidData(String),
}

#[async_trait]
pub trait Authorizer: Send + Sync {
    async fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<Decision, AuthorizationError>;

    async fn explain(
        &self,
        audit_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<Option<AuditEvent>, AuthorizationError>;
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditEvent {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub principal_type: String,
    pub principal_id: Option<Uuid>,
    pub on_behalf_of_user_id: Option<Uuid>,
    pub via_agent_id: Option<Uuid>,
    pub device_id: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<Uuid>,
    pub decision: String,
    pub reason: String,
    pub matched_grant_ids: Vec<Uuid>,
    pub policy_version: String,
    pub obligations: Value,
    pub delegation_chain: Value,
    pub context: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct PostgresAuthorizer {
    pool: PgPool,
}

impl PostgresAuthorizer {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn matching_grants(
        &self,
        request: &AuthorizationRequest,
    ) -> Result<Vec<Grant>, sqlx::Error> {
        let rows = sqlx::query(
            r#"SELECT id, effect, conditions
FROM authorization_grant
WHERE workspace_id = $1
  AND revoked_at IS NULL
  AND (expires_at IS NULL OR expires_at > now())
  AND action = $2
  AND resource_type = $3
  AND (resource_id IS NULL OR resource_id = $4)
  AND (
      (principal_type = $5 AND (principal_id IS NULL OR principal_id = $6))
      OR (principal_type = 'team' AND principal_id = ANY($7::uuid[]))
  )
ORDER BY created_at, id"#,
        )
        .bind(request.resource.workspace_id)
        .bind(request.action.as_str())
        .bind(request.resource.resource_type.as_str())
        .bind(request.resource.id)
        .bind(request.principal.principal_type.as_str())
        .bind(request.principal.id)
        .bind(&request.context.team_ids)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(Grant {
                    id: row.try_get("id")?,
                    effect: row.try_get("effect")?,
                    conditions: row.try_get("conditions")?,
                })
            })
            .collect()
    }

    async fn load_lease_chain(
        &self,
        lease_id: Uuid,
    ) -> Result<Vec<LeaseRow>, AuthorizationError> {
        let rows = sqlx::query(
            r#"WITH RECURSIVE lease_chain AS (
    SELECT token.id, token.task_id, token.agent_id, token.workspace_id,
           token.scope, token.parent_token_id, token.parent_fence,
           token.delegation_depth, token.delegation_fence,
           token.claim_dispatched_at, token.on_behalf_of_user_id,
           token.device_id, token.revoked_at, token.expires_at,
           task.status AS task_status, task.dispatched_at AS current_dispatched_at,
           ARRAY[token.id] AS path
    FROM task_token token
    JOIN agent_task_queue task ON task.id = token.task_id
    WHERE token.id = $1
  UNION ALL
    SELECT parent.id, parent.task_id, parent.agent_id, parent.workspace_id,
           parent.scope, parent.parent_token_id, parent.parent_fence,
           parent.delegation_depth, parent.delegation_fence,
           parent.claim_dispatched_at, parent.on_behalf_of_user_id,
           parent.device_id, parent.revoked_at, parent.expires_at,
           task.status, task.dispatched_at,
           child.path || parent.id
    FROM task_token parent
    JOIN lease_chain child ON child.parent_token_id = parent.id
    JOIN agent_task_queue task ON task.id = parent.task_id
    WHERE NOT parent.id = ANY(child.path)
      AND cardinality(child.path) <= 9
)
SELECT * FROM lease_chain ORDER BY delegation_depth DESC"#,
        )
        .bind(lease_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let raw_scope: Value = row.try_get("scope")?;
                let scope = serde_json::from_value(raw_scope).map_err(|error| {
                    AuthorizationError::InvalidData(format!("invalid lease scope: {error}"))
                })?;
                Ok(LeaseRow {
                    id: row.try_get("id")?,
                    task_id: row.try_get("task_id")?,
                    agent_id: row.try_get("agent_id")?,
                    workspace_id: row.try_get("workspace_id")?,
                    scope,
                    parent_token_id: row.try_get("parent_token_id")?,
                    parent_fence: row.try_get("parent_fence")?,
                    depth: row.try_get("delegation_depth")?,
                    fence: row.try_get("delegation_fence")?,
                    claim_dispatched_at: row.try_get("claim_dispatched_at")?,
                    on_behalf_of_user_id: row.try_get("on_behalf_of_user_id")?,
                    device_id: row.try_get("device_id")?,
                    revoked_at: row.try_get("revoked_at")?,
                    expires_at: row.try_get("expires_at")?,
                    task_status: row.try_get("task_status")?,
                    current_dispatched_at: row.try_get("current_dispatched_at")?,
                })
            })
            .collect()
    }

    async fn record_audit(
        &self,
        request: &AuthorizationRequest,
        decision: &Decision,
    ) -> Result<Uuid, sqlx::Error> {
        let id = Uuid::now_v7();
        let obligations = serde_json::to_value(&decision.obligations).unwrap_or_else(|_| json!([]));
        let delegation = serde_json::to_value(&request.delegation_chain).unwrap_or_else(|_| json!([]));
        // Keep the audit context identifier-only. Resource attributes may carry
        // integration metadata and are intentionally not persisted here.
        let context = json!({
            "workspace_role": request.context.workspace_role,
            "task_id": request.context.task_id,
            "lease_id": request.context.lease_id,
            "team_ids": request.context.team_ids,
            "approval_id": request.context.approval_id,
            "request_id": request.context.request_id,
        });
        sqlx::query(
            r#"INSERT INTO authorization_audit_event (
    id, workspace_id, principal_type, principal_id,
    on_behalf_of_user_id, via_agent_id, device_id,
    action, resource_type, resource_id, decision, reason,
    matched_grant_ids, policy_version, obligations, delegation_chain, context
) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)"#,
        )
        .bind(id)
        .bind(request.resource.workspace_id)
        .bind(request.principal.principal_type.as_str())
        .bind(request.principal.id)
        .bind(request.context.on_behalf_of_user_id)
        .bind(request.context.via_agent_id)
        .bind(request.context.device_id)
        .bind(request.action.as_str())
        .bind(request.resource.resource_type.as_str())
        .bind(request.resource.id)
        .bind(decision.effect.as_str())
        .bind(&decision.reason)
        .bind(&decision.matched_grants)
        .bind(&decision.policy_version)
        .bind(obligations)
        .bind(delegation)
        .bind(context)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn create_grant(&self, input: CreateGrant) -> Result<Uuid, AuthorizationError> {
        let id = Uuid::now_v7();
        sqlx::query(
            r#"INSERT INTO authorization_grant (
    id, workspace_id, principal_type, principal_id, action,
    resource_type, resource_id, effect, conditions, expires_at, created_by
) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)"#,
        )
        .bind(id)
        .bind(input.workspace_id)
        .bind(input.principal_type.as_str())
        .bind(input.principal_id)
        .bind(input.action)
        .bind(input.resource_type)
        .bind(input.resource_id)
        .bind(input.effect.as_str())
        .bind(input.conditions)
        .bind(input.expires_at)
        .bind(input.created_by)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn revoke_grant(
        &self,
        grant_id: Uuid,
        workspace_id: Uuid,
        actor_id: Uuid,
    ) -> Result<bool, AuthorizationError> {
        let result = sqlx::query(
            r#"UPDATE authorization_grant
SET revoked_at = now(), revoked_by = $3, updated_at = now()
WHERE id = $1 AND workspace_id = $2 AND revoked_at IS NULL"#,
        )
        .bind(grant_id)
        .bind(workspace_id)
        .bind(actor_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn revoke_lease(
        &self,
        lease_id: Uuid,
        reason: &str,
    ) -> Result<bool, AuthorizationError> {
        let result = sqlx::query(
            r#"UPDATE task_token
SET revoked_at = now(), revoked_reason = $2
WHERE id = $1 AND revoked_at IS NULL"#,
        )
        .bind(lease_id)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}

#[async_trait]
impl Authorizer for PostgresAuthorizer {
    async fn authorize(
        &self,
        mut request: AuthorizationRequest,
    ) -> Result<Decision, AuthorizationError> {
        let grants = self.matching_grants(&request).await?;
        let lease_chain = match request.context.lease_id {
            Some(lease_id) => self.load_lease_chain(lease_id).await?,
            None => Vec::new(),
        };
        if !lease_chain.is_empty() {
            request.delegation_chain = lease_chain.iter().map(DelegationHop::from).collect();
        }
        let mut decision = evaluate(&request, &grants, &lease_chain);
        decision.audit_id = Some(self.record_audit(&request, &decision).await?);
        Ok(decision)
    }

    async fn explain(
        &self,
        audit_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<Option<AuditEvent>, AuthorizationError> {
        let row = sqlx::query(
            r#"SELECT id, workspace_id, principal_type, principal_id,
       on_behalf_of_user_id, via_agent_id, device_id,
       action, resource_type, resource_id, decision, reason,
       matched_grant_ids, policy_version, obligations,
       delegation_chain, context, created_at
FROM authorization_audit_event
WHERE id = $1 AND workspace_id = $2"#,
        )
        .bind(audit_id)
        .bind(workspace_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(AuditEvent {
                id: row.try_get("id")?,
                workspace_id: row.try_get("workspace_id")?,
                principal_type: row.try_get("principal_type")?,
                principal_id: row.try_get("principal_id")?,
                on_behalf_of_user_id: row.try_get("on_behalf_of_user_id")?,
                via_agent_id: row.try_get("via_agent_id")?,
                device_id: row.try_get("device_id")?,
                action: row.try_get("action")?,
                resource_type: row.try_get("resource_type")?,
                resource_id: row.try_get("resource_id")?,
                decision: row.try_get("decision")?,
                reason: row.try_get("reason")?,
                matched_grant_ids: row.try_get("matched_grant_ids")?,
                policy_version: row.try_get("policy_version")?,
                obligations: row.try_get("obligations")?,
                delegation_chain: row.try_get("delegation_chain")?,
                context: row.try_get("context")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .transpose()
        .map_err(AuthorizationError::from)
    }
}

#[derive(Debug, Clone)]
struct Grant {
    id: Uuid,
    effect: String,
    conditions: Value,
}

#[derive(Debug, Clone)]
struct LeaseRow {
    id: Uuid,
    task_id: Uuid,
    agent_id: Uuid,
    workspace_id: Uuid,
    scope: Vec<Capability>,
    parent_token_id: Option<Uuid>,
    parent_fence: Option<i64>,
    depth: i32,
    fence: i64,
    claim_dispatched_at: Option<DateTime<Utc>>,
    on_behalf_of_user_id: Option<Uuid>,
    device_id: Option<Uuid>,
    revoked_at: Option<DateTime<Utc>>,
    expires_at: DateTime<Utc>,
    task_status: String,
    current_dispatched_at: Option<DateTime<Utc>>,
}

impl From<&LeaseRow> for DelegationHop {
    fn from(value: &LeaseRow) -> Self {
        Self {
            lease_id: value.id,
            task_id: value.task_id,
            principal_id: Some(value.agent_id),
            depth: value.depth,
            fence: value.fence,
            scope: value.scope.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateGrant {
    pub workspace_id: Uuid,
    pub principal_type: PrincipalType,
    pub principal_id: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<Uuid>,
    pub effect: DecisionEffect,
    pub conditions: Value,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_by: Option<Uuid>,
}

fn evaluate(
    request: &AuthorizationRequest,
    grants: &[Grant],
    lease_chain: &[LeaseRow],
) -> Decision {
    if request.resource.workspace_id.is_nil() {
        return Decision::new(DecisionEffect::Deny, "resource workspace is required");
    }
    if request.delegation_chain.len() > (MAX_DELEGATION_DEPTH as usize + 1) {
        return Decision::new(DecisionEffect::Deny, "delegation depth exceeded");
    }

    // Non-delegable guardrail: agents and runs can use a brokered credential,
    // but can never read its long-lived secret.
    if request.action.as_str() == Action::CREDENTIAL_READ_SECRET
        && matches!(
            request.principal.principal_type,
            PrincipalType::AgentDefinition | PrincipalType::TaskRun
        )
    {
        return Decision::new(
            DecisionEffect::Deny,
            "agent and task principals cannot read long-lived credential secrets",
        );
    }

    let lease_authorized = if request.principal.principal_type == PrincipalType::TaskRun
        || request.context.lease_id.is_some()
    {
        match validate_lease(request, lease_chain) {
            Ok(()) => true,
            Err(reason) => return Decision::new(DecisionEffect::Deny, reason),
        }
    } else {
        false
    };

    let matched: Vec<&Grant> = grants
        .iter()
        .filter(|grant| grant_conditions_match(&grant.conditions, request))
        .collect();
    let mut matched_ids: Vec<Uuid> = matched.iter().map(|grant| grant.id).collect();
    matched_ids.sort_unstable();

    if matched.iter().any(|grant| grant.effect == "deny") {
        let mut decision = Decision::new(DecisionEffect::Deny, "matched explicit deny grant");
        decision.matched_grants = matched_ids;
        return decision;
    }

    let requires_approval = matched
        .iter()
        .any(|grant| grant.effect == "require_approval")
        || request
            .resource
            .attributes
            .get("require_approval")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    // Phase 1 has no approval-record verifier. Presence of an arbitrary UUID
    // is not proof of approval, so this remains fail-closed until a verified
    // approval adapter can turn the obligation into a distinct allow grant.
    if requires_approval {
        let mut decision = Decision::new(
            DecisionEffect::RequireApproval,
            "approval is required before this action",
        );
        decision.matched_grants = matched_ids;
        decision.obligations.push(Obligation {
            kind: "obtain_approval".to_string(),
            detail: "obtain a verified approval grant and re-authorize".to_string(),
        });
        return decision;
    }

    let explicit_allow = matched.iter().any(|grant| grant.effect == "allow");
    let relationship_allow = relationship_allows(request);

    // A valid lease is necessary but not sufficient at sensitive resource
    // boundaries. Invocation still needs the target ACL; credential use still
    // needs the on-behalf owner relationship.
    let allowed = if lease_authorized {
        match request.action.as_str() {
            Action::AGENT_INVOKE | Action::CREDENTIAL_USE => relationship_allow,
            _ => relationship_allow || explicit_allow || lease_authorized,
        }
    } else {
        relationship_allow || explicit_allow
    };
    let mut decision = if allowed {
        Decision::new(
            DecisionEffect::Allow,
            if lease_authorized {
                "active capability lease and resource boundary allow action"
            } else if relationship_allow {
                "resource relationship and attributes allow action"
            } else {
                "matched allow grant"
            },
        )
    } else {
        Decision::new(
            DecisionEffect::Deny,
            "no resource relationship, lease, or grant allows action",
        )
    };
    decision.matched_grants = matched_ids;
    decision
}

fn relationship_allows(request: &AuthorizationRequest) -> bool {
    let effective_user = match request.principal.principal_type {
        PrincipalType::User => request.principal.id,
        PrincipalType::TaskRun | PrincipalType::AgentDefinition => {
            request.context.on_behalf_of_user_id
        }
        _ => None,
    };
    let is_owner = effective_user.is_some() && effective_user == request.resource.owner_id;
    let is_private = request
        .resource
        .attributes
        .get("private")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    match request.action.as_str() {
        Action::AGENT_INVOKE => is_owner
            || request
                .resource
                .attributes
                .get("invocation_allowed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        Action::CREDENTIAL_USE | Action::CREDENTIAL_READ_SECRET => is_owner,
        Action::RUNTIME_READ => {
            is_owner
                || (!is_private
                    && !matches!(request.context.workspace_role, Some(WorkspaceRole::Guest)))
        }
        Action::RUNTIME_UPDATE => is_owner,
        Action::WORKSPACE_MANAGE => matches!(
            request.context.workspace_role,
            Some(WorkspaceRole::Owner | WorkspaceRole::Admin)
        ),
        _ => is_owner,
    }
}

fn grant_conditions_match(conditions: &Value, request: &AuthorizationRequest) -> bool {
    let Some(map) = conditions.as_object() else {
        return false;
    };
    for (key, value) in map {
        let matches = match key.as_str() {
            "workspace_role" => serde_json::to_value(request.context.workspace_role)
                .ok()
                .as_ref()
                == Some(value),
            "device_id" => request
                .context
                .device_id
                .is_some_and(|id| value.as_str() == Some(id.to_string().as_str())),
            "on_behalf_of_user_id" => request
                .context
                .on_behalf_of_user_id
                .is_some_and(|id| value.as_str() == Some(id.to_string().as_str())),
            // Unknown conditions never match. This is additive and fail-closed.
            _ => false,
        };
        if !matches {
            return false;
        }
    }
    true
}

fn validate_lease(
    request: &AuthorizationRequest,
    chain: &[LeaseRow],
) -> Result<(), &'static str> {
    let Some(lease_id) = request.context.lease_id else {
        return Err("task/run principal requires a capability lease");
    };
    let Some(leaf) = chain.first() else {
        return Err("capability lease not found");
    };
    if leaf.id != lease_id {
        return Err("capability lease chain does not start at requested lease");
    }
    if leaf.workspace_id != request.resource.workspace_id {
        return Err("capability lease is bound to another workspace");
    }
    if request.context.task_id != Some(leaf.task_id)
        || request.principal.id != Some(leaf.task_id)
    {
        return Err("capability lease is bound to another task");
    }
    if request.context.on_behalf_of_user_id != leaf.on_behalf_of_user_id {
        return Err("capability lease on-behalf identity mismatch");
    }
    if request.context.device_id.is_some()
        && leaf.device_id.is_some()
        && request.context.device_id != leaf.device_id
    {
        return Err("capability lease is bound to another device");
    }
    let mut seen = HashSet::new();
    let by_id: HashMap<Uuid, &LeaseRow> = chain.iter().map(|row| (row.id, row)).collect();
    for lease in chain {
        if !seen.insert(lease.id) {
            return Err("capability lease delegation cycle detected");
        }
        if lease.depth > MAX_DELEGATION_DEPTH {
            return Err("capability lease delegation depth exceeded");
        }
        if lease.revoked_at.is_some() || lease.expires_at <= Utc::now() {
            return Err("capability lease or ancestor is expired or revoked");
        }
        if !matches!(
            lease.task_status.as_str(),
            "queued" | "dispatched" | "running" | "waiting_local_directory" | "deferred"
        ) {
            return Err("capability lease task is terminal");
        }
        if lease.claim_dispatched_at != lease.current_dispatched_at {
            return Err("capability lease claim fence is stale");
        }
        if let Some(parent_id) = lease.parent_token_id {
            let Some(parent) = by_id.get(&parent_id).copied() else {
                return Err("capability lease parent is missing");
            };
            if lease.depth != parent.depth + 1 || lease.parent_fence != Some(parent.fence) {
                return Err("capability lease parent fence or depth mismatch");
            }
            if !scope_is_subset(&lease.scope, &parent.scope) {
                return Err("child capability lease widens parent scope");
            }
        } else if lease.depth != 0 {
            return Err("root capability lease has non-zero depth");
        }
    }
    if !leaf.scope.iter().any(|capability| {
        capability.covers(
            request.action.as_str(),
            request.resource.resource_type.as_str(),
            request.resource.id,
            leaf.task_id,
        )
    }) {
        return Err("action/resource is outside capability lease scope");
    }
    Ok(())
}

pub fn scope_is_subset(child: &[Capability], parent: &[Capability]) -> bool {
    child.iter().all(|candidate| {
        parent.iter().any(|bound| {
            bound.action == candidate.action
                && bound.resource_type == candidate.resource_type
                && (bound.resource_id == "*" || bound.resource_id == candidate.resource_id)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(action: &str, resource_type: &str, owner: Uuid, actor: Uuid) -> AuthorizationRequest {
        AuthorizationRequest {
            principal: Principal {
                principal_type: PrincipalType::User,
                id: Some(actor),
            },
            action: Action::new(action),
            resource: Resource {
                resource_type: ResourceType::new(resource_type),
                id: Some(Uuid::now_v7()),
                workspace_id: Uuid::now_v7(),
                owner_id: Some(owner),
                attributes: json!({"private": true}),
            },
            context: AuthorizationContext {
                workspace_role: Some(WorkspaceRole::Admin),
                ..Default::default()
            },
            delegation_chain: Vec::new(),
        }
    }

    #[test]
    fn admin_does_not_automatically_read_private_runtime() {
        let owner = Uuid::now_v7();
        let admin = Uuid::now_v7();
        let decision = evaluate(
            &request(Action::RUNTIME_READ, ResourceType::RUNTIME, owner, admin),
            &[],
            &[],
        );
        assert_eq!(decision.effect, DecisionEffect::Deny);
    }

    #[test]
    fn require_approval_is_not_allow() {
        let actor = Uuid::now_v7();
        let mut req = request(Action::RUNTIME_UPDATE, ResourceType::RUNTIME, actor, actor);
        req.resource.attributes["require_approval"] = Value::Bool(true);
        let decision = evaluate(&req, &[], &[]);
        assert_eq!(decision.effect, DecisionEffect::RequireApproval);
        assert!(!decision.is_allowed());
        assert_eq!(decision.obligations[0].kind, "obtain_approval");

        req.context.approval_id = Some(Uuid::now_v7());
        let forged = evaluate(&req, &[], &[]);
        assert_eq!(forged.effect, DecisionEffect::RequireApproval);
        assert!(!forged.is_allowed());
    }

    #[test]
    fn child_scope_cannot_expand_parent() {
        let parent = vec![Capability::wildcard(
            Action::AGENT_INVOKE,
            ResourceType::AGENT_DEFINITION,
        )];
        let child = vec![Capability::wildcard(
            Action::CREDENTIAL_USE,
            ResourceType::CREDENTIAL,
        )];
        assert!(!scope_is_subset(&child, &parent));
        assert!(scope_is_subset(&parent, &parent));
    }

    #[test]
    fn agent_cannot_read_long_lived_secret_even_with_allow_grant() {
        let actor = Uuid::now_v7();
        let mut req = request(
            Action::CREDENTIAL_READ_SECRET,
            ResourceType::CREDENTIAL,
            actor,
            actor,
        );
        req.principal.principal_type = PrincipalType::AgentDefinition;
        req.context.on_behalf_of_user_id = Some(actor);
        let grant = Grant {
            id: Uuid::now_v7(),
            effect: "allow".to_string(),
            conditions: json!({}),
        };
        let decision = evaluate(&req, &[grant], &[]);
        assert_eq!(decision.effect, DecisionEffect::Deny);
    }
}
