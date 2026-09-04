// @vitest-environment node

import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiClient } from "./client";
import {
  EMPTY_ISSUE_CATEGORY_POLICY,
  EMPTY_LIST_ISSUE_CATEGORY_POLICIES_RESPONSE,
} from "./schemas";

afterEach(() => {
  vi.unstubAllGlobals();
});

const policy = {
  workspace_id: "ws-1",
  category: "in_review",
  default_execution_agent_id: "agent-execution",
  default_reviewer_agent_id: "agent-reviewer",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

describe("ApiClient issue category policies", () => {
  it("lists the workspace policies through the Rust-compatible endpoint", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ policies: [policy] }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const response = await new ApiClient("https://api.example.test").listIssueCategoryPolicies();

    expect(response.policies).toEqual([policy]);
    expect(fetchMock).toHaveBeenCalledWith(
      "https://api.example.test/api/issue-category-policies",
      expect.anything(),
    );
  });

  it("writes a policy with the category path and exact wire field names", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(policy), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const response = await new ApiClient("https://api.example.test").updateIssueCategoryPolicy(
      "in_review",
      {
        default_execution_agent_id: policy.default_execution_agent_id,
        default_reviewer_agent_id: policy.default_reviewer_agent_id,
      },
    );

    expect(response).toEqual(policy);
    expect(fetchMock).toHaveBeenCalledWith(
      "https://api.example.test/api/issue-category-policies/in_review",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({
          default_execution_agent_id: "agent-execution",
          default_reviewer_agent_id: "agent-reviewer",
        }),
      }),
    );
  });

  it("falls back instead of returning malformed policy responses", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ policies: "not-an-array" }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ default_execution_agent_id: 42 }), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    const client = new ApiClient("https://api.example.test");

    await expect(client.listIssueCategoryPolicies()).resolves.toEqual(
      EMPTY_LIST_ISSUE_CATEGORY_POLICIES_RESPONSE,
    );
    await expect(
      client.updateIssueCategoryPolicy("in_progress", {
        default_execution_agent_id: "agent-execution",
      }),
    ).resolves.toEqual(EMPTY_ISSUE_CATEGORY_POLICY);
  });
});
