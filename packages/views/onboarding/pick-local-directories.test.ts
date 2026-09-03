import { describe, expect, it, vi, afterEach } from "vitest";
import { pickLocalProjectFolders } from "./pick-local-directories";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("pickLocalProjectFolders", () => {
  it("reports unsupported when no desktop bridge exists", async () => {
    vi.stubGlobal("window", {});
    await expect(pickLocalProjectFolders()).resolves.toEqual({
      ok: false,
      reason: "unsupported",
    });
  });

  it("reports unsupported when the preload only has the singular picker", async () => {
    vi.stubGlobal("window", {
      desktopAPI: { pickDirectory: vi.fn() },
    });
    await expect(pickLocalProjectFolders()).resolves.toEqual({
      ok: false,
      reason: "unsupported",
    });
  });

  it("forwards the preload result", async () => {
    const folders = [
      { path: "/repo/api", basename: "api", originUrl: null },
    ];
    const pickDirectories = vi.fn(async () => ({ ok: true, folders }));
    vi.stubGlobal("window", { desktopAPI: { pickDirectories } });
    await expect(pickLocalProjectFolders("/repo")).resolves.toEqual({
      ok: true,
      folders,
    });
    expect(pickDirectories).toHaveBeenCalledWith("/repo");
  });
});
