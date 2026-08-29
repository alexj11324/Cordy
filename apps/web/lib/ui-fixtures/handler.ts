import { parseUiFixtureMode, type UiFixtureMode } from "./mode";
import {
  FIXTURE_USER_ID,
  FIXTURE_WORKSPACE_ID,
  SYSTEM_STATUS_KEYS,
  SYSTEM_STATUS_NAMES,
  fixtureAgentTaskSnapshot,
  fixtureNow,
  fixtureTaskRunsForIssue,
  fixtureTimelineForIssue,
  fixtureWorkingAgents,
  getUiFixtureStore,
  type FixtureIssue,
} from "./store";

export type FixtureHttpRequest = {
  method: string;
  pathname: string;
  search: URLSearchParams;
  cookieHeader: string | null;
  body: unknown;
  workspaceSlug?: string | null;
};

export type FixtureHttpResult = {
  status: number;
  body?: unknown;
};

function json(body: unknown, status = 200): FixtureHttpResult {
  return { status, body };
}

function noContent(): FixtureHttpResult {
  return { status: 204 };
}

function notFound(): FixtureHttpResult {
  return { status: 404, body: { error: "Not found" } };
}

function meForMode(mode: UiFixtureMode) {
  const { user } = getUiFixtureStore();
  if (mode === "onboarding" && user.onboarded_at === "2026-01-01T00:00:00Z") {
    return { ...user, onboarded_at: null };
  }
  return user;
}

function workspacesForMode(mode: UiFixtureMode) {
  const { workspaces } = getUiFixtureStore();
  if (mode === "onboarding") {
    return workspaces.filter((workspace) => workspace.id !== FIXTURE_WORKSPACE_ID);
  }
  return workspaces;
}

function member() {
  const { user } = getUiFixtureStore();
  return {
    id: "member-preview",
    workspace_id: FIXTURE_WORKSPACE_ID,
    user_id: user.id,
    role: "owner",
    created_at: user.created_at,
    name: user.name,
    email: user.email,
    avatar_url: user.avatar_url,
  };
}

function issueStatuses(workspaceId: string) {
  const statuses = SYSTEM_STATUS_KEYS.map((key, position) => ({
    id: `status-${key}`,
    workspace_id: workspaceId,
    key,
    name: SYSTEM_STATUS_NAMES[key],
    description: "",
    category: key,
    color: "#6b7280",
    is_system: true,
    position,
    archived_at: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  }));
  return {
    statuses,
    categories: [...SYSTEM_STATUS_KEYS],
    total: statuses.length,
  };
}

function filterIssues(search: URLSearchParams): FixtureIssue[] {
  const { issues } = getUiFixtureStore();
  const workspaceId = search.get("workspace_id");
  const statuses = search.get("statuses")?.split(",").filter(Boolean);
  const status = search.get("status");
  return issues.filter((item) => {
    if (workspaceId && item.workspace_id !== workspaceId) return false;
    if (status && item.status !== status) return false;
    if (statuses && statuses.length > 0 && !statuses.includes(item.status)) {
      return false;
    }
    return true;
  });
}

function asIssueList(issues: FixtureIssue[]) {
  return { issues, total: issues.length };
}

function readBodyRecord(body: unknown): Record<string, unknown> {
  if (body && typeof body === "object" && !Array.isArray(body)) {
    return body as Record<string, unknown>;
  }
  return {};
}

function runtime() {
  return {
    id: "runtime-local",
    workspace_id: FIXTURE_WORKSPACE_ID,
    daemon_id: "daemon-preview",
    name: "Local runtime",
    runtime_mode: "local",
    provider: "claude",
    launch_header: "claude",
    status: "online",
    device_info: "fixture",
    metadata: {},
    owner_id: FIXTURE_USER_ID,
    visibility: "private",
    last_seen_at: fixtureNow(),
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };
}

