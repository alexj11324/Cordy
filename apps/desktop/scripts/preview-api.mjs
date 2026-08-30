const WORKSPACE_ID = "ws-preview";
const PREVIEW_USER_ID = "user-preview";
const PREVIEW_MEMBER_ID = "member-preview";
const PREVIEW_AGENT_ID = "agent-preview";

const STATUS_CATEGORIES = [
  "backlog",
  "todo",
  "in_progress",
  "in_review",
  "done",
  "blocked",
  "cancelled",
];

const NOW = "2026-01-01T00:00:00Z";

const PREVIEW_WORKSPACE = {
  id: WORKSPACE_ID,
  name: "Preview",
  slug: "preview",
  description: null,
  context: null,
  settings: {},
  repos: [],
  issue_prefix: "PRE",
  avatar_url: null,
  created_at: NOW,
  updated_at: NOW,
};

const PREVIEW_MEMBER = {
  id: PREVIEW_MEMBER_ID,
  workspace_id: WORKSPACE_ID,
  user_id: PREVIEW_USER_ID,
  role: "owner",
  created_at: NOW,
  name: "Alex",
  email: "preview@local",
  avatar_url: null,
};

const PREVIEW_ISSUES = [
  previewIssue("101", "backlog", "Refine workspace onboarding", "Make the first-run path easier to understand.", "high"),
  previewIssue("102", "todo", "Polish issue board empty states", "Keep the board useful before real work arrives.", "medium"),
  previewIssue("103", "todo", "Add keyboard shortcuts", "Expose the common actions without extra chrome.", "low"),
  previewIssue("104", "in_progress", "Add real-time status indicator", "Show when an agent is actively working on an issue.", "urgent", "agent"),
  previewIssue("105", "in_review", "Check responsive sidebar", "Make the workspace navigation feel balanced at every width.", "medium"),
  previewIssue("106", "done", "Split web and API dev commands", "Let visual work start without the full local stack.", "none"),
];

const PREVIEW_AGENT = {
  id: PREVIEW_AGENT_ID,
  name: "Coding",
  avatar_url: null,
  running_task_count: 1,
  issue_ids: [issueId("104")],
};

const PREVIEW_AGENT_TASK = {
  id: "00000000-0000-4000-8000-000000000201",
  agent_id: PREVIEW_AGENT_ID,
  runtime_id: "runtime-preview",
  issue_id: issueId("104"),
  status: "running",
  priority: 0,
  dispatched_at: NOW,
  started_at: NOW,
  completed_at: null,
  result: null,
  error: null,
  created_at: NOW,
};

const PREVIEW_DIRECTORY_AGENT = {
  id: PREVIEW_AGENT_ID,
  workspace_id: WORKSPACE_ID,
  runtime_id: "runtime-preview",
  runtime_bound: false,
  name: "Coding",
  description: "Preview agent",
  instructions: "",
  avatar_url: null,
  runtime_mode: "local",
  runtime_config: {},
  custom_args: [],
  visibility: "workspace",
  permission_mode: "public_to",
  invocation_targets: [{ target_type: "workspace", target_id: null }],
  status: "working",
  max_concurrent_tasks: 1,
  model: "preview",
  owner_id: PREVIEW_USER_ID,
  skills: [],
  created_at: NOW,
  updated_at: NOW,
  archived_at: null,
  archived_by: null,
};

const preferences = new Map();

function issueId(number) {
  return `00000000-0000-4000-8000-000000000${number}`;
}

function previewIssue(number, status, title, description, priority, assignee = "member") {
  const id = issueId(number);
  const isAgent = assignee === "agent";
  return {
    id,
    workspace_id: WORKSPACE_ID,
    number: Number(number),
    identifier: `PRE-${number}`,
    title,
    description,
    status,
    status_category: status,
    priority,
    assignee_type: isAgent ? "agent" : "member",
    assignee_id: isAgent ? PREVIEW_AGENT_ID : PREVIEW_USER_ID,
    creator_type: "member",
    creator_id: PREVIEW_USER_ID,
    parent_issue_id: null,
    project_id: null,
    position: Number(number),
    stage: null,
    start_date: null,
    due_date: null,
    metadata: {},
    properties: {},
    labels: [],
    created_at: NOW,
    updated_at: NOW,
  };
}

function json(res, body, status = 200) {
  res.statusCode = status;
  res.setHeader("Content-Type", "application/json; charset=utf-8");
  res.end(JSON.stringify(body));
  return true;
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    let body = "";
    req.setEncoding("utf8");
    req.on("data", (chunk) => {
      body += chunk;
      if (body.length > 1_000_000) {
        reject(new Error("preview request body is too large"));
        req.destroy();
      }
    });
    req.on("end", () => {
      if (!body) {
        resolve({});
        return;
      }
      try {
        resolve(JSON.parse(body));
      } catch {
        reject(new Error("preview request body is not valid JSON"));
      }
    });
    req.on("error", reject);
  });
}

