import { describe, expect, it } from "vitest";
import {
  taskProvenanceOptions,
  workProductDetailOptions,
  workProductKeys,
  workProductListInfiniteOptions,
  workProductListOptions,
  unassociatedWorkProductListInfiniteOptions,
} from "./queries";

describe("Work Product query keys", () => {
  it("includes workspace and pagination in every collection key", () => {
    expect(workProductKeys.list("workspace-1", { page: 2, per_page: 10 })).toEqual([
      "work-products",
      "workspace-1",
      "list",
      2,
      10,
    ]);
    expect(workProductKeys.list("workspace-2", { page: 2, per_page: 10 })).not.toEqual(
      workProductKeys.list("workspace-1", { page: 2, per_page: 10 }),
    );
    expect(workProductKeys.unassociated("workspace-1", "", 20)).toEqual([
      "work-products",
      "workspace-1",
      "unassociated",
      "",
      20,
    ]);
  });

  it("disables reads without the workspace/resource identity", () => {
    expect(workProductListOptions(null).enabled).toBe(false);
    expect(workProductDetailOptions("workspace-1", "").enabled).toBe(false);
    expect(unassociatedWorkProductListInfiniteOptions("workspace-1", 20, "", false).enabled).toBe(false);
    expect(taskProvenanceOptions("workspace-1", "").enabled).toBe(false);
    expect(workProductListInfiniteOptions(null).enabled).toBe(false);
    expect(workProductListInfiniteOptions("workspace-1", 64, false).enabled).toBe(false);
  });
});
