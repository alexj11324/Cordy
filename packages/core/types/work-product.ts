/**
 * Work Product and execution provenance contracts exposed by the Go API.
 *
 * The server serializes pgx nullable values as either JSON null/string values
 * (depending on the pgx version), while the client contract deliberately
 * normalizes them to string | null at the API boundary.  Unknown provider,
 * relation, head, and discovery values remain strings so a newer backend can
 * be rendered without invalidating the whole response.
 */

export type WorkProduct = {
  id: string;
  workspace_id: string;
  kind: string;
  provider: string;
  external_identity: string;
  external_url: string | null;
  provider_record_type: string | null;
  provider_record_id: string | null;
  created_at: string;
  updated_at: string;
};

export type WorkProductRelation = {
  id: string;
  workspace_id: string;
  work_product_id: string;
  issue_id: string;
  task_id: string | null;
  run_id: string | null;
  relation_key: string;
  relation_source: string;
  attached_by_type: string;
  attached_by_id: string;
  attached_at: string;
  close_intent: boolean;
  detached_at: string | null;
  detached_by_type: string | null;
  detached_by_id: string | null;
  detached_task_id: string | null;
  detached_run_id: string | null;
};

export type ExecutionProvenance = {
  task_id: string;
  workspace_id: string;
  run_id: string | null;
  repo_identity: string;
  execution_workspace: string;
  head_branch: string | null;
  head_sha: string | null;
  head_state: string;
  started_at: string;
  finished_at: string | null;
  discovery_status: string;
  discovery_lease_id: string | null;
  discovery_match_count: number;
  discovery_reason: string | null;
  discovery_work_product_id: string | null;
  discovery_at: string | null;
  updated_at: string;
};

export type WorkProductPage = {
  products: WorkProduct[];
  page: number;
  per_page: number;
  has_more: boolean;
};

export type WorkProductRelationPage = {
  relations: WorkProductRelation[];
  page: number;
  per_page: number;
  has_more: boolean;
};

export type ExecutionProvenancePage = {
  provenance: ExecutionProvenance[];
  page: number;
  per_page: number;
  has_more: boolean;
};

/**
 * Human-created relations contain only user-controlled intent.  Actor, task,
 * run, relation key, and source are derived by the server from auth/request
 * context; accepting those fields here would suggest an unsafe impersonation
 * contract to callers.
 */
export type CreateWorkProductRelationRequest = {
  work_product_id: string;
  close_intent?: boolean;
};

export type WorkProductPageParams = {
  page?: number;
  per_page?: number;
};
