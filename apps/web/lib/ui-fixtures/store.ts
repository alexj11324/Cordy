const NOW = "2026-01-01T00:00:00Z";

export const FIXTURE_USER_ID = "user-preview";
export const FIXTURE_WORKSPACE_ID = "ws-preview";
export const FIXTURE_WORKSPACE_SLUG = "preview";

export type FixtureIssue = {
  id: string;
  workspace_id: string;
  number: number;
  identifier: string;
  title: string;
  description: string | null;
  status: string;
  status_category: string;
  priority: string;
  assignee_type: string | null;
  assignee_id: string | null;
  reviewer_type: string | null;
  reviewer_id: string | null;
  creator_type: string;
  creator_id: string;
  parent_issue_id: string | null;
  project_id: string | null;
  position: number;
  stage: number | null;
  start_date: string | null;
  due_date: string | null;
  metadata: Record<string, string | number | boolean>;
  properties: Record<string, unknown>;
  labels: unknown[];
  created_at: string;
  updated_at: string;
  last_activity_at: string | null;
};

export type FixtureUser = {
  id: string;
  name: string;
  email: string;
  avatar_url: string | null;
  onboarded_at: string | null;
  onboarding_questionnaire: Record<string, unknown>;
  starter_content_state: string | null;
  language: string | null;
  profile_description: string;
  timezone: string | null;
  created_at: string;
  updated_at: string;
};

export type FixtureWorkspace = {
  id: string;
  name: string;
  slug: string;
  description: string | null;
  context: string | null;
  settings: Record<string, unknown>;
  repos: unknown[];
  issue_prefix: string;
  avatar_url: string | null;
  created_at: string;
  updated_at: string;
};

export const FIXTURE_RUNTIME_ID = "runtime-local";
export const FIXTURE_AGENT_CONTENT_ID = "agent-content";
export const FIXTURE_AGENT_RESEARCH_ID = "agent-research";
export const FIXTURE_AGENT_CODING_ID = "agent-coding";

export type FixtureAgent = {
  id: string;
  workspace_id: string;
  runtime_id: string;
  runtime_bound: boolean;
  name: string;
  description: string;
  instructions: string;
  avatar_url: string | null;
  runtime_mode: "local";
  runtime_config: Record<string, unknown>;
  custom_args: string[];
  visibility: "workspace";
  permission_mode: "public_to";
  invocation_targets: Array<{ target_type: "workspace"; target_id: null }>;
  status: "idle" | "working";
  max_concurrent_tasks: number;
  model: string;
  owner_id: string | null;
  skills: unknown[];
  created_at: string;
  updated_at: string;
  archived_at: string | null;
  archived_by: string | null;
};

export type FixtureTimelineEntry = {
  type: "activity";
  id: string;
  actor_type: string;
  actor_id: string;
  created_at: string;
  action: string;
  details?: Record<string, unknown>;
};

export type FixtureTaskRun = {
  id: string;
  agent_id: string;
  runtime_id: string;
  issue_id: string;
  status: string;
  priority: number;
  dispatched_at: string | null;
  started_at: string | null;
  completed_at: string | null;
  result: unknown;
  error: string | null;
  created_at: string;
  kind?: string;
  handoff_note?: string;
  attribution?: {
    source: string;
    precise: boolean;
    delegated_from_task_id?: string;
  };
};

export type FixtureProject = {
  id: string;
  workspace_id: string;
  title: string;
  description: string | null;
  icon: string | null;
  status: string;
  priority: string;
  lead_type: string | null;
  lead_id: string | null;
  start_date: string | null;
  due_date: string | null;
  created_at: string;
  updated_at: string;
  issue_count: number;
  done_count: number;
  resource_count: number;
};

export type FixtureProjectResource = {
  id: string;
  project_id: string;
  workspace_id: string;
  resource_type: string;
  resource_ref: Record<string, unknown>;
  label: string | null;
  position: number;
  created_at: string;
  created_by: string | null;
};

export type FixtureStore = {
  user: FixtureUser;
  workspaces: FixtureWorkspace[];
  issues: FixtureIssue[];
  agents: FixtureAgent[];
  projects: FixtureProject[];
  projectResources: FixtureProjectResource[];
  nextIssueNumber: number;
};

