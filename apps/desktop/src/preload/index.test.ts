import { afterEach, expect, it, vi } from "vitest";

const { exposeInMainWorld } = vi.hoisted(() => ({
  exposeInMainWorld: vi.fn(),
}));

vi.mock("electron", () => ({
  contextBridge: { exposeInMainWorld },
  ipcRenderer: {
    sendSync: (channel: string) =>
      channel === "app:get-info"
        ? { version: "test", os: "macos" }
        : { ok: false, error: { message: "Test runtime" } },
  },
}));
vi.mock("@electron-toolkit/preload", () => ({ electronAPI: {} }));

afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetModules();
  vi.clearAllMocks();
});

it("exposes the native host identity used by the macOS glass shell", async () => {
  vi.stubGlobal("process", { ...process, contextIsolated: true });
  await import("./index");

  const bridge = exposeInMainWorld.mock.calls.find(
    ([name]) => name === "desktopAPI",
  )?.[1];
  expect(bridge).toMatchObject({
    host: "electron",
    appInfo: { os: "macos" },
  });
});
