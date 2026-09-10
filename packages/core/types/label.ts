/** Workspace-scoped label catalogs, separated by resource type. */
export type LabelResourceType = "issue" | "agent" | "skill" | "project";

export interface Label {
  id: string;
  workspace_id: string;
  resource_type?: LabelResourceType;
  name: string;
  description?: string;
  /** Normalized lowercase hex color, e.g. `#3b82f6`. */
  color: string;
  usage_count?: number;
  created_at: string;
  updated_at: string;
}

export interface CreateLabelRequest {
  resource_type?: LabelResourceType;
  name: string;
  description?: string;
  color: string;
}

export interface UpdateLabelRequest {
  name?: string;
  description?: string;
  color?: string;
}

export interface ListLabelsResponse {
  labels: Label[];
  total: number;
}

export interface IssueLabelsResponse {
  labels: Label[];
  issue_revision?: number;
}

export type ResourceLabelsResponse = IssueLabelsResponse;