function issue(
  partial: Pick<
    FixtureIssue,
    "id" | "number" | "title" | "status" | "priority" | "description" | "position"
  > &
    Partial<Pick<FixtureIssue, "assignee_type" | "assignee_id" | "reviewer_type" | "reviewer_id">>,
): FixtureIssue {
  return {
    workspace_id: FIXTURE_WORKSPACE_ID,
    identifier: `PRE-${partial.number}`,
    status_category: partial.status,
    assignee_type: "member",
    assignee_id: FIXTURE_USER_ID,
    reviewer_type: null,
    reviewer_id: null,
    creator_type: "member",
    creator_id: FIXTURE_USER_ID,
    parent_issue_id: null,
    project_id: null,
    stage: null,
    start_date: null,
    due_date: null,
    metadata: {},
    properties: {},
    labels: [],
    created_at: NOW,
    updated_at: NOW,
    last_activity_at: NOW,
    ...partial,
  };
}

function agent(
  partial: Pick<FixtureAgent, "id" | "name" | "description" | "status">,
): FixtureAgent {
  return {
    workspace_id: FIXTURE_WORKSPACE_ID,
    runtime_id: FIXTURE_RUNTIME_ID,
    runtime_bound: true,
    instructions: "",
    avatar_url: null,
    runtime_mode: "local",
    runtime_config: {},
    custom_args: [],
    visibility: "workspace",
    permission_mode: "public_to",
    invocation_targets: [{ target_type: "workspace", target_id: null }],
    max_concurrent_tasks: 3,
    model: "",
    owner_id: FIXTURE_USER_ID,
    skills: [],
    created_at: NOW,
    updated_at: NOW,
    archived_at: null,
    archived_by: null,
    ...partial,
  };
}

function seed(): FixtureStore {
  return {
    user: {
      id: FIXTURE_USER_ID,
      name: "Preview",
      email: "preview@local",
      avatar_url: null,
      onboarded_at: NOW,
      onboarding_questionnaire: {},
      starter_content_state: null,
      language: null,
      profile_description: "",
      timezone: null,
      created_at: NOW,
      updated_at: NOW,
    },
    workspaces: [
      {
        id: FIXTURE_WORKSPACE_ID,
        name: "Preview",
        slug: FIXTURE_WORKSPACE_SLUG,
        description: "Local fixture workspace",
        context: null,
        settings: {},
        repos: [],
        issue_prefix: "PRE",
        avatar_url: null,
        created_at: NOW,
        updated_at: NOW,
      },
    ],
    projects: [],
    projectResources: [],
    agents: [
      agent({
        id: FIXTURE_AGENT_CONTENT_ID,
        name: "Content",
        description: "Writes and edits product copy.",
        status: "idle",
      }),
      agent({
        id: FIXTURE_AGENT_RESEARCH_ID,
        name: "Research",
        description: "Audits the board and gathers context.",
        status: "idle",
      }),
      agent({
        id: FIXTURE_AGENT_CODING_ID,
        name: "Coding",
        description: "Implements the remaining UI work.",
        status: "working",
      }),
    ],
    issues: [
      issue({
        id: "issue-101",
        number: 101,
        title: "Refine workspace onboarding",
        description: "Make the first-run path easier to understand.",
        status: "backlog",
        priority: "high",
        position: 0,
        assignee_type: "agent",
        assignee_id: FIXTURE_AGENT_CONTENT_ID,
      }),
      issue({
        id: "issue-102",
        number: 102,
        title: "Polish issue board empty states",
        description: "Keep the board useful before real work arrives.",
        status: "todo",
        priority: "medium",
        position: 0,
        assignee_type: "agent",
        assignee_id: FIXTURE_AGENT_CODING_ID,
        reviewer_type: "agent",
        reviewer_id: FIXTURE_AGENT_RESEARCH_ID,
      }),
      issue({
        id: "issue-103",
        number: 103,
        title: "Add keyboard shortcuts",
        description: "Expose the common actions without extra chrome.",
        status: "todo",
        priority: "low",
        position: 1,
      }),
      issue({
        id: "issue-104",
        number: 104,
        title: "Connect a local device",
        description: "Let an agent pick up a task from this board.",
        status: "in_progress",
        priority: "high",
        position: 0,
        assignee_type: "agent",
        assignee_id: FIXTURE_AGENT_CODING_ID,
      }),
      issue({
        id: "issue-105",
        number: 105,
        title: "Review the landing header",
        description: "Check signed-out and signed-in treatments.",
        status: "in_review",
        priority: "medium",
        position: 0,
        assignee_type: "agent",
        assignee_id: FIXTURE_AGENT_CODING_ID,
        reviewer_type: "agent",
        reviewer_id: FIXTURE_AGENT_RESEARCH_ID,
      }),
      issue({
        id: "issue-106",
        number: 106,
        title: "Ship fixture API for web-dev",
        description: "Product screens render without the Rust server.",
        status: "done",
        priority: "high",
        position: 0,
        assignee_type: "agent",
        assignee_id: FIXTURE_AGENT_CONTENT_ID,
      }),
    ],
    nextIssueNumber: 107,
  };
}

