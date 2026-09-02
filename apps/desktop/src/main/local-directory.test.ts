// @vitest-environment node

import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type IpcHandler = (...args: unknown[]) => unknown;

const ctx = vi.hoisted(() => ({
  ipcHandlers: new Map<string, (...args: unknown[]) => unknown>(),
  showOpenDialog: vi.fn(),
}));

vi.mock("electron", () => ({
  app: { getPath: vi.fn(() => "/userdata") },
  dialog: { showOpenDialog: ctx.showOpenDialog },
  BrowserWindow: { fromWebContents: () => ({ id: 1 }) },
  ipcMain: {
    handle: (channel: string, handler: IpcHandler) => {
      ctx.ipcHandlers.set(channel, handler);
    },
  },
}));

import { setupLocalDirectory, validateLocalDirectory } from "./local-directory";

const temporaryDirectories: string[] = [];

async function createTemporaryDirectory(): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), "patchbay-picker-"));
  temporaryDirectories.push(directory);
  return directory;
}

function invoke(channel: string, value?: unknown) {
  const handler = ctx.ipcHandlers.get(channel);
  if (!handler) throw new Error(`no handler registered for ${channel}`);
  return handler({ sender: {} }, value);
}

beforeEach(() => {
  ctx.ipcHandlers.clear();
  ctx.showOpenDialog.mockReset();
});

afterEach(async () => {
  await Promise.all(
    temporaryDirectories
      .splice(0)
      .map((directory) => rm(directory, { recursive: true, force: true })),
  );
});

describe("validateLocalDirectory", () => {
  it("rejects a relative path outright", async () => {
    await expect(validateLocalDirectory("relative/path")).resolves.toEqual({
      ok: false,
      reason: "not_absolute",
    });
    await expect(validateLocalDirectory("")).resolves.toEqual({
      ok: false,
      reason: "not_absolute",
    });
  });

  it("rejects a missing path and a file that is not a directory", async () => {
    const directory = await createTemporaryDirectory();
    const filePath = join(directory, "note.txt");
    await writeFile(filePath, "hello");

    await expect(
      validateLocalDirectory(join(directory, "missing")),
    ).resolves.toEqual({ ok: false, reason: "not_found" });
    await expect(validateLocalDirectory(filePath)).resolves.toEqual({
      ok: false,
      reason: "not_a_directory",
    });
  });

  it("accepts a readable, writable directory", async () => {
    const directory = await createTemporaryDirectory();

    await expect(validateLocalDirectory(directory)).resolves.toMatchObject({
      ok: true,
    });
  });
});

describe("directory picker consent", () => {
  it("records the directory the user actually chose", async () => {
    // The picker is the only source of a workspace grant, so this callback is
    // the point where consent enters the main process.
    const directory = await createTemporaryDirectory();
    const chosen: string[] = [];
    ctx.showOpenDialog.mockResolvedValue({
      canceled: false,
      filePaths: [directory],
    });
    setupLocalDirectory(() => null, (path) => {
      chosen.push(path);
    });

    await expect(invoke("local-directory:pick")).resolves.toMatchObject({
      ok: true,
      path: directory,
    });
    expect(chosen).toEqual([directory]);
  });

  it("records nothing when the user cancels", async () => {
    const chosen: string[] = [];
    ctx.showOpenDialog.mockResolvedValue({ canceled: true, filePaths: [] });
    setupLocalDirectory(() => null, (path) => {
      chosen.push(path);
    });

    await expect(invoke("local-directory:pick")).resolves.toEqual({
      ok: false,
      reason: "cancelled",
    });
    expect(chosen).toEqual([]);
  });

  it("records nothing when validating a path the renderer names", async () => {
    // Validation is a UI affordance, not consent. A renderer that only calls
    // validate must not end up with a runnable directory.
    const directory = await createTemporaryDirectory();
    const chosen: string[] = [];
    setupLocalDirectory(() => null, (path) => {
      chosen.push(path);
    });

    await expect(
      invoke("local-directory:validate", directory),
    ).resolves.toMatchObject({ ok: true });
    expect(chosen).toEqual([]);
  });
});
