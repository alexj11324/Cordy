export type LinearSyncMode = "two_way" | "pull_only" | "push_only";

export type LinearConnection = {
  id: string;
  workspace_id: string;
  organization_id: string;
  organization_name?: string | null;
  actor_id?: string | null;
  scopes: string[];
  status: "active" | "reauthorization_required" | "revoked" | string;
  token_expires_at?: string | null;
  created_at: string;
  updated_at: string;
};

export type LinearProjectBinding = {
  id: string;
  workspace_id: string;
  connection_id: string;
  patchbay_project_id?: string | null;
  linear_project_id: string;
  default_linear_team_id?: string | null;
  sync_mode: LinearSyncMode;
  status: "active" | "out_of_scope" | "tombstone" | string;
  created_at: string;
  updated_at: string;
};

export type LinearMemberBinding = {
  id: string;
  workspace_id: string;
  member_id?: string | null;
  linear_user_id: string;
  normalized_email: string;
  status: "active" | "unbound" | "diagnostic" | string;
  diagnostic?: string | null;
  created_at: string;
  updated_at: string;
};

export type LinearStatusBinding = {
  id: string;
  project_binding_id: string;
  patchbay_status: string;
  linear_status_id: string;
  created_at: string;
  updated_at: string;
};

export type LinearRelationLink = {
  id: string;
  workspace_id: string;
  from_issue_id: string;
  to_issue_id: string;
  linear_relation_id?: string | null;
  relation_type: "parent" | "blocks" | "blocked_by" | string;
  status: "active" | "conflict" | "tombstone" | string;
  created_at: string;
  updated_at: string;
};

export type LinearAgentBinding = {
  id: string;
  workspace_id: string;
  agent_id: string;
  linear_label_group_id: string;
  linear_label_id: string;
  label_name: string;
  created_at: string;
  updated_at: string;
};

export type LinearSyncConflict = {
  id: string;
  workspace_id: string;
  issue_id?: string | null;
  linear_issue_id?: string | null;
  field: string;
  local_value?: unknown;
  remote_value?: unknown;
  local_revision?: number | null;
  remote_updated_at?: string | null;
  correlation_id?: string | null;
  status: "open" | "resolved" | "ignored" | string;
  created_at: string;
  resolved_at?: string | null;
};

export type LinearConnectionResponse = {
  connected: boolean;
  connection: LinearConnection | null;
  project_bindings: LinearProjectBinding[];
};

export type LinearProjectBindingsResponse = {
  bindings: LinearProjectBinding[];
};

export type LinearMemberBindingsResponse = {
  bindings: LinearMemberBinding[];
};

export type LinearStatusBindingsResponse = {
  bindings: LinearStatusBinding[];
};

export type LinearIssueRelationsResponse = {
  relations: LinearRelationLink[];
};

export type LinearAgentBindingsResponse = {
  bindings: LinearAgentBinding[];
};

export type LinearConflictsResponse = {
  conflicts: LinearSyncConflict[];
};

export type LinearOAuthStartResponse = {
  authorization_url: string;
  state_expires_at: string;
};

export type LinearProjectBindingRequest = {
  linear_project_id: string;
  patchbay_project_id?: string | null;
  default_linear_team_id?: string | null;
  sync_mode: LinearSyncMode;
};

export type LinearMemberBindingRequest = {
  linear_user_id: string;
  email: string;
  active?: boolean;
  kind?: "human" | string;
};

export type LinearStatusBindingRequest = {
  patchbay_status: string;
  linear_status_id: string;
};

export type LinearIssueRelationRequest = {
  to_issue_id: string;
  relation_type: "parent" | "blocks" | "blocked_by" | string;
  linear_relation_id?: string | null;
  status?: "active" | "conflict" | "tombstone" | string;
};

export type LinearAgentBindingRequest = {
  agent_id: string;
  linear_label_group_id: string;
  linear_label_id: string;
  label_name: string;
};