function activity(
  partial: Omit<FixtureTimelineEntry, "type">,
): FixtureTimelineEntry {
  return { type: "activity", ...partial };
}

export function fixtureTimelineForIssue(issueId: string): FixtureTimelineEntry[] {
  if (issueId === "issue-102") {
    return [
      activity({
        id: "act-102-created",
        actor_type: "member",
        actor_id: FIXTURE_USER_ID,
        action: "created",
        created_at: "2026-01-01T00:00:00Z",
      }),
      activity({
        id: "act-102-assigned",
        actor_type: "member",
        actor_id: FIXTURE_USER_ID,
        action: "assignee_changed",
        details: {
          from_type: null,
          from_id: null,
          to_type: "agent",
          to_id: FIXTURE_AGENT_RESEARCH_ID,
        },
        created_at: "2026-01-01T00:05:00Z",
      }),
      activity({
        id: "act-102-handoff-review",
        actor_type: "agent",
        actor_id: FIXTURE_AGENT_CONTENT_ID,
        action: "review_handoff",
        details: {
          from_type: "agent",
          from_id: FIXTURE_AGENT_CONTENT_ID,
          to_type: "agent",
          to_id: FIXTURE_AGENT_RESEARCH_ID,
          from_status: "in_progress",
          to_status: "in_review",
        },
        created_at: "2026-01-01T00:12:00Z",
      }),
      activity({
        id: "act-102-handoff",
        actor_type: "agent",
        actor_id: FIXTURE_AGENT_RESEARCH_ID,
        action: "review_handoff",
        details: {
          from_type: "agent",
          from_id: FIXTURE_AGENT_RESEARCH_ID,
          to_type: "agent",
          to_id: FIXTURE_AGENT_CODING_ID,
          from_status: "in_review",
          to_status: "todo",
        },
        created_at: "2026-01-01T00:20:00Z",
      }),
    ];
  }
  if (issueId === "issue-105") {
    return [
      activity({
        id: "act-105-created",
        actor_type: "member",
        actor_id: FIXTURE_USER_ID,
        action: "created",
        created_at: "2026-01-01T00:00:00Z",
      }),
      activity({
        id: "act-105-handoff",
        actor_type: "agent",
        actor_id: FIXTURE_AGENT_CODING_ID,
        action: "review_handoff",
        details: {
          from_type: "agent",
          from_id: FIXTURE_AGENT_CODING_ID,
          to_type: "agent",
          to_id: FIXTURE_AGENT_RESEARCH_ID,
          from_status: "in_progress",
          to_status: "in_review",
        },
        created_at: "2026-01-01T00:30:00Z",
      }),
    ];
  }
  return [
    activity({
      id: `act-${issueId}-created`,
      actor_type: "member",
      actor_id: FIXTURE_USER_ID,
      action: "created",
      created_at: NOW,
    }),
  ];
}

function taskRun(partial: FixtureTaskRun): FixtureTaskRun {
  return partial;
}