function categoryOf(issue) {
  return issue.status_category ?? issue.status;
}

function actorMatches(issue, actors, type) {
  if (!Array.isArray(actors) || actors.length === 0) return true;
  return actors.some(
    (actor) => actor?.type === issue[`${type}_type`] && actor?.id === issue[`${type}_id`],
  );
}

function matchesIssue(issue, query = {}, { ignoreStatus = false, ignoreWorking = false } = {}) {
  const scope = query.scope ?? {};
  if (Array.isArray(scope.assignee_types) && !scope.assignee_types.includes(issue.assignee_type)) {
    return false;
  }
  if (scope.kind === "project" && scope.project_id !== issue.project_id) {
    return false;
  }
  if (scope.kind === "assignee" && !actorMatches(issue, [scope.actor], "assignee")) {
    return false;
  }
  if (scope.kind === "creator" && !actorMatches(issue, [scope.actor], "creator")) {
    return false;
  }

  const filters = query.filters ?? {};
  if (!ignoreStatus && Array.isArray(filters.statuses) && filters.statuses.length > 0) {
    if (!filters.statuses.some((status) => status === issue.status || status === categoryOf(issue))) {
      return false;
    }
  }
  if (Array.isArray(filters.priorities) && filters.priorities.length > 0 && !filters.priorities.includes(issue.priority)) {
    return false;
  }
  if (!actorMatches(issue, filters.assignees, "assignee")) {
    if (!(filters.include_no_assignee && issue.assignee_id === null)) return false;
  }
  if (!actorMatches(issue, filters.creators, "creator")) return false;
  if (Array.isArray(filters.project_ids) && filters.project_ids.length > 0 && !filters.project_ids.includes(issue.project_id)) {
    if (!(filters.include_no_project && issue.project_id === null)) return false;
  }
  if (Array.isArray(filters.label_ids) && filters.label_ids.length > 0) {
    const labels = new Set((issue.labels ?? []).map((label) => label.id));
    if (!filters.label_ids.some((labelId) => labels.has(labelId))) return false;
  }
  if (!ignoreWorking && Array.isArray(filters.working_issue_ids)) {
    if (!filters.working_issue_ids.includes(issue.id)) return false;
  }
  if (query.search) {
    const term = String(query.search).trim().toLowerCase();
    if (term && !`${issue.identifier} ${issue.title} ${issue.description ?? ""}`.toLowerCase().includes(term)) {
      return false;
    }
  }
  return true;
}

function filteredIssues(query, options) {
  return PREVIEW_ISSUES.filter((issue) => matchesIssue(issue, query, options));
}

function sortIssues(issues, query = {}) {
  const field = query.sort?.field ?? "position";
  const direction = query.sort?.direction === "desc" ? -1 : 1;
  return [...issues].sort((a, b) => {
    const left = field === "title" ? a.title : field === "priority" ? a.priority : a[field] ?? "";
    const right = field === "title" ? b.title : field === "priority" ? b.priority : b[field] ?? "";
    return String(left).localeCompare(String(right), undefined, { numeric: true }) * direction;
  });
}

function facetValues(kind, issues) {
  const counts = new Map();
  const add = (key) => counts.set(key, (counts.get(key) ?? 0) + 1);
  for (const issue of issues) {
    if (kind === "status") add(issue.status);
    else if (kind === "priority") add(issue.priority);
    else if (kind === "assignee") add(`${issue.assignee_type}:${issue.assignee_id}`);
    else if (kind === "creator") add(`${issue.creator_type}:${issue.creator_id}`);
    else if (kind === "project") add(issue.project_id ?? "__none__");
    else if (kind === "label") for (const label of issue.labels ?? []) add(label.id);
  }
  return [...counts].map(([key, count]) => ({ key, count }));
}

function tableFacets(body) {
  const requested = Array.isArray(body.facets) ? body.facets : [];
  const facets = requested.map((request) => {
    if (request.kind === "working_agents") {
      const issues = filteredIssues(body.query, { ignoreWorking: true });
      const runningIds = new Set(PREVIEW_AGENT.issue_ids);
      return {
        kind: request.kind,
        values: issues.filter((issue) => runningIds.has(issue.id)).length > 0
          ? [{ key: PREVIEW_AGENT_ID, count: issues.filter((issue) => runningIds.has(issue.id)).length }]
          : [],
      };
    }
    const issues = request.kind === "status"
      ? filteredIssues(body.query, { ignoreStatus: true })
      : filteredIssues(body.query);
    return {
      kind: request.kind,
      ...(request.kind === "property" ? { property_id: request.property_id } : {}),
      values: request.kind === "property" ? [] : facetValues(request.kind, issues),
    };
  });
  return {
    query_fingerprint: JSON.stringify(body.query ?? {}),
    total: filteredIssues(body.query).length,
    facets,
  };
}

