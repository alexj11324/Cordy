import { Readable } from "node:stream";
import { describe, expect, it } from "vitest";

import { handlePreviewRequest } from "./preview-api.mjs";

function request(method, url, body) {
  const stream = Readable.from(body === undefined ? [] : [JSON.stringify(body)]);
  stream.method = method;
  stream.url = url;
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

async function call(method, url, body) {
  const res = response();
  const handled = await handlePreviewRequest(request(method, url, body), res);
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

    expect(snapshot.body).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          issue_id: "00000000-0000-4000-8000-000000000104",
          status: "running",
        }),
      ]),
    );
    expect(issueTasks.body).toEqual(snapshot.body);
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
});
