import { Readable } from "node:stream";
import { describe, expect, it } from "vitest";

import { handlePreviewRequest } from "./preview-api.mjs";

function request(method, url, body, headers = {}) {
  const stream = Readable.from(body === undefined ? [] : [JSON.stringify(body)]);
  stream.method = method;
  stream.url = url;
  stream.headers = headers;
  return stream;
}

function response() {
  let body = "";
  return {
    statusCode: 200,
    headers: {},
    setHeader(name, value) {
      this.headers[name] = value;
    },
    end(value = "") {
      body = value;
      this.writableEnded = true;
    },
    writableEnded: false,
    json() {
      return body ? JSON.parse(body) : null;
    },
  };
}

async function call(method, url, body, headers) {
  const res = response();
  const handled = await handlePreviewRequest(request(method, url, body, headers), res);
  return { handled, status: res.statusCode, body: res.json() };
}

const baseQuery = {
  scope: { kind: "workspace" },
  filters: {},
  sort: { field: "position", direction: "asc" },
};

describe("local Vite preview API", () => {
  it("answers the shared table contract for actor scopes", async () => {
    const members = await call("POST", "/api/issues/table/rows", {
      query: {
        ...baseQuery,
        scope: { kind: "workspace", assignee_types: ["member"] },
      },
      page: { limit: 50 },
    });
    const agents = await call("POST", "/api/issues/table/rows", {
      query: {
        ...baseQuery,
        scope: { kind: "workspace", assignee_types: ["agent", "squad"] },
      },
      page: { limit: 50 },
    });

    expect(members.handled).toBe(true);
    expect(members.body.total).toBe(4);
    expect(members.body.rows.map(({ issue }) => issue.identifier)).not.toContain(
      "PRE-104",
    );
    expect(agents.body.total).toBe(2);
    expect(agents.body.rows.map(({ issue }) => issue.identifier)).toEqual(
      expect.arrayContaining(["PRE-104", "PRE-105"]),
    );
  });

  it("applies my scopes, assignee filters, date ranges, and priority ordering", async () => {
    const assigned = await call("POST", "/api/issues/table/rows", {
      query: {
        ...baseQuery,
        scope: { kind: "my", relation: "assigned" },
      },
      page: { limit: 50 },
    });
    const noAssignee = await call("POST", "/api/issues/table/rows", {
      query: {
        ...baseQuery,
        filters: { assignees: [], include_no_assignee: true },
      },
      page: { limit: 50 },
    });
    const byPriority = await call("POST", "/api/issues/table/rows", {
      query: {
        ...baseQuery,
        sort: { field: "priority", direction: "asc" },
      },
      page: { limit: 50 },
    });
    const allIssues = await call("GET", "/api/issues?limit=50");
    const newest = allIssues.body.issues.find((issue) => issue.identifier === "PRE-101");
    const recentRange = {
      field: "created_at",
      start: newest.created_at,
      end: new Date(Date.parse(newest.created_at) + 1_000).toISOString(),
    };
    const recent = await call("POST", "/api/issues/table/rows", {
      query: {
        ...baseQuery,
        filters: { date: recentRange },
      },
      page: { limit: 50 },
    });
    const recentFacets = await call("POST", "/api/issues/table/facets", {
      query: {
        ...baseQuery,
        filters: { date: recentRange },
      },
      facets: [{ kind: "status" }],
    });

    expect(assigned.body.rows.map(({ issue }) => issue.identifier)).toEqual([
      "PRE-101",
      "PRE-102",
      "PRE-103",
      "PRE-106",
    ]);
    expect(assigned.body.rows.map(({ issue }) => issue.identifier)).not.toEqual(
      expect.arrayContaining(["PRE-104", "PRE-105"]),
    );
    expect(noAssignee.body).toMatchObject({ total: 0, rows: [] });
    expect(byPriority.body.rows.map(({ issue }) => issue.identifier)).toEqual([
      "PRE-104",
      "PRE-101",
      "PRE-102",
      "PRE-105",
      "PRE-103",
      "PRE-106",
    ]);
    expect(recent.body.rows.map(({ issue }) => issue.identifier)).toEqual(["PRE-101"]);
    expect(recentFacets.body.total).toBe(1);
    expect(recentFacets.body.facets[0].values).toEqual([{ key: "backlog", count: 1 }]);
  });

  it("serves compound swimlane groups and cell rows", async () => {
    const group = { kind: "compound", primary: "assignee", secondary: "status" };
    const groups = await call("POST", "/api/issues/table/groups", {
      query: baseQuery,
      group,
    });
    const agentGroup = groups.body.groups.find(
      (candidate) => candidate.key === "assignee:agent:agent-preview",
    );
    const inProgress = agentGroup.secondary_groups.find(
      (candidate) => candidate.value.status === "in_progress",
    );
    const rows = await call("POST", "/api/issues/table/rows", {
      query: baseQuery,
      group,
      group_key: inProgress.key,
      hierarchy: { enabled: false },
      parent_id: null,
      page: { limit: 50 },
    });
    const categoryGroup = { ...group, secondary: "status_category" };
    const categoryGroups = await call("POST", "/api/issues/table/groups", {
      query: baseQuery,
      group: categoryGroup,
    });
    const categoryCell = categoryGroups.body.groups
      .find((candidate) => candidate.key === agentGroup.key)
      .secondary_groups.find((candidate) => candidate.value.status === "in_progress");

    expect(groups.body.total).toBe(6);
    expect(agentGroup.count).toBe(2);
    expect(agentGroup.secondary_groups).toHaveLength(7);
    expect(inProgress.key).toContain(":status:in_progress");
    expect(inProgress.count).toBe(1);
    expect(rows.body.rows.map(({ issue }) => issue.identifier)).toEqual(["PRE-104"]);
    expect(categoryCell.key).toContain(":status_category:in_progress");
    expect(categoryCell.count).toBe(1);
  });

  it("answers status and active-agent facets from the same filtered query", async () => {
    const result = await call("POST", "/api/issues/table/facets", {
      query: baseQuery,
      facets: [{ kind: "status" }, { kind: "working_agents" }],
      include_total: true,
    });
    const workingAgents = await call("GET", "/api/working-agents");
    const assignedWorkingAgents = await call(
      "GET",
      "/api/working-agents?type=issue&scope=mine&relation=assigned",
    );
    const childWorkingAgents = await call(
      "GET",
      "/api/working-agents?type=issue&parent=00000000-0000-4000-8000-000000000104",
    );
    const autopilotWorkingAgents = await call(
      "GET",
      "/api/working-agents?type=autopilot",
    );

    expect(result.body.total).toBe(6);
    expect(
      result.body.facets.find((facet) => facet.kind === "status").values,
    ).toEqual(
      expect.arrayContaining([
        { key: "todo", count: 2 },
        { key: "in_progress", count: 1 },
      ]),
    );
    expect(
      result.body.facets.find((facet) => facet.kind === "working_agents").values,
    ).toEqual([
      { key: "agent-preview", count: 1 },
      { key: "agent-mika", count: 1 },
    ]);
    expect(workingAgents.body.map((agent) => agent.id)).toEqual([
      "agent-preview",
      "agent-mika",
    ]);
    expect(workingAgents.body.every((agent) => agent.running_task_count > 0)).toBe(true);
    expect(assignedWorkingAgents).toEqual({ handled: true, status: 200, body: [] });
    expect(childWorkingAgents).toEqual({ handled: true, status: 200, body: [] });
    expect(autopilotWorkingAgents.body.map((agent) => agent.id)).toEqual([
      "agent-preview",
      "agent-mika",
    ]);
  });

  it("serves the running task through shared activity endpoints without a transcript", async () => {
    const snapshot = await call("GET", "/api/agent-task-snapshot");
    const issueTasks = await call(
      "GET",
      "/api/issues/00000000-0000-4000-8000-000000000104/task-runs",
    );
    const taskMessages = await call(
      "GET",
      "/api/tasks/task-pre-104/messages",
    );
    const unsupportedTaskWrite = await call(
      "POST",
      "/api/tasks/task-pre-104/messages",
      { content: "not persisted" },
    );

    expect(snapshot.body).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          issue_id: "00000000-0000-4000-8000-000000000104",
          status: "running",
        }),
      ]),
    );
    expect(issueTasks.body).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          issue_id: "00000000-0000-4000-8000-000000000104",
          status: "running",
        }),
      ]),
    );
    expect(issueTasks.body.every((task) => task.issue_id === "00000000-0000-4000-8000-000000000104")).toBe(true);
    expect(taskMessages.handled).toBe(false);
    expect(unsupportedTaskWrite.handled).toBe(false);
  });

  it("returns issue detail data and does not claim unsupported writes succeeded", async () => {
    const detail = await call("GET", "/api/issues/PRE-102");
    const comments = await call("GET", "/api/issues/PRE-102/comments");
    const unsupported = await call("POST", "/api/issues/PRE-102/comments", {
      body: "not persisted",
    });

    expect(detail.body.identifier).toBe("PRE-102");
    expect(comments.handled).toBe(true);
    expect(comments.status).toBe(200);
    expect(comments.body).toEqual([]);
    expect(unsupported.handled).toBe(false);
  });

  it("keeps the sample directory and issue execution log linked", async () => {
    const agents = await call("GET", "/api/agents");
    const issue = await call("GET", "/api/issues/PRE-105");
    const tasks = await call("GET", "/api/issues/PRE-105/task-runs");

    expect(agents.body.map((agent) => agent.name)).toEqual(
      expect.arrayContaining(["Atlas", "Mika", "Nova", "Quill"]),
    );
    expect(agents.body.find((agent) => agent.name === "Mika")).toMatchObject({
      system_key: "mika",
    });
    expect(issue.body).toMatchObject({
      assignee_type: "agent",
      assignee_id: "agent-preview",
      reviewer_type: "agent",
      reviewer_id: "agent-mika",
    });
    expect(tasks.body).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          issue_id: "00000000-0000-4000-8000-000000000105",
          agent_id: "agent-preview",
          status: "completed",
        }),
        expect.objectContaining({
          issue_id: "00000000-0000-4000-8000-000000000105",
          agent_id: "agent-mika",
          status: "running",
        }),
      ]),
    );
    for (const task of tasks.body) {
      expect(task.issue_id).toBe("00000000-0000-4000-8000-000000000105");
      expect(task).not.toHaveProperty("chat_session_id");
      expect(task).not.toHaveProperty("side_chat_parent_task_id");
      expect(task).not.toHaveProperty("side_chat_root_comment_id");
      expect(task).not.toHaveProperty("transcript");
      expect(task).not.toHaveProperty("messages");
    }
  });

  it("serves empty usage arrays for every seeded runtime", async () => {
    const runtimes = await call("GET", "/api/runtimes");

    expect(runtimes.body).toHaveLength(4);
    for (const runtime of runtimes.body) {
      const usage = await call(
        "GET",
        `/api/runtimes/${runtime.id}/usage?days=14&tz=America%2FNew_York`,
      );
      expect(usage).toEqual({ handled: true, status: 200, body: [] });
    }
    const unknown = await call("GET", "/api/runtimes/runtime-unknown/usage");
    expect(unknown).toMatchObject({ handled: true, status: 404 });
  });

  it("serves completed empty model catalogs for every seeded runtime", async () => {
    const runtimes = await call("GET", "/api/runtimes");

    expect(runtimes.body).toHaveLength(4);
    for (const runtime of runtimes.body) {
      const models = await call("POST", `/api/runtimes/${runtime.id}/models`);
      expect(models).toMatchObject({
        handled: true,
        status: 200,
        body: {
          id: `preview-models-${runtime.id}`,
          runtime_id: runtime.id,
          status: "completed",
          models: [],
          supported: true,
        },
      });
      expect(models.body.created_at).toEqual(expect.any(String));
      expect(models.body.updated_at).toEqual(expect.any(String));
    }

    const unknown = await call("POST", "/api/runtimes/runtime-unknown/models");
    expect(unknown).toMatchObject({ handled: true, status: 404 });
  });

  it("serves completed empty capability responses for every seeded runtime", async () => {
    const runtimes = await call("GET", "/api/runtimes");

    for (const runtime of runtimes.body) {
      const capabilities = await call(
        "POST",
        `/api/runtimes/${runtime.id}/local-skills`,
      );
      expect(capabilities).toMatchObject({
        handled: true,
        status: 200,
        body: {
          id: `preview-local-skills-${runtime.id}`,
          runtime_id: runtime.id,
          status: "completed",
          skills: [],
          supported: true,
          mcp_servers: [],
          mcp_supported: false,
        },
      });
      expect(capabilities.body.created_at).toEqual(expect.any(String));
      expect(capabilities.body.updated_at).toEqual(expect.any(String));
    }

    const unknown = await call(
      "POST",
      "/api/runtimes/runtime-unknown/local-skills",
    );
    expect(unknown).toMatchObject({ handled: true, status: 404 });
  });

  it("serves read-only automation samples with runs and an explicit write boundary", async () => {
    const list = await call("GET", "/api/autopilots");
    const detail = await call("GET", "/api/autopilots/autopilot-pr-review");
    const runs = await call("GET", "/api/autopilots/autopilot-pr-review/runs");
    const tasks = await call("GET", "/api/agent-task-snapshot");

    expect(list.body.total).toBe(3);
    expect(list.body.can_create).toBe(false);
    expect(detail.body.autopilot).toMatchObject({
      title: "PR review handoff",
      can_write: false,
      trigger_kinds: ["schedule"],
      issue_title_template: "PR review follow-up",
    });
    expect(detail.body.triggers).toHaveLength(1);
    expect(detail.body.triggers[0].kind).toBe("schedule");
    expect(runs.body.runs.map((run) => run.id)).toEqual([
      "run-pr-review-queued",
      "run-pr-review-current",
      "run-pr-review-completed",
    ]);
    expect(list.body.autopilots.find(
      (autopilot) => autopilot.id === "autopilot-pr-review",
    )).toMatchObject({
      last_run_at: runs.body.runs[0].triggered_at,
      last_run_status: runs.body.runs[0].status,
    });
    expect(detail.body.autopilot).toMatchObject({
      last_run_at: runs.body.runs[0].triggered_at,
      last_run_status: runs.body.runs[0].status,
      execution_mode: "create_issue",
    });
    const earliestRunAt = Math.min(
      ...runs.body.runs.map((run) => Date.parse(run.triggered_at)),
    );
    expect(Date.parse(list.body.autopilots.find(
      (autopilot) => autopilot.id === "autopilot-pr-review",
    ).created_at)).toBeLessThan(earliestRunAt);
    for (const autopilot of list.body.autopilots) {
      const autopilotRuns = await call(
        "GET",
        `/api/autopilots/${autopilot.id}/runs`,
      );
      const oldestRun = Math.min(
        ...autopilotRuns.body.runs.map((run) => Date.parse(run.triggered_at)),
      );
      expect(Date.parse(autopilot.created_at)).toBeLessThan(oldestRun);
      expect(autopilot).toMatchObject({
        last_run_at: autopilotRuns.body.runs[0].triggered_at,
        last_run_status: autopilotRuns.body.runs[0].status,
      });
    }
    expect(runs.body.runs).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ status: "running", task_id: "task-pre-105-review" }),
        expect.objectContaining({ status: "completed", task_id: "task-pre-106" }),
        expect.objectContaining({
          id: "run-pr-review-queued",
          autopilot_id: "autopilot-pr-review",
          task_id: "task-pre-102",
        }),
      ]),
    );
    const completedRun = runs.body.runs.find((run) => run.id === "run-pr-review-completed");
    const completedTask = tasks.body.find((task) => task.id === "task-pre-106");
    const currentRun = runs.body.runs.find((run) => run.id === "run-pr-review-current");
    const currentTask = tasks.body.find((task) => task.id === "task-pre-105-review");
    expect(Date.parse(currentRun.triggered_at)).toBeLessThanOrEqual(
      Date.parse(currentTask.created_at),
    );
    expect(Date.parse(completedRun.triggered_at)).toBeLessThanOrEqual(
      Date.parse(completedTask.created_at),
    );
    expect(Date.parse(completedRun.completed_at)).toBeGreaterThanOrEqual(
      Date.parse(completedTask.completed_at),
    );

    const ciWatchDetail = await call("GET", "/api/autopilots/autopilot-ci-watch");
    const ciWatchTrigger = ciWatchDetail.body.triggers[0];
    const ciWatchRuns = await call("GET", "/api/autopilots/autopilot-ci-watch/runs");
    const ciWatchRun = ciWatchRuns.body.runs[0];
    const ciWatchTask = tasks.body.find((task) => task.id === ciWatchRun.task_id);
    expect(ciWatchDetail.body.autopilot).toMatchObject({
      execution_mode: "create_issue",
      issue_title_template: "CI watch follow-up",
    });
    expect(ciWatchDetail.body.autopilot.next_run_at).toBe(ciWatchTrigger.next_run_at);
    expect(Date.parse(ciWatchRun.triggered_at)).toBeLessThan(
      Date.parse(ciWatchTask.created_at),
    );
    expect(Date.parse(ciWatchRun.triggered_at)).toBeLessThan(
      Date.parse(ciWatchTask.dispatched_at),
    );
    expect(Date.parse(ciWatchTrigger.next_run_at)).toBeGreaterThan(Date.now() - 60_000);
    expect(Number(new Intl.DateTimeFormat("en-US", {
      timeZone: ciWatchTrigger.timezone,
      minute: "numeric",
    }).format(new Date(ciWatchTrigger.next_run_at))) % 15).toBe(0);
    expect(Number(new Intl.DateTimeFormat("en-US", {
      timeZone: detail.body.triggers[0].timezone,
      minute: "numeric",
    }).format(new Date(detail.body.triggers[0].next_run_at))) % 30).toBe(0);
    const cronPreview = await call(
      "GET",
      "/api/autopilots/cron-preview?expr=%2A%2F15+%2A+%2A+%2A+%2A&tz=America%2FNew_York",
    );
    expect(cronPreview.body.next_runs).toHaveLength(3);
    expect(cronPreview.body.next_runs).toEqual(
      [...cronPreview.body.next_runs].sort(),
    );
    for (const nextRun of cronPreview.body.next_runs) {
      expect(Number(new Intl.DateTimeFormat("en-US", {
        timeZone: "America/New_York",
        minute: "numeric",
      }).format(new Date(nextRun))) % 15).toBe(0);
    }

    const tasksById = new Map(tasks.body.map((task) => [task.id, task]));
    const issues = await call("GET", "/api/issues?limit=50");
    const issuesById = new Map(issues.body.issues.map((issue) => [issue.id, issue]));
    const autopilotsById = new Map(
      list.body.autopilots.map((autopilot) => [autopilot.id, autopilot]),
    );
    for (const run of runs.body.runs) {
      const task = tasksById.get(run.task_id);
      const autopilot = autopilotsById.get(run.autopilot_id);
      expect(task).toBeDefined();
      expect(autopilot).toBeDefined();
      expect(task).toMatchObject({
        issue_id: expect.any(String),
        agent_id: autopilot.assignee_id,
        autopilot_run_id: run.id,
        kind: "autopilot",
        trigger_summary: autopilot.title,
      });
      expect(task.issue_id).not.toBe("");
    }
    for (const task of tasks.body) {
      const issue = issuesById.get(task.issue_id);
      expect(issue).toBeDefined();
      expect(Date.parse(issue.created_at)).toBeLessThanOrEqual(
        Date.parse(task.created_at),
      );
    }
    for (const autopilot of list.body.autopilots) {
      const autopilotRuns = await call(
        "GET",
        `/api/autopilots/${autopilot.id}/runs`,
      );
      for (const run of autopilotRuns.body.runs) {
        if (!run.issue_id) continue;
        const issue = issuesById.get(run.issue_id);
        expect(issue).toBeDefined();
        expect(Date.parse(issue.created_at)).toBeLessThanOrEqual(
          Date.parse(run.triggered_at),
        );
      }
    }
    for (const task of tasks.body) {
      expect(task).not.toHaveProperty("chat_session_id");
      expect(task).not.toHaveProperty("transcript");
      expect(task).not.toHaveProperty("messages");
    }

    const unsupportedTrigger = await call(
      "POST",
      "/api/autopilots/autopilot-pr-review/trigger",
    );
    expect(unsupportedTrigger.handled).toBe(false);
  });

  it("does not expose preview chat or transcript entrypoints", async () => {
    const page = await call(
      "GET",
      "/api/chat/sessions/preview/messages/page?limit=25",
    );
    const messages = await call(
      "GET",
      "/api/chat/sessions/preview/messages",
    );
    const pending = await call(
      "GET",
      "/api/chat/sessions/preview/pending-task",
    );

    expect(page.handled).toBe(false);
    expect(messages.handled).toBe(false);
    expect(pending.handled).toBe(false);
  });

  it("keeps shared issue list and optional detail reads on the JSON boundary", async () => {
    const list = await call("GET", "/api/issues?limit=50");
    const window = await call("POST", "/api/issues/query", {
      ids: ["00000000-0000-4000-8000-000000000104"],
    });
    const quickActions = await call("GET", "/api/quick-actions");
    const assigneeFrequency = await call("GET", "/api/assignee-frequency");

    expect(list).toMatchObject({ handled: true, status: 200 });
    expect(list.body.total).toBe(6);
    expect(window.body.issues.map((issue) => issue.identifier)).toEqual(["PRE-104"]);
    expect(quickActions.body).toEqual({ quick_actions: [], total: 0 });
    expect(assigneeFrequency.body).toEqual([]);
  });

  it("resolves optional shared reads without enabling preview mutations", async () => {
    const agents = await call("GET", "/api/agents");
    const [lark, slack, dingtalk, groupRoutes, wecom, telegram, weixin, profiles, plugins, workspaceMcp] =
      await Promise.all([
        call("GET", "/api/workspaces/ws-preview/lark/installations"),
        call("GET", "/api/workspaces/ws-preview/slack/installations"),
        call("GET", "/api/workspaces/ws-preview/dingtalk/installations"),
        call("GET", "/api/workspaces/ws-preview/dingtalk/group-routes"),
        call("GET", "/api/workspaces/ws-preview/wecom/installations"),
        call("GET", "/api/workspaces/ws-preview/telegram/installations"),
        call("GET", "/api/workspaces/ws-preview/weixin/installations"),
        call("GET", "/api/workspaces/ws-preview/runtime-profiles"),
        call("GET", "/api/workspaces/ws-preview/plugins"),
        call("GET", "/api/workspaces/ws-preview/mcp-servers"),
      ]);

    for (const result of [lark, slack, dingtalk, wecom, telegram, weixin]) {
      expect(result).toMatchObject({
        handled: true,
        status: 200,
        body: { installations: [], configured: false, install_supported: false },
      });
    }
    expect(groupRoutes).toMatchObject({
      handled: true,
      status: 200,
      body: { routes: [] },
    });
    expect(profiles).toMatchObject({
      handled: true,
      status: 200,
      body: { runtime_profiles: [] },
    });
    expect(plugins).toMatchObject({
      handled: true,
      status: 200,
      body: { plugins: [] },
    });
    expect(workspaceMcp).toEqual({ handled: true, status: 200, body: [] });
    for (const agent of agents.body) {
      await expect(
        call("GET", `/api/agents/${agent.id}/mcp-servers`),
      ).resolves.toEqual({ handled: true, status: 200, body: [] });
    }
    expect(
      await call("GET", "/api/agents/agent-unknown/mcp-servers"),
    ).toMatchObject({ handled: true, status: 404 });
  });

  it("localizes preview-owned fixture copy from the browser language", async () => {
    const issues = await call(
      "GET",
      "/api/issues?limit=1",
      undefined,
      { "accept-language": "zh-CN,zh;q=0.9" },
    );
    const autopilots = await call(
      "GET",
      "/api/autopilots",
      undefined,
      { "accept-language": "ja-JP,ja;q=0.9" },
    );
    const tasks = await call(
      "GET",
      "/api/issues/PRE-105/task-runs",
      undefined,
      { "accept-language": "ja-JP,ja;q=0.9" },
    );
    const preferredEnglish = await call(
      "GET",
      "/api/issues?limit=1",
      undefined,
      { "accept-language": "en-US,en;q=0.9,ja;q=0.8" },
    );
    const runtimes = await call(
      "GET",
      "/api/runtimes",
      undefined,
      { "accept-language": "ja-JP,ja;q=0.9" },
    );

    expect(issues.body.issues[0]).toMatchObject({
      title: "优化工作区引导",
      description: "让首次使用流程更容易理解。",
    });
    expect(preferredEnglish.body.issues[0]).toMatchObject({
      title: "Refine workspace onboarding",
      description: "Make the first-run path easier to understand.",
    });
    expect(autopilots.body.autopilots[0]).toMatchObject({
      title: "PR 確認の引き継ぎ",
      description: "完了した実装作業を対応可能な確認担当者に渡します。",
    });
    expect(tasks.body).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          id: "task-pre-105-review",
          trigger_summary: "PR 確認の引き継ぎ",
        }),
      ]),
    );
    expect(runtimes.body.find((runtime) => runtime.id === "runtime-preview")).toMatchObject({
      device_info: "ブラウザプレビューランタイム",
    });
    expect(runtimes.body.find((runtime) => runtime.id === "runtime-quill")).toMatchObject({
      device_info: "ランタイムはオフラインです",
    });
    expect(autopilots.body.autopilots[0]).toMatchObject({
      issue_title_template: "PR 確認のフォローアップ",
    });
  });
});
