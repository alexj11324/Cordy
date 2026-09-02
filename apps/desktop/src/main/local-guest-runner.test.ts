// @vitest-environment node

import { EventEmitter } from "node:events";
import { mkdtemp, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { realpath } from "node:fs/promises";
import { PassThrough } from "node:stream";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LocalGuestMode } from "../shared/local-guest";

type IpcHandler = (...args: unknown[]) => unknown;

const ctx = vi.hoisted(() => ({
  userDataPath: "",
  ipcHandlers: new Map<string, (...args: unknown[]) => unknown>(),
  windowsBySender: new Map<unknown, unknown>(),
  spawn: vi.fn(),
  verifyBundledCli: vi.fn(async () => true),
  childEnvironment: { PATH: "/usr/bin", HOME: "/guest-runtime-home" },
}));

vi.mock("electron", () => ({
  app: {
    getPath: vi.fn(() => ctx.userDataPath),
    getAppPath: vi.fn(() => "/app"),
  },
  dialog: { showOpenDialog: vi.fn() },
  BrowserWindow: {
    fromWebContents: (sender: unknown) =>
      ctx.windowsBySender.get(sender) ?? null,
  },
  ipcMain: {
    handle: (channel: string, handler: IpcHandler) => {
      ctx.ipcHandlers.set(channel, handler);
    },
    removeHandler: (channel: string) => {
      ctx.ipcHandlers.delete(channel);
    },
  },
}));

vi.mock("node:child_process", () => ({ spawn: ctx.spawn }));

vi.mock("./local-guest-runtime", () => ({
  bundledCliPath: () => "/app/resources/bin/patchbay",
  verifyBundledCli: ctx.verifyBundledCli,
  localGuestChildEnvironment: async () => ({ ...ctx.childEnvironment }),
}));

import { loadLocalGuestRunHistory, localGuestRunHistoryPath } from "./local-guest-history-storage";
import { setupLocalGuestRunner } from "./local-guest-runner";
import { LocalWorkspaceGrants } from "./local-guest-workspace";

/** Stands in for the spawned `patchbay daemon run-local` process. */
class FakeChild extends EventEmitter {
  readonly stdout = new PassThrough();
  readonly stderr = new PassThrough();
  readonly stdin = new PassThrough();
  killed = false;
  readonly signals: string[] = [];
  stdinPayload = "";

  constructor() {
    super();
    this.stdin.on("data", (chunk: Buffer) => {
      this.stdinPayload += chunk.toString();
    });
  }

  kill(signal: string): boolean {
    this.signals.push(signal);
    this.killed = true;
    // A real child dies and its streams close; the runner finalises on close.
    queueMicrotask(() => this.close());
    return true;
  }

  emitLine(value: unknown): void {
    this.stdout.write(`${JSON.stringify(value)}\n`);
  }

  emitRaw(line: string): void {
    this.stdout.write(`${line}\n`);
  }

  close(): void {
    this.stdout.end();
    this.stderr.end();
    this.emit("close", 0);
  }
}

const temporaryDirectories: string[] = [];

async function createTemporaryDirectory(prefix: string): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), prefix));
  temporaryDirectories.push(directory);
  return realpath(directory);
}

const mainSender = { id: "main" };
const otherSender = { id: "other" };
const sentEvents: Array<{ channel: string; payload: unknown }> = [];
const mainWindow = {
  isDestroyed: () => false,
  webContents: {
    isDestroyed: () => false,
    send: (channel: string, payload: unknown) => {
      sentEvents.push({ channel, payload });
    },
  },
};

let mode: LocalGuestMode = "guest";
let grants: LocalWorkspaceGrants;
let workspace: string;
let controller: ReturnType<typeof setupLocalGuestRunner>;

function invoke(channel: string, sender: unknown, value?: unknown) {
  const handler = ctx.ipcHandlers.get(channel);
  if (!handler) throw new Error(`no handler registered for ${channel}`);
  return handler({ sender }, value);
}

function runRequest(overrides: Record<string, unknown> = {}) {
  return {
    workingDirectory: workspace,
    prompt: "inspect the workspace",
    timeoutMs: 60_000,
    ...overrides,
  };
}

/**
 * Waits until the runner has finalised the active run. The run slot is only
 * released after its history write settles, so a follow-up start would
 * otherwise race and report "busy".
 */
