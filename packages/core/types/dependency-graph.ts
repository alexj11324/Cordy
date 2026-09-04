import type {
  Issue,
  IssueExecutorType,
  IssueOwnerType,
  IssueReviewerType,
} from "./issue";

export type DependencyGraphReadinessState =
  | "todo"
  | "ready"
  | "running"
  | "blocked"
  | "done"
  | "cancelled";

export type DependencyGraphOwner = {
  type: IssueOwnerType;
  id: string;
};

export type DependencyGraphExecutor = {
  type: IssueExecutorType;
  id: string;
};

export type DependencyGraphReviewer = {
  type: IssueReviewerType;
  id: string;
};

export type DependencyGraphPlan = {
  id: string;
  workspace_id: string;
  parent_issue_id: string;
  idempotency_key: string;
  request_hash?: string;
  goal: string;
  status: string;
  attention_required: boolean;
  attention_reason: string | null;
  created_by_type: string;
  created_by_id: string;
  created_at: string;
  updated_at: string;
};

export type DependencyGraphReadiness = {
  total: number;
  ready: number;
  running: number;
  blocked: number;
  done: number;
  cancelled: number;
};

export type DependencyGraphNodeReadiness = {
  state: DependencyGraphReadinessState | (string & {});
  gate_open: boolean;
  satisfied_prerequisites: number;
  total_prerequisites: number;
  unlock_condition: string;
};

export type DependencyGraphNode = {
  id: string;
  plan_id: string;
  workspace_id: string;
  temp_id: string;
  issue_id: string;
  issue?: Issue;
  title: string;
  description: string;
  acceptance_criteria: string[];
  context: Record<string, unknown>;
  outputs: string[];
  executor_type: IssueExecutorType | null;
  executor_id: string | null;
  candidate_executors: DependencyGraphExecutor[];
  owner_type: IssueOwnerType | null;
  owner_id: string | null;
  reviewer_type: IssueReviewerType | null;
  reviewer_id: string | null;
  runtime_id: string | null;
  model_id: string | null;
  wave: number;
  status: string;
  status_category: string;
  ready: boolean;
  blocked_by: string[];
  created_at: string;
  updated_at: string;
  readiness: DependencyGraphNodeReadiness;
};

export type DependencyGraphEdge = {
  id: string;
  plan_id: string;
  workspace_id: string;
  from_issue_id: string;
  to_issue_id: string;
  from: string;
  to: string;
  type: string;
  reason: string;
  consumed_output: string;
  created_at: string;
  prerequisite_status: string;
  satisfied: boolean;
  satisfied_prerequisites: number;
  total_prerequisites: number;
  unlock_condition: string;
};

export type DependencyGraphResponse = {
  plan: DependencyGraphPlan;
  parent?: Issue;
  children: Issue[];
  nodes: DependencyGraphNode[];
  edges: DependencyGraphEdge[];
  waves: string[][];
  readiness: DependencyGraphReadiness;
};

export type ListDependencyGraphsResponse = {
  graphs: DependencyGraphResponse[];
  next_cursor: string | null;
};
