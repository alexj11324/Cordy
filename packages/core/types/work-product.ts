export type WorkProductKind =
  | "pull_request"
  | "branch"
  | "commit"
  | "preview"
  | "artifact"
  | "document";

export type WorkProductRelationSource =
  | "manual_explicit"
  | "task_explicit"
  | "execution_branch_discovery";

export type WorkProductDiscoveryStatus =
  | "not_attempted"
  | "pending"
  | "in_progress"
  | "unassociated"
  | "ambiguous"
  | "associated"
  | "ineligible";

export type WorkProductRelation = {
  id: string;
  workspace_id: string;
  work_product_id: string;
  issue_id: string | null;
  task_id: string | null;
  run_id: string | null;
  relation_key: string;
  relation_source: WorkProductRelationSource | string;
  attached_by_type: "user" | "agent" | string;
  attached_by_id: string;
  attached_at: string;
  close_intent: boolean;
  detached_at: string | null;
  detached_by_type: string | null;
  detached_by_id: string | null;
  detached_task_id: string | null;
  detached_run_id: string | null;
};

export type WorkProduct = {
  id: string;
  workspace_id: string;
  kind: WorkProductKind | string;
  provider: string;
  external_identity: string;
  external_url: string | null;
  provider_record_type: string | null;
  provider_record_id: string | null;
  created_at: string;
  updated_at: string;
  association_state: "associated" | "unassociated" | string;
  relation: WorkProductRelation | null;
};

export type ExecutionProvenance = {
  task_id: string;
  workspace_id: string;
  run_id: string | null;
  repo_identity: string | null;
  execution_workspace: string | null;
  head_branch: string | null;
  head_sha: string | null;
  head_state: string;
  started_at: string | null;
  finished_at: string | null;
  discovery_status: WorkProductDiscoveryStatus | string;
  discovery_match_count: number;
  discovery_reason: string | null;
  discovery_work_product_id: string | null;
  discovery_at: string | null;
  updated_at: string;
};

export type IssueWorkProductsResponse = {
  work_products: WorkProduct[];
};

export type TaskWorkProductsResponse = {
  task_id: string;
  provenances: ExecutionProvenance[];
  work_products: WorkProduct[];
};

export type UnassociatedWorkProductsResponse = {
  work_products: WorkProduct[];
};

export type AttachIssuePullRequestRequest = {
  url: string;
  title?: string;
  state?: string;
  branch?: string;
  head_ref_name?: string;
  head_sha?: string;
  author_login?: string;
  close_intent?: boolean;
};

export type AttachIssuePullRequestResponse = {
  pull_request: import("./github").GitHubPullRequest;
  work_product: WorkProduct;
  relation: WorkProductRelation;
};

export type AttachWorkProductRequest = {
  work_product_id: string;
  close_intent?: boolean;
};

export type AttachWorkProductResponse = {
  work_product: WorkProduct;
  relation: WorkProductRelation;
};