async function waitForRunToSettle(): Promise<void> {
  await vi.waitFor(() => {
    const last = sentEvents.at(-1)?.payload as { event?: string } | undefined;
    expect(last?.event).toBe("result");
  });
  const historyPath = localGuestRunHistoryPath(ctx.userDataPath);
  await vi.waitFor(async () => {
    const loaded = await loadLocalGuestRunHistory(historyPath);
    expect(loaded.ok && loaded.history.runs.length > 0).toBe(true);
  });
}

/** Runs `start` and hands back the child the runner spawned. */
async function startRun(
  request: Record<string, unknown> = runRequest(),
): Promise<{ result: { ok: boolean; runId?: string }; child: FakeChild }> {
  const child = new FakeChild();
  ctx.spawn.mockReturnValueOnce(child);
  const result = (await invoke("guest-run:start", mainSender, request)) as {
    ok: boolean;
    runId?: string;
  };
  return { result, child };
}

beforeEach(async () => {
  ctx.ipcHandlers.clear();
  ctx.spawn.mockReset();
  ctx.verifyBundledCli.mockReset();
  ctx.verifyBundledCli.mockResolvedValue(true);
  ctx.windowsBySender.clear();
  ctx.windowsBySender.set(mainSender, mainWindow);
  sentEvents.length = 0;
  mode = "guest";
  ctx.userDataPath = await createTemporaryDirectory("patchbay-guest-runner-");
  workspace = await createTemporaryDirectory("patchbay-guest-workspace-");
  grants = new LocalWorkspaceGrants();
  await grants.grant(workspace);
  controller = setupLocalGuestRunner(
    () => mainWindow as never,
    () => mode,
    grants,
  );
});

afterEach(async () => {
  controller.cancel();
  // The runner persists history fire-and-forget after reporting the terminal
  // event, so a background mkdir/write/rename inside the temp directory can
  // still be in flight when teardown runs. Retry once on ENOTEMPTY instead of
  // failing the run on a decided outcome (CI flake: rmdir local-guest).
  for (const directory of temporaryDirectories.splice(0)) {
    try {
      await rm(directory, { recursive: true, force: true });
    } catch (err) {
      if ((err as NodeJS.ErrnoException)?.code !== "ENOTEMPTY") throw err;
      await new Promise((resolve) => setTimeout(resolve, 50));
      await rm(directory, { recursive: true, force: true });
    }
  }
});

const GUEST_RUN_CHANNELS = [
  "guest-run:start",
  "guest-run:cancel",
  "guest-run:history",
  "guest-run:clear-history",
] as const;

describe("local Guest runner isolation", () => {
  it("refuses every run channel outside Guest mode", async () => {
    // The load-bearing property. If someone later reuses this runner for a
    // signed-in cloud user, or forgets the mode check on a new channel, this
    // is what fails.
    for (const nonGuest of ["cloud", "undecided"] as const) {
      mode = nonGuest;
      for (const channel of GUEST_RUN_CHANNELS) {
        const result = (await invoke(channel, mainSender, {
          runId: "anything",
        })) as { ok: boolean; reason?: string };
        expect(result.ok).toBe(false);
        if (channel !== "guest-run:clear-history") {
          expect(result.reason).toBe("guest_required");
        }
      }
    }
    expect(ctx.spawn).not.toHaveBeenCalled();
  });

  it("refuses every run channel from a window that is not the main window", async () => {
    for (const channel of GUEST_RUN_CHANNELS) {
      const result = (await invoke(channel, otherSender, {
        runId: "anything",
      })) as { ok: boolean; reason?: string };
      expect(result.ok).toBe(false);
    }
    expect(ctx.spawn).not.toHaveBeenCalled();
  });

  it("refuses a working directory the user never chose", async () => {
    const unchosen = await createTemporaryDirectory("patchbay-unchosen-");

    const { result } = await startRun(
      runRequest({ workingDirectory: unchosen }),
    );

    expect(result).toEqual({ ok: false, reason: "invalid_directory" });
    expect(ctx.spawn).not.toHaveBeenCalled();
  });

  it("refuses to run when the bundled runner fails its checksum", async () => {
    ctx.verifyBundledCli.mockResolvedValue(false);

    const { result } = await startRun();

    expect(result).toEqual({ ok: false, reason: "cli_unavailable" });
    expect(ctx.spawn).not.toHaveBeenCalled();
  });

  it("verifies the bundled runner before every spawn, not once at startup", async () => {
    const first = await startRun();
    first.child.close();
    await waitForRunToSettle();

    ctx.verifyBundledCli.mockResolvedValue(false);
    const second = await startRun();

    expect(second.result).toEqual({ ok: false, reason: "cli_unavailable" });
    expect(ctx.verifyBundledCli).toHaveBeenCalledTimes(2);
  });

  it("rejects a malformed request without touching the filesystem", async () => {
    for (const request of [
      null,
      { workingDirectory: workspace, prompt: "   ", timeoutMs: 60_000 },
      { workingDirectory: workspace, prompt: "x", timeoutMs: 10 },
      { workingDirectory: workspace, prompt: "x", timeoutMs: 60_000, token: "s" },
      { workingDirectory: "relative/path", prompt: "x", timeoutMs: 60_000 },
    ]) {
      const result = (await invoke("guest-run:start", mainSender, request)) as {
        ok: boolean;
        reason?: string;
      };
      expect(result.ok).toBe(false);
    }
    expect(ctx.spawn).not.toHaveBeenCalled();
  });

  it("spawns the local runner with no cloud configuration", async () => {
    await startRun();

    expect(ctx.spawn).toHaveBeenCalledWith(
      "/app/resources/bin/patchbay",
      ["daemon", "run-local"],
      expect.objectContaining({
        cwd: workspace,
        env: ctx.childEnvironment,
      }),
    );
    const [, , options] = ctx.spawn.mock.calls[0] as [
      string,
      string[],
      { env: NodeJS.ProcessEnv },
    ];
    expect(
      Object.keys(options.env).filter((key) => key.startsWith("PATCHBAY")),
    ).toEqual([]);
  });
});

