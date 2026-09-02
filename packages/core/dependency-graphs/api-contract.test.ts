// @vitest-environment node
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiClient } from "../api/client";

afterEach(() => vi.unstubAllGlobals());

describe("GET /api/issues/:id/dependency-graph contract", () => {
  it("parses the persisted Go node gate fields", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({
        plan: {
          id: "plan-1",
          workspace_id: "ws-1",
          parent_issue_id: "target",
          idempotency_key: "key-1",
          goal: "Ship safely",
          status: "active",
          attention_required: false,
          attention_reason: null,
          created_by_type: "member",
          created_by_id: "member-1",
          created_at: "2026-09-01T00:00:00Z",
          updated_at: "2026-09-01T00:00:00Z",
        },
        nodes: [{
          id: "node-1",
          plan_id: "plan-1",
          workspace_id: "ws-1",
          temp_id: "target",
          issue_id: "target",
          title: "Target",
          description: "",
          acceptance_criteria: [],
          context: {},
          outputs: [],
          executor_type: null,
          executor_id: null,
          candidate_executors: [],
          wave: 1,
          owner_type: null,
          owner_id: null,
          reviewer_type: null,
          reviewer_id: null,
          runtime_id: null,
          model_id: null,
          status: "blocked",
          status_category: "blocked",
          ready: false,
          blocked_by: ["source-1"],
          created_at: "2026-09-01T00:00:00Z",
          updated_at: "2026-09-01T00:00:00Z",
        }],
        edges: [{
          id: "edge-1",
          plan_id: "plan-1",
          workspace_id: "ws-1",
          from_issue_id: "source-1",
          to_issue_id: "target",
          type: "hard",
          reason: "Needs source output",
          consumed_output: "artifact",
          created_at: "2026-09-01T00:00:00Z",
        }],
      }), { status: 200, headers: { "Content-Type": "application/json" } }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const graph = await new ApiClient("https://api.example.test").getDependencyGraph("target");

    expect(graph?.nodes[0]).toMatchObject({
      status_category: "blocked",
      ready: false,
      blocked_by: ["source-1"],
    });
    expect(graph?.edges[0]).toMatchObject({
      from_issue_id: "source-1",
      to_issue_id: "target",
      type: "hard",
    });
    expect(fetchMock).toHaveBeenCalledWith(
      "https://api.example.test/api/issues/target/dependency-graph",
      expect.any(Object),
    );
  });

  it("normalizes the Go no-active-plan response to null", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ plan: null, nodes: [], edges: [] }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    ));

    await expect(
      new ApiClient("https://api.example.test").getDependencyGraph("standalone"),
    ).resolves.toBeNull();
  });
});
