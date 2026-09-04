import { describe, expect, it, vi, afterEach } from "vitest";
import { pickDirectories } from "./local-directory";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("pickDirectories", () => {
  it("reports unsupported when no desktop bridge exists", async () => {
    vi.stubGlobal("window", {});
    await expect(pickDirectories()).resolves.toEqual({
      ok: false,
      reason: "unsupported",
    });
  });

  it("reports unsupported when the preload only has the singular picker", async () => {
    vi.stubGlobal("window", {
      desktopAPI: { pickDirectory: vi.fn() },
    });
    await expect(pickDirectories()).resolves.toEqual({
      ok: false,
      reason: "unsupported",
    });
  });

  it("forwards the preload result", async () => {
    const folders = [{ path: "/repo/api", basename: "api" }];
    const nativePicker = vi.fn(async () => ({ ok: true, folders }));
    vi.stubGlobal("window", { desktopAPI: { pickDirectories: nativePicker } });
    await expect(pickDirectories("/repo")).resolves.toEqual({
      ok: true,
      folders,
    });
    expect(nativePicker).toHaveBeenCalledWith("/repo");
  });
});
