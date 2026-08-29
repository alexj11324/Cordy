import { afterEach, describe, expect, it } from "vitest";
import { handleFixtureRequest } from "./handler";
import { UI_FIXTURE_COOKIE } from "./mode";
import {
  FIXTURE_AGENT_CODING_ID,
  FIXTURE_AGENT_RESEARCH_ID,
  resetUiFixtureStore,
} from "./store";
import { isUiFixturesEnabled } from "./enabled";

afterEach(() => {
  resetUiFixtureStore();
});

function request(
  partial: Partial<Parameters<typeof handleFixtureRequest>[0]> &
    Pick<Parameters<typeof handleFixtureRequest>[0], "method" | "pathname">,
) {
  return handleFixtureRequest({
    search: new URLSearchParams(),
    cookieHeader: null,
    body: undefined,
    ...partial,
  });
}

describe("isUiFixturesEnabled", () => {
  it("is on only for non-production web-dev", () => {
    expect(
      isUiFixturesEnabled({
        NODE_ENV: "development",
        PATCHBAY_UI_FIXTURES: "1",
      }),
    ).toBe(true);
    expect(
      isUiFixturesEnabled({
        NODE_ENV: "production",
        PATCHBAY_UI_FIXTURES: "1",
      }),
    ).toBe(false);
    expect(isUiFixturesEnabled({ NODE_ENV: "development" })).toBe(false);
  });
});