describe("local Guest run stream protocol", () => {
  it("sends the request as one JSON line and closes stdin", async () => {
    const { child } = await startRun();

    await vi.waitFor(() => expect(child.stdinPayload).not.toBe(""));
    expect(JSON.parse(child.stdinPayload)).toEqual({
      working_directory: workspace,
      prompt: "inspect the workspace",
      timeout_ms: 60_000,
    });
  });

  it("forwards started, message and result events to the renderer", async () => {
    const { result, child } = await startRun();
    expect(result.ok).toBe(true);

    child.emitLine({ event: "started" });
    child.emitLine({ event: "message", text: "Inspecting" });
    child.emitLine({
      event: "result",
      status: "completed",
      text: "1 files",
      duration_ms: 12,
    });
    child.close();

    await vi.waitFor(() =>
      expect(
        sentEvents.some(
          (entry) =>
            (entry.payload as { event?: string }).event === "result",
        ),
      ).toBe(true),
    );
    const events = sentEvents
      .filter((entry) => entry.channel === "guest-run:event")
      .map((entry) => entry.payload as { runId: string; event: string });
    expect(events.map((event) => event.event)).toEqual([
      "started",
      "message",
      "result",
    ]);
    expect(new Set(events.map((event) => event.runId))).toEqual(
      new Set([result.runId]),
    );
  });

  it("treats an unparseable line as a protocol failure and stops the child", async () => {
    const { child } = await startRun();

    child.emitRaw("this is not JSON");

    await vi.waitFor(() => expect(child.signals).toContain("SIGTERM"));
    await vi.waitFor(() => {
      const last = sentEvents.at(-1)?.payload as {
        event?: string;
        status?: string;
      };
      expect(last?.event).toBe("result");
    });
    const last = sentEvents.at(-1)?.payload as { status?: string };
    expect(last.status).toBe("cancelled");
  });

  it("rejects a second run while one is in flight", async () => {
    await startRun();

    const second = (await invoke(
      "guest-run:start",
      mainSender,
      runRequest(),
    )) as { ok: boolean; reason?: string };

    expect(second).toEqual({ ok: false, reason: "busy" });
    expect(ctx.spawn).toHaveBeenCalledTimes(1);
  });
});