function groupKeyForIssue(issue, group) {
  if (group?.kind === "assignee") return issue.assignee_id ? `assignee:${issue.assignee_type}:${issue.assignee_id}` : "assignee:none";
  if (group?.kind === "project") return issue.project_id ? `project:${issue.project_id}` : "project:none";
  return `status:${categoryOf(issue)}`;
}

function groupValueForIssue(issue, group) {
  if (group?.kind === "assignee") {
    return { kind: "assignee", actor: issue.assignee_id ? { type: issue.assignee_type, id: issue.assignee_id } : null };
  }
  if (group?.kind === "project") return { kind: "project", project_id: issue.project_id };
  return { kind: "status", status: categoryOf(issue) };
}

function tableGroups(body) {
  const issues = filteredIssues(body.query);
  const grouped = new Map();
  for (const issue of issues) {
    const key = groupKeyForIssue(issue, body.group);
    const current = grouped.get(key);
    if (current) current.count += 1;
    else grouped.set(key, { key, value: groupValueForIssue(issue, body.group), count: 1 });
  }
  return {
    query_fingerprint: JSON.stringify(body.query ?? {}),
    total: issues.length,
    groups: [...grouped.values()],
    next_cursor: null,
  };
}

function tableRows(body) {
  let issues = filteredIssues(body.query);
  const groupKey = body.group_key;
  if (groupKey?.startsWith("status:")) {
    const category = groupKey.slice("status:".length);
    issues = issues.filter((issue) => categoryOf(issue) === category);
  } else if (groupKey?.startsWith("assignee:")) {
    const expected = groupKey.slice("assignee:".length);
    issues = issues.filter(
      (issue) =>
        `${issue.assignee_type}:${issue.assignee_id}` === expected ||
        (expected === "none" && issue.assignee_id === null),
    );
  } else if (groupKey?.startsWith("project:")) {
    const expected = groupKey.slice("project:".length);
    issues = issues.filter((issue) => (issue.project_id ?? "none") === expected);
  }
  issues = sortIssues(issues, body.query);
  const limit = Number(body.page?.limit) || 50;
  const offset = body.page?.cursor ? Number(body.page.cursor) || 0 : 0;
  const page = issues.slice(offset, offset + limit);
  return {
    query_fingerprint: JSON.stringify(body.query ?? {}),
    group_key: groupKey ?? null,
    parent_id: body.parent_id ?? null,
    total: issues.length,
    rows: page.map((issue) => ({ issue, direct_child_count: 0 })),
    branch_total: issues.length,
    next_cursor: offset + limit < issues.length ? String(offset + limit) : null,
  };
}

function preferenceKey(url) {
  return `${url.searchParams.get("scope_type") ?? "workspace"}:${url.searchParams.get("scope_id") ?? ""}`;
}

function findPreviewIssue(value) {
  const id = decodeURIComponent(value);
  return PREVIEW_ISSUES.find(
    (issue) => issue.id === id || issue.identifier === id,
  );
}