describe("handleFixtureRequest", () => {
  it("returns an onboarded user and preview workspace for the app", () => {
    const me = request({ method: "GET", pathname: "/api/me" });
    const workspaces = request({ method: "GET", pathname: "/api/workspaces" });
    expect(me.status).toBe(200);
    expect(me.body).toMatchObject({
      id: "user-preview",
      onboarded_at: "2026-01-01T00:00:00Z",
    });
    expect(workspaces.body).toEqual(
      expect.arrayContaining([expect.objectContaining({ slug: "preview" })]),
    );
  });

  it("hides the preview workspace while onboarding", () => {
    const me = request({
      method: "GET",
      pathname: "/api/me",
      cookieHeader: `${UI_FIXTURE_COOKIE}=onboarding`,
    });
    const workspaces = request({
      method: "GET",
      pathname: "/api/workspaces",
      cookieHeader: `${UI_FIXTURE_COOKIE}=onboarding`,
    });
    expect(me.body).toMatchObject({ onboarded_at: null });
    expect(workspaces.body).toEqual([]);
  });

  it("lists fixture issues on the real issues endpoints", () => {
    const listed = request({
      method: "GET",
      pathname: "/api/issues",
      search: new URLSearchParams({ workspace_id: "ws-preview" }),
    });
    const todo = request({
      method: "GET",
      pathname: "/api/issues",
      search: new URLSearchParams({ statuses: "todo" }),
    });
    expect(listed.body).toMatchObject({ total: 6 });
    expect(todo.body).toMatchObject({ total: 2 });
  });

  it("returns table rows for status group keys", () => {
    const rows = request({
      method: "POST",
      pathname: "/api/issues/table/rows",
      body: { group_key: "status:todo" },
    });
    expect(rows.body).toMatchObject({
      total: 2,
      branch_total: 2,
    });
    expect(
      (rows.body as { rows: { issue: { identifier: string } }[] }).rows.map(
        (row) => row.issue.identifier,
      ),
    ).toEqual(["PRE-102", "PRE-103"]);
  });

  it("does not treat child-progress as an issue id", () => {
    const result = request({
      method: "GET",
      pathname: "/api/issues/child-progress",
    });
    expect(result.status).toBe(200);
    expect(result.body).toEqual({ progress: [] });
  });

  it("creates a workspace during onboarding", () => {
    const created = request({
      method: "POST",
      pathname: "/api/workspaces",
      cookieHeader: `${UI_FIXTURE_COOKIE}=onboarding`,
      body: { name: "Acme", slug: "acme" },
    });
    expect(created.status).toBe(201);
    const listed = request({
      method: "GET",
      pathname: "/api/workspaces",
      cookieHeader: `${UI_FIXTURE_COOKIE}=onboarding`,
    });
    expect(listed.body).toEqual(
      expect.arrayContaining([expect.objectContaining({ slug: "acme" })]),
    );
  });

  it("lists fixture agents and a review handoff on PRE-102", () => {
    const agents = request({ method: "GET", pathname: "/api/agents" });
    expect(agents.body).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: "agent-content", name: "Content" }),
        expect.objectContaining({ id: "agent-research", name: "Research" }),
        expect.objectContaining({ id: "agent-coding", name: "Coding" }),
      ]),
    );

    const issue = request({ method: "GET", pathname: "/api/issues/issue-102" });
    expect(issue.body).toMatchObject({
      assignee_type: "agent",
      assignee_id: "agent-coding",
      reviewer_type: "agent",
      reviewer_id: "agent-research",
    });

    const timeline = request({
      method: "GET",
      pathname: "/api/issues/issue-102/timeline",
    });
    expect(timeline.body).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          action: "review_handoff",
          details: expect.objectContaining({
            from_id: "agent-content",
            to_id: "agent-research",
          }),
        }),
        expect.objectContaining({
          action: "review_handoff",
          details: expect.objectContaining({
            from_id: "agent-research",
            to_id: "agent-coding",
          }),
        }),
      ]),
    );

    const runs = request({
      method: "GET",
      pathname: "/api/issues/issue-102/task-runs",
    });
    expect(runs.body).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          agent_id: "agent-coding",
          status: "running",
          handoff_note:
            "Research finished the empty-state audit. Polish the remaining copy.",
        }),
      ]),
    );
  });

  it("rejects clearing a reviewer and entering review without one", () => {
    const cleared = request({
      method: "PATCH",
      pathname: "/api/issues/issue-102",
      body: { reviewer_type: null, reviewer_id: null },
    });
    expect(cleared.status).toBe(400);
    expect(cleared.body).toMatchObject({ code: "reviewer_cannot_clear" });

    const missing = request({
      method: "PATCH",
      pathname: "/api/issues/issue-103",
      body: { status: "in_review" },
    });
    expect(missing.status).toBe(400);
    expect(missing.body).toMatchObject({ code: "review_handoff_required" });
  });

  it("remaps a legacy in-review assignee swap onto the reviewer", () => {
    const updated = request({
      method: "PATCH",
      pathname: "/api/issues/issue-104",
      body: {
        status: "in_review",
        assignee_type: "agent",
        assignee_id: FIXTURE_AGENT_RESEARCH_ID,
      },
    });
    expect(updated.status).toBe(200);
    expect(updated.body).toMatchObject({
      status: "in_review",
      assignee_id: FIXTURE_AGENT_CODING_ID,
      reviewer_id: FIXTURE_AGENT_RESEARCH_ID,
    });
  });

  it("creates a project against the workspace named in the header", () => {
    const created = request({
      method: "POST",
      pathname: "/api/projects",
      workspaceSlug: "preview",
      body: {
        title: "api",
        resources: [
          {
            resource_type: "github_repo",
            resource_ref: { url: "https://github.com/acme/api" },
          },
        ],
      },
    });
    expect(created.status).toBe(201);
    expect(created.body).toMatchObject({
      title: "api",
      workspace_id: "ws-preview",
      resource_count: 1,
    });
    const createdBody = created.body as { id: string };
    const listed = request({
      method: "GET",
      pathname: `/api/projects/${createdBody.id}/resources`,
    });
    expect(listed.status).toBe(200);
    expect(listed.body).toMatchObject({
      total: 1,
      resources: [
        {
          project_id: createdBody.id,
          resource_type: "github_repo",
          resource_ref: { url: "https://github.com/acme/api" },
        },
      ],
    });
  });
});
