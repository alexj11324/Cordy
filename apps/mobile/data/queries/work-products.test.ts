import { describe, expect, it, vi } from "vitest";
import {
  workProductDetailOptions,
  workProductKeys,
  workProductListOptions,
} from "./work-products";

// The mobile Vitest lane is deliberately native-free. Query key factories do
// not need the fetch client, so keep React Native out of this pure data test.
vi.mock("@/data/api", () => ({ api: {} }));

describe("mobile Work Product queries", () => {
  it("keeps list and detail caches isolated by workspace and product", () => {
    expect(workProductKeys.list("ws-1")).toEqual([
      "work-products",
      "ws-1",
      "list",
    ]);
    expect(workProductKeys.list("ws-1")).not.toEqual(workProductKeys.list("ws-2"));
    expect(workProductKeys.detail("ws-1", "wp-1")).not.toEqual(
      workProductKeys.detail("ws-1", "wp-2"),
    );
  });

  it("does not enable requests before workspace or product identity is ready", () => {
    expect(workProductListOptions(null).enabled).toBe(false);
    expect(workProductDetailOptions("ws-1", "").enabled).toBe(false);
    expect(workProductDetailOptions(null, "wp-1").enabled).toBe(false);
  });
});
