import { describe, expect, it } from "vitest";
import { weixinInstallationsOptions, weixinKeys } from "./queries";

describe("Weixin query keys", () => {
  it("keeps installations scoped to the workspace", () => {
    expect(weixinKeys.installations("workspace-1")).toEqual([
      "weixin",
      "workspace-1",
      "installations",
    ]);
  });

  it("does not enable an installation query without a workspace", () => {
    expect(weixinInstallationsOptions("").enabled).toBe(false);
    expect(weixinInstallationsOptions("workspace-1").enabled).toBe(true);
  });
});