export async function handlePreviewRequest(req, res) {
  const url = new URL(req.url ?? "/", "http://127.0.0.1");
  const method = req.method ?? "GET";
  const path = url.pathname;

  if (method === "GET" && path === "/api/workspaces") return json(res, [PREVIEW_WORKSPACE]);
  if (method === "GET" && path === `/api/workspaces/${WORKSPACE_ID}/members`) return json(res, [PREVIEW_MEMBER]);
  if (method === "GET" && path === "/api/agents") return json(res, [PREVIEW_DIRECTORY_AGENT]);
  if (method === "GET" && path === `/api/agents/${PREVIEW_AGENT_ID}`) return json(res, PREVIEW_DIRECTORY_AGENT);
  if (method === "GET" && path === "/api/squads") return json(res, []);
  if (method === "GET" && path === "/api/projects") return json(res, { projects: [], total: 0 });
  if (method === "GET" && path === "/api/properties") return json(res, { properties: [], total: 0 });
  if (method === "GET" && path === "/api/labels") return json(res, { labels: [], total: 0 });
  if (method === "GET" && path === "/api/pins") return json(res, []);
  if (method === "GET" && path === "/api/issue-views") return json(res, []);
  if (method === "GET" && path === "/api/issue-view-preferences") {
    const key = preferenceKey(url);
    return json(res, preferences.get(key) ?? {
      scope_type: url.searchParams.get("scope_type") ?? "workspace",
      scope_id: url.searchParams.get("scope_id"),
      prefs: { hidden: [], order: [] },
      updated_at: NOW,
    });
  }
  if (method === "PUT" && path === "/api/issue-view-preferences") {
    const body = await readBody(req);
    const key = `${body.scope_type ?? "workspace"}:${body.scope_id ?? ""}`;
    const value = {
      scope_type: body.scope_type ?? "workspace",
      scope_id: body.scope_id ?? null,
      prefs: body.prefs ?? { hidden: [], order: [] },
      updated_at: NOW,
    };
    preferences.set(key, value);
    return json(res, value);
  }
  if (method === "GET" && path === "/api/issue-statuses") {
    return json(res, {
      statuses: STATUS_CATEGORIES.map((category, index) => ({
        id: `status-${category}`,
        workspace_id: WORKSPACE_ID,
        key: category,
        name: category,
        description: "",
        category,
        color: "#6b7280",
        is_system: true,
        position: index,
        archived_at: null,
        created_at: NOW,
        updated_at: NOW,
      })),
      categories: STATUS_CATEGORIES,
      total: STATUS_CATEGORIES.length,
    });
  }
  if (method === "GET" && path === "/api/working-agents") return json(res, [PREVIEW_AGENT]);
  if (method === "GET" && path === "/api/agent-task-snapshot") return json(res, [PREVIEW_AGENT_TASK]);
  if (method === "GET" && path === "/api/issues/child-progress") return json(res, { progress: [] });
  const taskMessagesResource = /^\/api\/tasks\/([^/]+)\/messages$/.exec(path);
  if (method === "GET" && taskMessagesResource) {
    return decodeURIComponent(taskMessagesResource[1]) === PREVIEW_AGENT_TASK.id
      ? json(res, [])
      : json(res, { error: "Preview task not found" }, 404);
  }
  const commentsResource = /^\/api\/issues\/([^/]+)\/comments$/.exec(path);
  if (method === "GET" && commentsResource) {
    return findPreviewIssue(commentsResource[1])
      ? json(res, [])
      : json(res, { error: "Preview issue not found" }, 404);
  }
  const issueResource = /^\/api\/issues\/([^/]+)\/(timeline|subscribers|attachments|labels|task-runs|pull-requests|children)$/.exec(path);
  if (method === "GET" && issueResource) {
    if (!findPreviewIssue(issueResource[1])) {
      return json(res, { error: "Preview issue not found" }, 404);
    }
    switch (issueResource[2]) {
      case "timeline":
      case "subscribers":
      case "attachments":
        return json(res, []);
      case "task-runs":
        return json(res, issueResource[1] === PREVIEW_AGENT_TASK.issue_id ? [PREVIEW_AGENT_TASK] : []);
      case "labels":
        return json(res, { labels: [] });
      case "pull-requests":
        return json(res, { pull_requests: [] });
      case "children":
        return json(res, { issues: [] });
    }
  }
  if (method === "GET" && path.startsWith("/api/issues/")) {
    const issue = findPreviewIssue(path.slice("/api/issues/".length));
    return issue ? json(res, issue) : json(res, { error: "Preview issue not found" }, 404);
  }
  if (method === "POST" && path === "/api/issues/table/facets") return json(res, tableFacets(await readBody(req)));
  if (method === "POST" && path === "/api/issues/table/groups") return json(res, tableGroups(await readBody(req)));
  if (method === "POST" && path === "/api/issues/table/rows") return json(res, tableRows(await readBody(req)));

  return false;
}

/**
 * The browser host's local API is deliberately an HTTP boundary, not a second
 * React page. It supplies the same response contracts that the shared issue
 * surface consumes, so Vite HMR exercises the production renderer. It is only
 * installed by vite.web.config.mjs; Electron continues to use its configured
 * real backend. Unsupported writes fall through to Vite and remain visible as
 * unavailable rather than being reported as successful mutations.
 */
export function previewApiPlugin() {
  return {
    name: "patchbay-local-preview-api",
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        if (!req.url?.startsWith("/api/")) {
          next();
          return;
        }
        try {
          const handled = await handlePreviewRequest(req, res);
          if (!handled && !res.writableEnded) next();
        } catch (error) {
          if (res.writableEnded) return;
          json(res, {
            error: error instanceof Error ? error.message : "Preview API request failed",
          }, 500);
        }
      });
    },
  };
}
