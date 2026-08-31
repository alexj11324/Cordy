/**
 * Public contracts for the Linear installation and project-binding flows.
 *
 * Provider enums intentionally remain strings at the API boundary. Linear can
 * add status types or connection states without making an older client erase
 * the value or fall back to an unrelated binding.
 */
export type LinearConnectionStatus = string;

export type LinearConnection = {
  id: string;
  workspace_id: string;
  organization_id: string;
  organization_name: string;
  actor_id: string;
  scopes: string[];
  webhook_id: string | null;
  status: LinearConnectionStatus;
  token_expires_at: string;
  last_success_at: string | null;
  last_error: string | null;
  created_at: string;
  updated_at: string;
};

export type LinearConnectionResponse = {
  configured: boolean;
  connected: boolean;
  connection: LinearConnection | null;
};

export type LinearConnectResponse = {
  authorization_url: string;
  state_expires_at: string;
};

export type LinearCatalogTeam = {
  id: string;
  key: string;
  name: string;
};

export type LinearCatalogProject = {
  id: string;
  name: string;
};

export type LinearCatalogState = {
  id: string;
  name: string;
  type: string;
  color: string;
};

export type LinearCatalogUser = {
  id: string;
  name: string;
  email: string | null;
};

export type LinearCatalogLabel = {
  id: string;
  name: string;
  color: string;
  is_group: boolean;
  parent_id: string | null;
  team_id: string | null;
};

export type LinearCatalogResponse = {
  teams: LinearCatalogTeam[];
  projects: LinearCatalogProject[];
  states: LinearCatalogState[];
  users: LinearCatalogUser[];
  labels: LinearCatalogLabel[];
};

export type LinearDryRunResponse = {
  patchbay_project_id: string;
  linear_project_id: string;
  sync_mode: string;
  initial_source_of_truth: string | null;
  local_issue_count: number;
  remote_issue_count: number;
  remote_issue_count_truncated: boolean;
  candidate_import_count: number;
  candidate_publish_count: number;
  unmapped_remote_status_count: number;
  exact_link_counts_available: boolean;
};

export type LinearInitialImportResponse = {
  queued: boolean;
  inbox_id: string;
};

export type LinearBindingStatus =
  | "draft"
  | "active"
  | "paused"
  | "tombstone"
  | (string & {});

export type LinearSyncMode =
  | "import"
  | "publish"
  | "two_way"
  | "not_synced"
  | (string & {});

export type LinearInitialSource = "linear" | "patchbay" | (string & {});

export type LinearProjectBinding = {
  id: string;
  workspace_id: string;
  connection_id: string;
  patchbay_project_id: string;
  linear_project_id: string;
  linear_team_id: string | null;
  status: LinearBindingStatus;
  sync_mode: LinearSyncMode;
  initial_source_of_truth: LinearInitialSource | null;
  status_mapping: Record<string, unknown>;
  agent_label_mapping: Record<string, unknown>;
  activated_at: string | null;
  paused_at: string | null;
  created_by_id: string;
  created_at: string;
  updated_at: string;
};

export type ListLinearBindingsResponse = {
  bindings: LinearProjectBinding[];
};

export type SaveLinearProjectBindingRequest = {
  connection_id: string;
  patchbay_project_id: string;
  linear_project_id: string;
  linear_team_id?: string | null;
  status?: LinearBindingStatus;
  sync_mode: LinearSyncMode;
  initial_source_of_truth?: LinearInitialSource | null;
  status_mapping?: Record<string, unknown>;
  agent_label_mapping?: Record<string, unknown>;
};