export function fixtureTaskRunsForIssue(issueId: string): FixtureTaskRun[] {
  if (issueId === "issue-102") {
    return [
      taskRun({
        id: "task-102-research",
        agent_id: FIXTURE_AGENT_RESEARCH_ID,
        runtime_id: FIXTURE_RUNTIME_ID,
        issue_id: "issue-102",
        status: "completed",
        priority: 0,
        dispatched_at: "2026-01-01T00:06:00Z",
        started_at: "2026-01-01T00:06:10Z",
        completed_at: "2026-01-01T00:18:00Z",
        result: {
          output:
            "Empty-state copy is consistent across the board. Remaining work is polish on the zero-issue column and the filtered-empty hint.",
        },
        error: null,
        created_at: "2026-01-01T00:06:00Z",
        kind: "direct",
        attribution: { source: "direct_human", precise: true },
      }),
      taskRun({
        id: "task-102-coding",
        agent_id: FIXTURE_AGENT_CODING_ID,
        runtime_id: FIXTURE_RUNTIME_ID,
        issue_id: "issue-102",
        status: "running",
        priority: 0,
        dispatched_at: "2026-08-29T12:48:00Z",
        started_at: "2026-08-29T12:48:05Z",
        completed_at: null,
        result: null,
        error: null,
        created_at: "2026-08-29T12:48:00Z",
        kind: "direct",
        handoff_note: "Research finished the empty-state audit. Polish the remaining copy.",
        attribution: {
          source: "delegation",
          precise: true,
          delegated_from_task_id: "task-102-research",
        },
      }),
    ];
  }
  if (issueId === "issue-104") {
    return [
      taskRun({
        id: "task-104-coding",
        agent_id: FIXTURE_AGENT_CODING_ID,
        runtime_id: FIXTURE_RUNTIME_ID,
        issue_id: "issue-104",
        status: "running",
        priority: 0,
        dispatched_at: "2026-01-01T00:10:00Z",
        started_at: "2026-01-01T00:10:05Z",
        completed_at: null,
        result: null,
        error: null,
        created_at: "2026-01-01T00:10:00Z",
        kind: "direct",
        attribution: { source: "direct_human", precise: true },
      }),
    ];
  }
  if (issueId === "issue-105") {
    return [
      taskRun({
        id: "task-105-coding",
        agent_id: FIXTURE_AGENT_CODING_ID,
        runtime_id: FIXTURE_RUNTIME_ID,
        issue_id: "issue-105",
        status: "completed",
        priority: 0,
        dispatched_at: "2026-01-01T00:12:00Z",
        started_at: "2026-01-01T00:12:05Z",
        completed_at: "2026-01-01T00:28:00Z",
        result: null,
        error: null,
        created_at: "2026-01-01T00:12:00Z",
        kind: "direct",
        attribution: { source: "direct_human", precise: true },
      }),
      taskRun({
        id: "task-105-research",
        agent_id: FIXTURE_AGENT_RESEARCH_ID,
        runtime_id: FIXTURE_RUNTIME_ID,
        issue_id: "issue-105",
        status: "queued",
        priority: 0,
        dispatched_at: null,
        started_at: null,
        completed_at: null,
        result: null,
        error: null,
        created_at: "2026-01-01T00:30:00Z",
        kind: "direct",
        handoff_note: "Header treatments are implemented. Please review signed-in vs signed-out.",
        attribution: {
          source: "delegation",
          precise: true,
          delegated_from_task_id: "task-105-coding",
        },
      }),
    ];
  }
  return [];
}

export function fixtureWorkingAgents() {
  return [
    {
      id: FIXTURE_AGENT_CODING_ID,
      name: "Coding",
      avatar_url: null,
      running_task_count: 2,
      issue_ids: ["issue-102", "issue-104"],
    },
  ];
}

export function fixtureAgentTaskSnapshot(): FixtureTaskRun[] {
  return [
    ...fixtureTaskRunsForIssue("issue-102"),
    ...fixtureTaskRunsForIssue("issue-104"),
    ...fixtureTaskRunsForIssue("issue-105"),
  ].filter((task) => task.status === "running" || task.status === "queued");
}

let store: FixtureStore = seed();

export function resetUiFixtureStore(): void {
  store = seed();
}

export function getUiFixtureStore(): FixtureStore {
  return store;
}

export function fixtureNow(): string {
  return new Date().toISOString();
}

export const SYSTEM_STATUS_KEYS = [
  "backlog",
  "todo",
  "in_progress",
  "in_review",
  "done",
  "blocked",
  "cancelled",
] as const;

export const SYSTEM_STATUS_NAMES: Record<(typeof SYSTEM_STATUS_KEYS)[number], string> = {
  backlog: "Backlog",
  todo: "Todo",
  in_progress: "In Progress",
  in_review: "In Review",
  done: "Done",
  blocked: "Blocked",
  cancelled: "Cancelled",
};
