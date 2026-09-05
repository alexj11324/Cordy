import type { Agent, AgentTask, Issue, Workspace } from "@patchbay/core/types";

const timestamp = "2026-09-05T12:00:00Z";
export const workspace: Workspace = {
  id: "card-preview-workspace",
  slug: "card-preview",
  name: "Card preview",
  description: null,
  context: null,
  settings: {},
  repos: [],
  issue_prefix: "PREVIEW",
  avatar_url: null,
  created_at: timestamp,
  updated_at: timestamp,
};
export const agent: Agent = {
  id: "card-preview-agent",
  workspace_id: workspace.id,
  runtime_id: "card-preview-runtime",
  name: "Preview Agent",
  description: "In-memory display fixture",
  instructions: "",
  avatar_url: null,
  runtime_mode: "local",
  runtime_config: {},
  custom_args: [],
  visibility: "private",
  permission_mode: "private",
  invocation_targets: [],
  status: "idle",
  max_concurrent_tasks: 1,
  model: "",
  owner_id: null,
  skills: [],
  created_at: timestamp,
  updated_at: timestamp,
  archived_at: null,
  archived_by: null,
};
export const issue: Issue = {
  id: "card-preview-issue",
  workspace_id: workspace.id,
  number: 1,
  identifier: "PREVIEW-1",
  title: "Polish the task card animation",
  description: "This is the real card, with in-memory preview data.",
  status: "in_progress",
  priority: "high",
  owner_type: null,
  owner_id: null,
  executor_type: "agent",
  executor_id: agent.id,
  reviewer_type: null,
  reviewer_id: null,
  creator_type: "member",
  creator_id: "card-preview-member",
  parent_issue_id: null,
  project_id: null,
  position: 0,
  stage: null,
  start_date: null,
  due_date: null,
  metadata: {},
  properties: {},
  labels: [],
  created_at: timestamp,
  updated_at: timestamp,
};
export type ExecutionState = "idle" | "queued" | "running";

// No issue status -> execution conversion: these are independent server fields.
export function taskSnapshot(state: ExecutionState): AgentTask[] {
  if (state === "idle") return [];
  return [
    {
      id: "card-preview-task",
      workspace_id: workspace.id,
      agent_id: agent.id,
      runtime_id: agent.runtime_id,
      issue_id: issue.id,
      status: state,
      priority: 0,
      dispatched_at: null,
      started_at: state === "running" ? timestamp : null,
      completed_at: null,
      result: null,
      error: null,
      created_at: timestamp,
    },
  ];
}
