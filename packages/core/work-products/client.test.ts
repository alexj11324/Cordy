import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiClient } from "../api/client";

afterEach(() => {
  vi.unstubAllGlobals();
});

function jsonResponse(value: unknown): Response {
  return new Response(JSON.stringify(value), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

const product = {
  id: "product-1",
  workspace_id: "workspace-1",
  kind: "pull_request",
  provider: "github",
  external_identity: "acme/repo#42",
  external_url: "https://github.com/acme/repo/pull/42",
  provider_record_type: "pull_request",
  provider_record_id: "record-42",
  created_at: "2026-09-02T00:00:00Z",
  updated_at: "2026-09-02T00:01:00Z",
};

const relation = {
  id: "relation-1",
  workspace_id: "workspace-1",
  work_product_id: "product-1",
  issue_id: "issue/1",
  task_id: "task-1",
  run_id: "run-1",
  relation_key: "task:task-1:run:run-1:product:product-1",
  relation_source: "manual_explicit",
  attached_by_type: "user",
  attached_by_id: "user-1",
  attached_at: "2026-09-02T00:02:00Z",
  close_intent: false,
  detached_at: null,
  detached_by_type: null,
  detached_by_id: null,
  detached_task_id: null,
  detached_run_id: null,
};

const provenance = {
  task_id: "task-1",
  workspace_id: "workspace-1",
  run_id: "run-1",
  repo_identity: "acme/repo",
  execution_workspace: "/workspaces/task-1",
  head_branch: "feature/work-product",
  head_sha: "0123456789abcdef0123456789abcdef01234567",
  head_state: "attached",
  started_at: "2026-09-02T00:00:00Z",
  finished_at: "2026-09-02T00:03:00Z",
  discovery_status: "matched",
  discovery_lease_id: "lease-1",
  discovery_match_count: 1,
  discovery_reason: "exact head SHA",
  discovery_work_product_id: "product-1",
  discovery_at: "2026-09-02T00:04:00Z",
  updated_at: "2026-09-02T00:04:00Z",
};

describe("ApiClient Work Product / provenance endpoints", () => {
  it("keeps pagination and workspace scope in the read URLs", async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(
      jsonResponse({ products: [product], page: 2, per_page: 10, has_more: true }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      new ApiClient("https://api.example.test").listWorkProducts({ page: 2, per_page: 10 }),
    ).resolves.toMatchObject({ products: [product], page: 2, has_more: true });
    expect(fetchMock.mock.calls[0]?.[0]).toBe(
      "https://api.example.test/api/work-products?page=2&per_page=10",
    );
  });

  it("uses canonical catalog and explicit attachment routes", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(product))
      .mockResolvedValueOnce(
        jsonResponse({ work_products: [product], next_page: null }),
      )
      .mockResolvedValueOnce(jsonResponse({ work_product: product, relation }));
    vi.stubGlobal("fetch", fetchMock);
    const client = new ApiClient("https://api.example.test");

    await expect(client.getWorkProduct("product/1")).resolves.toEqual(product);
    await expect(
      client.listUnassociatedWorkProducts({ page: 1, per_page: 20 }),
    ).resolves.toMatchObject({ work_products: [product] });
    await expect(
      client.attachExistingWorkProduct("issue/1", {
        work_product_id: "product-1",
        close_intent: true,
      }),
    ).resolves.toMatchObject({ relation });

    expect(fetchMock.mock.calls[0]?.[0]).toBe(
      "https://api.example.test/api/work-products/product%2F1",
    );
    expect(fetchMock.mock.calls[1]?.[0]).toBe(
      "https://api.example.test/api/work-products/unassociated?page=1&per_page=20",
    );
    expect(JSON.parse(String(fetchMock.mock.calls[2]?.[1]?.body))).toEqual({
      work_product_id: "product-1",
      close_intent: true,
    });
  });

  it("normalizes Go nullable wrapper fixtures for provenance", async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(
      jsonResponse({
        provenance: [
          {
            ...provenance,
            run_id: { String: "run-1", Valid: true },
            head_sha: { String: provenance.head_sha, Valid: true },
            finished_at: { Time: provenance.finished_at, Valid: true },
            discovery_reason: { Valid: false },
          },
        ],
        page: 1,
        per_page: 64,
        has_more: false,
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      new ApiClient("https://api.example.test").listWorkspaceProvenance(),
    ).resolves.toMatchObject({
      provenance: [{ run_id: "run-1", head_sha: provenance.head_sha, discovery_reason: null }],
    });
  });
});
