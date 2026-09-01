import { describe, expect, it, vi } from "vitest";

import { setApiInstance } from "../api";
import type { ApiClient } from "../api/client";
import type { DependencyGraphResponse } from "../types";
import { dependencyGraphsOptions } from "./queries";

const graph = (id: string) => ({ id }) as unknown as DependencyGraphResponse;

describe("dependencyGraphsOptions", () => {
  it("accepts a single terminal page without treating it as a repeated cursor", async () => {
    const listDependencyGraphs = vi.fn().mockResolvedValueOnce({
      graphs: [graph("only")],
      next_cursor: null,
    });
    setApiInstance({ listDependencyGraphs } as unknown as ApiClient);

    const options = dependencyGraphsOptions("workspace-1");
    if (!options.queryFn)
      throw new Error("dependency graph query function is missing");
    const result = await options.queryFn({} as never);

    expect(result).toEqual([graph("only")]);
    expect(listDependencyGraphs).toHaveBeenCalledOnce();
    expect(listDependencyGraphs).toHaveBeenCalledWith({ limit: 64 });
  });

  it("loads every cursor page while preserving the flat graph result", async () => {
    const listDependencyGraphs = vi
      .fn()
      .mockResolvedValueOnce({
        graphs: [graph("first")],
        next_cursor: "page-2",
      })
      .mockResolvedValueOnce({ graphs: [graph("second")], next_cursor: null });
    setApiInstance({ listDependencyGraphs } as unknown as ApiClient);

    const options = dependencyGraphsOptions("workspace-1", "project-1");
    if (!options.queryFn)
      throw new Error("dependency graph query function is missing");
    const result = await options.queryFn({} as never);

    expect(result).toEqual([graph("first"), graph("second")]);
    expect(listDependencyGraphs).toHaveBeenNthCalledWith(1, {
      projectId: "project-1",
      limit: 64,
    });
    expect(listDependencyGraphs).toHaveBeenNthCalledWith(2, {
      projectId: "project-1",
      limit: 64,
      cursor: "page-2",
    });
  });

  it("rejects a repeated non-terminal cursor", async () => {
    const listDependencyGraphs = vi
      .fn()
      .mockResolvedValueOnce({ graphs: [], next_cursor: "page-2" })
      .mockResolvedValueOnce({ graphs: [], next_cursor: "page-2" });
    setApiInstance({ listDependencyGraphs } as unknown as ApiClient);

    const options = dependencyGraphsOptions("workspace-1");
    if (!options.queryFn)
      throw new Error("dependency graph query function is missing");

    await expect(options.queryFn({} as never)).rejects.toThrow(
      "dependency graph pagination cursor did not advance",
    );
    expect(listDependencyGraphs).toHaveBeenCalledTimes(2);
  });
});
