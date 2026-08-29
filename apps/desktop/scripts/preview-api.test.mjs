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
    expect(members.body.total).toBe(5);
    expect(members.body.rows.map(({ issue }) => issue.identifier)).not.toContain(
      "PRE-104",
    );
    expect(agents.body.total).toBe(1);
    expect(agents.body.rows[0].issue.identifier).toBe("PRE-104");
  });

  it("answers status and active-agent facets from the same filtered query", async () => {
    const result = await call("POST", "/api/issues/table/facets", {
      query: baseQuery,
      facets: [{ kind: "status" }, { kind: "working_agents" }],
      include_total: true,
    });

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
    ).toEqual([{ key: "agent-preview", count: 1 }]);
  });

  it("serves the running task through both shared activity endpoints", async () => {
    const snapshot = await call("GET", "/api/agent-task-snapshot");
    const issueTasks = await call(
      "GET",
      "/api/issues/00000000-0000-4000-8000-000000000104/task-runs",
    );
    const taskMessages = await call(
      "GET",
      "/api/tasks/00000000-0000-4000-8000-000000000201/messages",
    );
    const unsupportedTaskWrite = await call(
      "POST",
      "/api/tasks/00000000-0000-4000-8000-000000000201/messages",
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
    expect(issueTasks.body).toEqual(snapshot.body);
    expect(taskMessages).toEqual({ handled: true, status: 200, body: [] });
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
    const tasks = await call("GET", "/api/issues/PRE-105/task-runs");

    expect(agents.body.map((agent) => agent.name)).toEqual(
      expect.arrayContaining(["Atlas", "Mika", "Nova", "Quill"]),
    );
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
  });

  it("serves read-only automation samples with runs and an explicit write boundary", async () => {
    const list = await call("GET", "/api/autopilots");
    const detail = await call("GET", "/api/autopilots/autopilot-pr-review");
    const runs = await call("GET", "/api/autopilots/autopilot-pr-review/runs");

    expect(list.body.total).toBe(3);
    expect(detail.body.autopilot).toMatchObject({
      title: "PR review handoff",
      can_write: false,
    });
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

    const unsupportedTrigger = await call(
      "POST",
      "/api/autopilots/autopilot-pr-review/trigger",
    );
    expect(unsupportedTrigger.handled).toBe(false);
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
    const [lark, slack, dingtalk, groupRoutes, wecom, telegram, weixin, profiles, plugins] =
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

    expect(issues.body.issues[0]).toMatchObject({
      title: "优化工作区引导",
      description: "让首次使用流程更容易理解。",
    });
    expect(autopilots.body.autopilots[0]).toMatchObject({
      title: "PR 確認の引き継ぎ",
      description: "完了した実装作業を対応可能な確認担当者に渡します。",
    });
  });
});
