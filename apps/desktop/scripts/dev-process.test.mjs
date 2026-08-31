import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import {
  clearDevProcessState,
  devProcessLauncherIsRunning,
  devProcessStatePath,
  devProcessTreeIsRunning,
  parseDevProcessState,
  planDevProcessSignal,
  readDevProcessState,
  writeDevProcessState,
} from "../../../scripts/dev-process.mjs";
import { stopCompleteDev } from "../../../scripts/stop-dev.mjs";

let sandbox;

afterEach(async () => {
  if (sandbox) await rm(sandbox, { recursive: true, force: true });
  sandbox = undefined;
});

describe("complete development process tracking", () => {
  it("stores a checkout-bound process manifest and clears only its owner", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-process-"));
    await writeDevProcessState(sandbox, {
      pid: 4242,
      parentPid: process.pid,
      platform: "darwin",
      startedAt: "2026-08-31T00:00:00.000Z",
    });

    expect(await readDevProcessState(sandbox)).toMatchObject({
      pid: 4242,
      parentPid: process.pid,
      repoRoot: sandbox,
    });
    await expect(
      writeDevProcessState(sandbox, {
        pid: 5252,
        parentPid: process.pid,
      }),
    ).rejects.toMatchObject({ code: "EEXIST" });
    await clearDevProcessState(sandbox, 9999);
    expect(await readFile(devProcessStatePath(sandbox), "utf8")).toContain(
      '"pid": 4242',
    );
    await clearDevProcessState(sandbox, 4242);
    expect(await readDevProcessState(sandbox)).toBeNull();
  });

  it("rejects a manifest copied from a different checkout", () => {
    expect(() =>
      parseDevProcessState(
        JSON.stringify({ pid: 4242, parentPid: 4241, repoRoot: "/other" }),
        "/repo",
      ),
    ).toThrow(/invalid complete development process state/);
  });

  it("targets the isolated POSIX process group and the Windows child tree", () => {
    expect(planDevProcessSignal("darwin", 4242)).toEqual({
      pid: -4242,
      signal: "SIGTERM",
    });
    expect(planDevProcessSignal("linux", 4242, { force: true })).toEqual({
      pid: -4242,
      signal: "SIGKILL",
    });
    expect(planDevProcessSignal("win32", 4242)).toEqual({
      command: "taskkill.exe",
      args: ["/PID", "4242", "/T", "/F"],
    });
  });

  it("checks the whole POSIX group instead of only the backend port", () => {
    const checked = [];
    expect(
      devProcessTreeIsRunning(
        { pid: 4242 },
        {
          platform: "linux",
          killProcess(pid, signal) {
            checked.push({ pid, signal });
          },
        },
      ),
    ).toBe(true);
    expect(checked).toEqual([{ pid: -4242, signal: 0 }]);
  });

  it("requires the recorded launcher to still own a stoppable tree", () => {
    const checked = [];
    expect(
      devProcessLauncherIsRunning(
        { parentPid: 3131 },
        {
          killProcess(pid, signal) {
            checked.push({ pid, signal });
          },
        },
      ),
    ).toBe(true);
    expect(checked).toEqual([{ pid: 3131, signal: 0 }]);
  });

  it.skipIf(process.platform === "win32")(
    "stops a real tracked POSIX process group",
    async () => {
      sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-process-"));
      const child = spawn(
        process.execPath,
        ["-e", "setInterval(() => {}, 1000)"],
        { detached: true, stdio: "ignore" },
      );
      const exited = once(child, "exit");
      await once(child, "spawn");
      try {
        await writeDevProcessState(sandbox, {
          pid: child.pid,
          parentPid: process.pid,
          platform: process.platform,
          startedAt: new Date().toISOString(),
        });

        expect(devProcessTreeIsRunning({ pid: child.pid })).toBe(true);
        await stopCompleteDev({
          repoRoot: sandbox,
          log: { log() {} },
        });
        await exited;
        expect(devProcessTreeIsRunning({ pid: child.pid })).toBe(false);
        expect(await readDevProcessState(sandbox)).toBeNull();
      } finally {
        if (devProcessTreeIsRunning({ pid: child.pid })) {
          process.kill(-child.pid, "SIGKILL");
          await exited;
        }
      }
    },
  );
});
