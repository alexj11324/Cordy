// @vitest-environment node
import { describe, expect, it, vi } from "vitest";
import {
  dependencyGraphKeys,
  dependencyGraphsOptions,
} from "./queries";

const listDependencyGraphs = vi.hoisted(() => vi.fn());
const getDependencyGraph = vi.hoisted(() => vi.fn());

vi.mock("../api", () => ({
  api: { listDependencyGraphs, getDependencyGraph },
  ApiError: class ApiError extends Error {
    status = 500;
  },
}));

describe("dependency graph queries", () => {
  it("flattens cursor pages without dropping the terminal page", async () => {
    listDependencyGraphs
      .mockResolvedValueOnce({ graphs: [{ plan: { id: "plan-1" } }], next_cursor: "next" })
      .mockResolvedValueOnce({ graphs: [{ plan: { id: "plan-2" } }], next_cursor: null });

    const options = dependencyGraphsOptions("ws-1");
    const result = await options.queryFn!({
      queryKey: options.queryKey,
      signal: new AbortController().signal,
    } as never);

    expect(result).toHaveLength(2);
    expect(listDependencyGraphs).toHaveBeenNthCalledWith(
      1,
      { limit: 64 },
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );
    expect(listDependencyGraphs).toHaveBeenNthCalledWith(
      2,
      { limit: 64, cursor: "next" },
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );
  });

  it("fails closed when the server repeats a pagination cursor", async () => {
    listDependencyGraphs
      .mockResolvedValueOnce({ graphs: [], next_cursor: "same" })
      .mockResolvedValueOnce({ graphs: [], next_cursor: "same" });

    const options = dependencyGraphsOptions("ws-1", "project-1");
    await expect(
      options.queryFn!({
        queryKey: options.queryKey,
        signal: new AbortController().signal,
      } as never),
    ).rejects.toThrow("repeated cursor");
  });

  it("keeps list and detail cache entries under one workspace prefix", () => {
    expect(dependencyGraphKeys.list("ws-1")).toEqual([
      "dependency-graphs",
      "ws-1",
      "list",
      null,
    ]);
    expect(dependencyGraphKeys.detail("ws-1", "issue-1")).toEqual([
      "dependency-graphs",
      "ws-1",
      "detail",
      "issue-1",
    ]);
  });
});