describe("local Guest run cancellation and timeout", () => {
  it("cancels the active run on request and reports it as cancelled", async () => {
    const { result, child } = await startRun();

    const cancelled = await invoke("guest-run:cancel", mainSender, {
      runId: result.runId,
    });

    expect(cancelled).toEqual({ ok: true });
    expect(child.signals).toContain("SIGTERM");
    await vi.waitFor(() => {
      const last = sentEvents.at(-1)?.payload as { status?: string };
      expect(last?.status).toBe("cancelled");
    });
  });

  it("refuses to cancel a run id it does not own", async () => {
    const { result } = await startRun();

    expect(
      invoke("guest-run:cancel", mainSender, { runId: `${result.runId}-nope` }),
    ).toEqual({ ok: false, reason: "not_found" });
    expect(invoke("guest-run:cancel", mainSender, { runId: 42 })).toEqual({
      ok: false,
      reason: "not_found",
    });
  });

  it("stops a run that outlives its timeout", async () => {
    const { child } = await startRun(runRequest({ timeoutMs: 1_000 }));

    await vi.waitFor(
      () => {
        const last = sentEvents.at(-1)?.payload as { status?: string };
        expect(last?.status).toBe("timeout");
      },
      { timeout: 5_000 },
    );
    expect(child.signals).toContain("SIGTERM");
  });

  it("reports a runner that exits without a result rather than hanging", async () => {
    const { child } = await startRun();

    child.stderr.write("panic: local runner exploded");
    child.close();

    await vi.waitFor(() => {
      const last = sentEvents.at(-1)?.payload as {
        status?: string;
        error?: string;
      };
      expect(last?.status).toBe("failed");
      expect(last?.error).toContain("exploded");
    });
    // The terminal event fires before the history write settles; teardown
    // must wait for the write or its rm races the background persist
    // (ENOTEMPTY flake on local-guest).
    const historyPath = localGuestRunHistoryPath(ctx.userDataPath);
    await vi.waitFor(async () => {
      const loaded = await loadLocalGuestRunHistory(historyPath);
      expect(loaded.ok && loaded.history.runs.length).toBe(1);
    });
  });
});

describe("local Guest run history", () => {
  it("records a completed run against the resolved directory with 0600 permissions", async () => {
    const { child } = await startRun();
    child.emitLine({ event: "result", status: "completed", text: "1 files" });
    child.close();

    const historyPath = localGuestRunHistoryPath(ctx.userDataPath);
    await vi.waitFor(async () => {
      const loaded = await loadLocalGuestRunHistory(historyPath);
      expect(loaded.ok && loaded.history.runs.length).toBe(1);
    });

    const loaded = await loadLocalGuestRunHistory(historyPath);
    expect(loaded.ok).toBe(true);
    if (!loaded.ok) return;
    expect(loaded.history.lastDirectory).toBe(workspace);
    expect(loaded.history.runs[0]).toMatchObject({
      workingDirectory: workspace,
      status: "completed",
      prompt: "inspect the workspace",
    });
    // No cloud identifiers may appear in local-only state.
    expect(JSON.stringify(loaded.history)).not.toMatch(/token|workspaceId/i);

    const fileStat = await stat(historyPath);
    expect(fileStat.mode & 0o777).toBe(0o600);
    const directoryStat = await stat(join(ctx.userDataPath, "local-guest"));
    expect(directoryStat.mode & 0o777).toBe(0o700);
  });

  it("hands the history back only in Guest mode and clears it on request", async () => {
    const { child } = await startRun();
    child.emitLine({ event: "result", status: "completed" });
    child.close();

    const historyPath = localGuestRunHistoryPath(ctx.userDataPath);
    await vi.waitFor(async () => {
      const loaded = await loadLocalGuestRunHistory(historyPath);
      expect(loaded.ok && loaded.history.runs.length).toBe(1);
    });

    const read = (await invoke("guest-run:history", mainSender)) as {
      ok: boolean;
      history?: { runs: unknown[] };
    };
    expect(read.ok).toBe(true);
    expect(read.history?.runs).toHaveLength(1);

    await controller.clear();

    const cleared = (await invoke("guest-run:history", mainSender)) as {
      ok: boolean;
      history?: { runs: unknown[] };
    };
    expect(cleared.history?.runs).toEqual([]);
  });

  it("re-grants only the directory it previously persisted", async () => {
    // A fresh runner over an existing history must make the prefilled
    // directory runnable again — and nothing else.
    const { child } = await startRun();
    child.emitLine({ event: "result", status: "completed" });
    child.close();
    await vi.waitFor(async () => {
      const loaded = await loadLocalGuestRunHistory(
        localGuestRunHistoryPath(ctx.userDataPath),
      );
      expect(loaded.ok && loaded.history.runs.length).toBe(1);
    });

    const freshGrants = new LocalWorkspaceGrants();
    ctx.ipcHandlers.clear();
    setupLocalGuestRunner(() => mainWindow as never, () => mode, freshGrants);

    const unchosen = await createTemporaryDirectory("patchbay-unchosen-");
    const rejected = await startRun(
      runRequest({ workingDirectory: unchosen }),
    );
    expect(rejected.result).toEqual({ ok: false, reason: "invalid_directory" });

    const accepted = await startRun();
    expect(accepted.result.ok).toBe(true);
    accepted.child.close();
  });
});