export function handleFixtureRequest(request: FixtureHttpRequest): FixtureHttpResult {
  const method = request.method.toUpperCase();
  const path = request.pathname.replace(/\/+$/, "") || "/";
  const mode = parseUiFixtureMode(request.cookieHeader);
  const store = getUiFixtureStore();

  if (path === "/api/config" && method === "GET") {
    return json({
      cdn_domain: "",
      cdn_signed: false,
      allow_signup: true,
      google_client_id: "",
      workspace_creation_disabled: false,
      vcs_integration_available: false,
      local_worktree_supported: false,
      feature_flags: {},
    });
  }

  if (path === "/api/me" && method === "GET") {
    return json(meForMode(mode));
  }

  if (path === "/api/me" && method === "PATCH") {
    const patch = readBodyRecord(request.body);
    Object.assign(store.user, patch, { updated_at: fixtureNow() });
    return json(meForMode(mode));
  }

  if (path === "/api/me/onboarding" && method === "PATCH") {
    const patch = readBodyRecord(request.body);
    const questionnaire = patch.questionnaire;
    if (questionnaire && typeof questionnaire === "object") {
      store.user.onboarding_questionnaire = {
        ...store.user.onboarding_questionnaire,
        ...(questionnaire as Record<string, unknown>),
      };
    }
    store.user.updated_at = fixtureNow();
    return json(meForMode(mode));
  }

  if (path === "/api/me/onboarding/complete" && method === "POST") {
    store.user.onboarded_at = fixtureNow();
    store.user.updated_at = store.user.onboarded_at;
    return json(store.user);
  }

  if (path === "/api/workspaces" && method === "GET") {
    return json(workspacesForMode(mode));
  }

  if (path === "/api/workspaces" && method === "POST") {
    const patch = readBodyRecord(request.body);
    const slug =
      typeof patch.slug === "string" && patch.slug.length > 0
        ? patch.slug
        : `ws-${store.workspaces.length + 1}`;
    const created = {
      id: `ws-${slug}`,
      name: typeof patch.name === "string" ? patch.name : slug,
      slug,
      description: typeof patch.description === "string" ? patch.description : null,
      context: typeof patch.context === "string" ? patch.context : null,
      settings: {},
      repos: [],
      issue_prefix:
        typeof patch.issue_prefix === "string" ? patch.issue_prefix : "PRE",
      avatar_url: null,
      created_at: fixtureNow(),
      updated_at: fixtureNow(),
    };
    store.workspaces.push(created);
    return json(created, 201);
  }

  const workspaceMatch = path.match(/^\/api\/workspaces\/([^/]+)$/);
  if (workspaceMatch && method === "GET") {
    const workspace = store.workspaces.find(
      (item) => item.id === workspaceMatch[1] || item.slug === workspaceMatch[1],
    );
    return workspace ? json(workspace) : notFound();
  }

  const membersMatch = path.match(/^\/api\/workspaces\/([^/]+)\/members$/);
  if (membersMatch && method === "GET") {
    return json([member()]);
  }

  if (path === "/api/issue-statuses" && method === "GET") {
    return json(issueStatuses(FIXTURE_WORKSPACE_ID));
  }

  if (
    (path === "/api/issues" && method === "GET") ||
    (path === "/api/issues/query" && method === "POST")
  ) {
    const search =
      method === "POST" && request.body && typeof request.body === "object"
        ? new URLSearchParams(
            Object.fromEntries(
              Object.entries(readBodyRecord(request.body)).map(([key, value]) => [
                key,
                String(value),
              ]),
            ),
          )
        : request.search;
    return json(asIssueList(filterIssues(search)));
  }

  if (path === "/api/issues/grouped" && method === "GET") {
    const issues = filterIssues(request.search);
    const byStatus = new Map<string, FixtureIssue[]>();
    for (const item of issues) {
      const group = byStatus.get(item.status) ?? [];
      group.push(item);
      byStatus.set(item.status, group);
    }
    return json({
      groups: [...byStatus.entries()].map(([status, groupIssues]) => ({
        id: status,
        assignee_type: null,
        assignee_id: null,
        issues: groupIssues,
        total: groupIssues.length,
      })),
    });
  }

  if (path === "/api/issues" && method === "POST") {
    const patch = readBodyRecord(request.body);
    const number = store.nextIssueNumber;
    store.nextIssueNumber += 1;
    const status = typeof patch.status === "string" ? patch.status : "todo";
    const created: FixtureIssue = {
      id: `issue-${number}`,
      workspace_id:
        typeof patch.workspace_id === "string"
          ? patch.workspace_id
          : FIXTURE_WORKSPACE_ID,
      number,
      identifier: `PRE-${number}`,
      title: typeof patch.title === "string" ? patch.title : "Untitled",
      description: typeof patch.description === "string" ? patch.description : null,
      status,
      status_category: status,
      priority: typeof patch.priority === "string" ? patch.priority : "none",
      assignee_type: typeof patch.assignee_type === "string" ? patch.assignee_type : null,
      assignee_id: typeof patch.assignee_id === "string" ? patch.assignee_id : null,
      reviewer_type: typeof patch.reviewer_type === "string" ? patch.reviewer_type : null,
      reviewer_id: typeof patch.reviewer_id === "string" ? patch.reviewer_id : null,
      creator_type: "member",
      creator_id: FIXTURE_USER_ID,
      parent_issue_id: null,
      project_id: typeof patch.project_id === "string" ? patch.project_id : null,
      position: 0,
      stage: null,
      start_date: null,
      due_date: null,
      metadata: {},
      properties: {},
      labels: [],
      created_at: fixtureNow(),
      updated_at: fixtureNow(),
      last_activity_at: fixtureNow(),
    };
    store.issues.push(created);
    return json(created, 201);
  }

  if (path === "/api/issues/child-progress" && method === "GET") {
    return json({ progress: [] });
  }

  const issueMatch = path.match(/^\/api\/issues\/([^/]+)$/);
  if (issueMatch && method === "GET") {
    const found = store.issues.find(
      (item) => item.id === issueMatch[1] || item.identifier === issueMatch[1],
    );
    return found ? json(found) : notFound();
  }
  if (issueMatch && method === "PATCH") {
    const found = store.issues.find(
      (item) => item.id === issueMatch[1] || item.identifier === issueMatch[1],
    );
    if (!found) return notFound();
    const patch = readBodyRecord(request.body);
    const reviewerInRequest =
      Object.prototype.hasOwnProperty.call(patch, "reviewer_type") ||
      Object.prototype.hasOwnProperty.call(patch, "reviewer_id");
    const assigneeTouched =
      Object.prototype.hasOwnProperty.call(patch, "assignee_type") ||
      Object.prototype.hasOwnProperty.call(patch, "assignee_id");
    let nextReviewerType = Object.prototype.hasOwnProperty.call(patch, "reviewer_type")
      ? typeof patch.reviewer_type === "string"
        ? patch.reviewer_type
        : null
      : found.reviewer_type;
    let nextReviewerId = Object.prototype.hasOwnProperty.call(patch, "reviewer_id")
      ? typeof patch.reviewer_id === "string"
        ? patch.reviewer_id
        : null
      : found.reviewer_id;
    if (found.reviewer_type && found.reviewer_id && (!nextReviewerType || !nextReviewerId)) {
      return json(
        {
          code: "reviewer_cannot_clear",
          error: "a reviewer cannot be removed once set",
        },
        400,
      );
    }
    const nextStatus = typeof patch.status === "string" ? patch.status : found.status;
    let nextAssigneeType = Object.prototype.hasOwnProperty.call(patch, "assignee_type")
      ? typeof patch.assignee_type === "string"
        ? patch.assignee_type
        : null
      : found.assignee_type;
    let nextAssigneeId = Object.prototype.hasOwnProperty.call(patch, "assignee_id")
      ? typeof patch.assignee_id === "string"
        ? patch.assignee_id
        : null
      : found.assignee_id;
    const enteringReview = found.status !== "in_review" && nextStatus === "in_review";
    if (
      enteringReview &&
      !reviewerInRequest &&
      assigneeTouched &&
      !found.reviewer_type &&
      nextAssigneeType &&
      nextAssigneeId &&
      (nextAssigneeType !== found.assignee_type || nextAssigneeId !== found.assignee_id)
    ) {
      nextReviewerType = nextAssigneeType;
      nextReviewerId = nextAssigneeId;
      nextAssigneeType = found.assignee_type;
      nextAssigneeId = found.assignee_id;
    }
    if (nextStatus === "in_review") {
      if (!nextReviewerType || !nextReviewerId) {
        return json(
          {
            code: "review_handoff_required",
            error:
              "moving an issue into review requires assigning a different reviewer in the same update",
          },
          400,
        );
      }
      if (nextReviewerType === nextAssigneeType && nextReviewerId === nextAssigneeId) {
        return json(
          {
            code: "review_handoff_required",
            error:
              "moving an issue into review requires assigning a different reviewer in the same update",
          },
          400,
        );
      }
    }
    Object.assign(found, patch, { updated_at: fixtureNow() });
    if (typeof patch.status === "string") {
      found.status_category = patch.status;
    }
    found.assignee_type = nextAssigneeType;
    found.assignee_id = nextAssigneeId;
    found.reviewer_type = nextReviewerType;
    found.reviewer_id = nextReviewerId;
    return json(found);
  }

  const issueSubMatch = path.match(/^\/api\/issues\/([^/]+)\/([^/]+)$/);
  if (issueSubMatch && method === "GET") {
    const issueKey = issueSubMatch[1];
    const found = store.issues.find(
      (item) => item.id === issueKey || item.identifier === issueKey,
    );
    const issueId = found?.id ?? issueKey;
    const sub = issueSubMatch[2];
    if (sub === "children") return json({ issues: [] });
    if (sub === "labels") return json({ labels: [] });
    if (sub === "pull-requests") return json({ pull_requests: [] });
    if (sub === "attachments") return json([]);
    if (sub === "timeline") return json(fixtureTimelineForIssue(issueId));
    if (sub === "subscribers") return json([]);
    if (sub === "comments") return json([]);
    if (sub === "task-runs") return json(fixtureTaskRunsForIssue(issueId));
  }

  const moveMatch = path.match(/^\/api\/issues\/([^/]+)\/move$/);
  if (moveMatch && method === "POST") {
    const found = store.issues.find((item) => item.id === moveMatch[1]);
    if (!found) return notFound();
    const patch = readBodyRecord(request.body);
    if (typeof patch.status === "string") {
      found.status = patch.status;
      found.status_category = patch.status;
    }
    found.updated_at = fixtureNow();
    return json(found);
  }

  if (path === "/api/issues/table/groups" && method === "POST") {
    const issues = store.issues;
    return json({
      query_fingerprint: "fixture",
      total: issues.length,
      groups: SYSTEM_STATUS_KEYS.map((status) => ({
        key: `status:${status}`,
        value: { kind: "status", status },
        count: issues.filter((item) => item.status === status).length,
      })),
      next_cursor: null,
    });
  }

  if (path === "/api/issues/table/rows" && method === "POST") {
    const patch = readBodyRecord(request.body);
    const groupKey = typeof patch.group_key === "string" ? patch.group_key : null;
    const statusKey = groupKey?.startsWith("status:")
      ? groupKey.slice("status:".length)
      : groupKey;
    const issues = store.issues.filter((item) =>
      statusKey ? item.status === statusKey : true,
    );
    return json({
      query_fingerprint: "fixture",
      group_key: groupKey,
      parent_id: null,
      total: issues.length,
      rows: issues.map((item) => ({ issue: item, direct_child_count: 0 })),
      branch_total: issues.length,
      next_cursor: null,
    });
  }

  if (path === "/api/issues/table/facets" && method === "POST") {
    const issues = store.issues;
    return json({
      query_fingerprint: "fixture",
      total: issues.length,
      facets: [
        {
          kind: "status",
          values: SYSTEM_STATUS_KEYS.map((status) => ({
            key: status,
            count: issues.filter((item) => item.status === status).length,
          })),
        },
      ],
    });
  }

  if (path === "/api/labels" && method === "GET") {
    return json({ labels: [], total: 0 });
  }
  if (path === "/api/properties" && method === "GET") {
    return json({ properties: [], total: 0 });
  }
  if (path === "/api/issue-views" && method === "GET") {
    return json([]);
  }
  if (path === "/api/quick-actions" && method === "GET") {
    return json({ quick_actions: [] });
  }
  if (path === "/api/issue-view-preferences" && method === "GET") {
    return json({
      scope_type: request.search.get("scope_type") ?? "workspace",
      scope_id: request.search.get("scope_id"),
      prefs: { hidden: [], order: [] },
      updated_at: "2026-01-01T00:00:00Z",
    });
  }
  if (path === "/api/working-agents" && method === "GET") {
    return json(fixtureWorkingAgents());
  }
  if (path === "/api/inbox" && method === "GET") {
    return json([]);
  }
  if (path === "/api/inbox/archived" && method === "GET") {
    return json([]);
  }
  if (path === "/api/inbox/unread-count" && method === "GET") {
    return json({ count: 0 });
  }
  if (path === "/api/inbox/unread-summary" && method === "GET") {
    return json([]);
  }
  if (path === "/api/pins" && method === "GET") {
    return json([]);
  }
  if (path === "/api/agents" && method === "GET") {
    return json(store.agents);
  }
  const agentMatch = path.match(/^\/api\/agents\/([^/]+)$/);
  if (agentMatch && method === "GET") {
    const found = store.agents.find((item) => item.id === agentMatch[1]);
    return found ? json(found) : notFound();
  }
  if (path === "/api/runtimes" && method === "GET") {
    return json([runtime()]);
  }
  if (path === "/api/chat/sessions" && method === "GET") {
    return json([]);
  }
  if (path === "/api/chat/pinned-agents" && method === "GET") {
    return json([]);
  }
  if (path === "/api/chat/pending-tasks" && method === "GET") {
    return json({ tasks: [] });
  }
  if (path === "/api/chat/pending-tasks/has-any" && method === "GET") {
    return json({ has_pending: false });
  }
  if (path === "/api/invitations" && method === "GET") {
    return json([]);
  }
  if (path === "/api/agent-activity-30d" && method === "GET") {
    return json([]);
  }
  if (path === "/api/agent-run-counts" && method === "GET") {
    return json([]);
  }
  if (path === "/api/agent-task-snapshot" && method === "GET") {
    return json(fixtureAgentTaskSnapshot());
  }
  if (path === "/api/squads" && method === "GET") {
    return json([]);
  }
  if (path === "/api/projects" && method === "POST") {
    const patch = readBodyRecord(request.body);
    const resources = Array.isArray(patch.resources) ? patch.resources : [];
    const slug = request.workspaceSlug;
    const workspace =
      store.workspaces.find((item) => item.slug === slug || item.id === slug) ??
      store.workspaces.at(-1);
    const created = {
      id: `project-${store.projects.length + 1}`,
      workspace_id: workspace?.id ?? FIXTURE_WORKSPACE_ID,
      title: typeof patch.title === "string" ? patch.title : "Untitled",
      description: typeof patch.description === "string" ? patch.description : null,
      icon: typeof patch.icon === "string" ? patch.icon : null,
      status: typeof patch.status === "string" ? patch.status : "planned",
      priority: typeof patch.priority === "string" ? patch.priority : "none",
      lead_type: null,
      lead_id: null,
      start_date: null,
      due_date: null,
      created_at: fixtureNow(),
      updated_at: fixtureNow(),
      issue_count: 0,
      done_count: 0,
      resource_count: resources.length,
    };
    store.projects.push(created);
    for (const [index, item] of resources.entries()) {
      if (!item || typeof item !== "object" || Array.isArray(item)) continue;
      const record = item as Record<string, unknown>;
      const resourceRef =
        record.resource_ref &&
        typeof record.resource_ref === "object" &&
        !Array.isArray(record.resource_ref)
          ? (record.resource_ref as Record<string, unknown>)
          : {};
      store.projectResources.push({
        id: `resource-${store.projectResources.length + 1}`,
        project_id: created.id,
        workspace_id: created.workspace_id,
        resource_type:
          typeof record.resource_type === "string" ? record.resource_type : "github_repo",
        resource_ref: resourceRef,
        label: typeof record.label === "string" ? record.label : null,
        position: typeof record.position === "number" ? record.position : index,
        created_at: fixtureNow(),
        created_by: FIXTURE_USER_ID,
      });
    }
    return json(created, 201);
  }
  if (path === "/api/projects" && method === "GET") {
    return json({ projects: store.projects, total: store.projects.length });
  }
  const projectResourcesMatch = path.match(/^\/api\/projects\/([^/]+)\/resources$/);
  if (projectResourcesMatch && method === "GET") {
    const projectId = projectResourcesMatch[1];
    if (!store.projects.some((item) => item.id === projectId)) return notFound();
    const resources = store.projectResources.filter((item) => item.project_id === projectId);
    return json({ resources, total: resources.length });
  }
  const projectMatch = path.match(/^\/api\/projects\/([^/]+)$/);
  if (projectMatch && method === "GET") {
    const found = store.projects.find((item) => item.id === projectMatch[1]);
    return found ? json(found) : notFound();
  }

  if (path === "/api/agents/mika" && method === "POST") {
    return json({
      id: "agent-mika",
      workspace_id: FIXTURE_WORKSPACE_ID,
      runtime_id: "runtime-local",
      name: "Mika",
      description: "Your workspace Chief of Staff.",
      instructions: "",
      system_key: "mika",
      avatar_url: null,
      visibility: "workspace",
      permission_mode: "public_to",
      invocation_targets: [{ target_type: "workspace", target_id: null }],
      max_concurrent_tasks: 3,
      archived_at: null,
      runtime_mode: "local",
      runtime_config: {},
      custom_args: [],
      onboarding_session: {
        id: "session-onboarding",
        workspace_id: FIXTURE_WORKSPACE_ID,
        agent_id: "agent-mika",
        creator_id: FIXTURE_USER_ID,
        title: "Getting started with Mika",
        status: "active",
        has_unread: false,
        created_at: fixtureNow(),
        updated_at: fixtureNow(),
      },
    });
  }

  const onboardingChat = path.match(/^\/api\/chat\/sessions\/([^/]+)\/onboarding$/);
  if (onboardingChat && method === "POST") {
    return json({
      started: true,
      message_id: "message-onboarding",
      created_at: fixtureNow(),
    });
  }

  if (method === "DELETE") return noContent();
  if (method === "GET" && path.endsWith("/unread-count")) {
    return json({ count: 0 });
  }
  if (method === "GET") return json([]);
  return json({});
}
