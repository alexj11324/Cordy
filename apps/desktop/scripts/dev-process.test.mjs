import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import {
  acquireDevLifecycleLock,
  clearDevProcessState,
  devProcessLauncherIsRunning,
  devProcessStatePath,
  devProcessTreeIsRunning,
  inspectDevProcessIdentity,
  parseDevProcessState,
  planDevProcessSignal,
  readProcessStartToken,
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
      processStartToken: "darwin:child-start",
      parentStartToken: "darwin:parent-start",
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
        platform: "darwin",
        processStartToken: "darwin:other-child-start",
        parentStartToken: "darwin:parent-start",
      }),
    ).rejects.toMatchObject({ code: "EEXIST" });
    await clearDevProcessState(sandbox, 9999);
    expect(await readFile(devProcessStatePath(sandbox), "utf8")).toContain(
      '"pid": 4242',
    );
    await clearDevProcessState(sandbox, 4242);
    expect(await readDevProcessState(sandbox)).toBeNull();
  });

  it("serializes launch and removal with a checkout lifecycle lock", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-process-"));
    const lockPath = join(sandbox, "git-state", "lifecycle.lock");
    const resolveLockPath = () => lockPath;
    await mkdir(join(sandbox, "git-state"));

    const release = await acquireDevLifecycleLock(sandbox, {
      resolveLockPath,
    });
    await expect(
      acquireDevLifecycleLock(sandbox, { resolveLockPath }),
    ).rejects.toThrow(/development lifecycle is busy/);
    await release();

    const releaseAgain = await acquireDevLifecycleLock(sandbox, {
      resolveLockPath,
    });
    await releaseAgain();
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

  it("reads platform process start identities through POSIX and Windows tools", () => {
    const calls = [];
    const spawnCommand = (command, args) => {
      calls.push({ command, args });
      return {
        status: 0,
        stdout:
          command === "ps"
            ? "Sun Aug 31 12:34:56 2026\n"
            : "638922404960000000\r\n",
      };
    };

    expect(
      readProcessStartToken(4242, { platform: "linux", spawnCommand }),
    ).toBe("linux:Sun Aug 31 12:34:56 2026");
    expect(
      readProcessStartToken(5252, { platform: "win32", spawnCommand }),
    ).toBe("win32:638922404960000000");
    expect(calls[0]).toMatchObject({
      command: "ps",
      args: ["-o", "lstart=", "-p", "4242"],
    });
    expect(calls[1]).toMatchObject({
      command: "powershell.exe",
    });
  });

  it("distinguishes an exited process from a reused child or launcher PID", () => {
    const state = {
      pid: 4242,
      parentPid: 3131,
      platform: "linux",
      processStartToken: "linux:child-original",
      parentStartToken: "linux:parent-original",
    };

    expect(
      inspectDevProcessIdentity(state, {
        platform: "linux",
        readStartToken: () => null,
      }),
    ).toMatchObject({ childRunning: false, matches: false });
    expect(
      inspectDevProcessIdentity(state, {
        platform: "linux",
        readStartToken(pid) {
          return pid === state.pid
            ? "linux:child-reused"
            : state.parentStartToken;
        },
      }),
    ).toMatchObject({
      childRunning: true,
      matches: false,
      reason: expect.stringContaining("different process start identity"),
    });
    expect(
      inspectDevProcessIdentity(state, {
        platform: "linux",
        readStartToken(pid) {
          return pid === state.pid
            ? state.processStartToken
            : "linux:parent-reused";
        },
      }),
    ).toMatchObject({
      childRunning: true,
      matches: false,
      reason: expect.stringContaining("launcher PID"),
    });
  });

  it.each(["linux", "win32"])(
    "never signals a %s tree when the tracked PID was reused",
    async (platform) => {
      sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-process-"));
      const state = {
        pid: 4242,
        parentPid: 3131,
        platform,
        processStartToken: `${platform}:child-original`,
        parentStartToken: `${platform}:parent-original`,
      };
      await writeDevProcessState(sandbox, state);
      const signals = [];
      const spawned = [];

      await expect(
        stopCompleteDev({
          repoRoot: sandbox,
          platform,
          readStartToken(pid) {
            return pid === state.pid
              ? `${platform}:child-reused`
              : state.parentStartToken;
          },
          killProcess(pid, signal) {
            signals.push({ pid, signal });
          },
          spawnCommand(command, args) {
            spawned.push({ command, args });
            return { status: 0 };
          },
          log: { log() {} },
        }),
      ).rejects.toThrow(/different process start identity/);
      expect(signals).toEqual([]);
      expect(spawned).toEqual([]);
      expect(await readDevProcessState(sandbox)).toMatchObject(state);
    },
  );

  it.each(["linux", "win32"])(
    "rechecks a %s identity immediately before the first destructive signal",
    async (platform) => {
      sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-process-"));
      const state = {
        pid: 4242,
        parentPid: 3131,
        platform,
        processStartToken: `${platform}:child-original`,
        parentStartToken: `${platform}:parent-original`,
      };
      await writeDevProcessState(sandbox, state);
      const signals = [];
      const spawned = [];
      let childChecks = 0;

      await expect(
        stopCompleteDev({
          repoRoot: sandbox,
          platform,
          readStartToken(pid) {
            if (pid === state.parentPid) return state.parentStartToken;
            childChecks += 1;
            return childChecks === 1
              ? state.processStartToken
              : `${platform}:child-reused`;
          },
          killProcess(pid, signal) {
            signals.push({ pid, signal });
          },
          spawnCommand(command, args) {
            spawned.push({ command, args });
            return { status: 0 };
          },
          log: { log() {} },
        }),
      ).rejects.toThrow(/identity changed immediately before stop/);
      expect(signals).toEqual([
        { pid: platform === "win32" ? state.pid : -state.pid, signal: 0 },
        { pid: state.parentPid, signal: 0 },
      ]);
      expect(spawned).toEqual([]);
      expect(await readDevProcessState(sandbox)).toMatchObject(state);
    },
  );

  it("rechecks ownership before escalating a POSIX stop to SIGKILL", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-process-"));
    const state = {
      pid: 4242,
      parentPid: 3131,
      platform: "linux",
      processStartToken: "linux:child-original",
      parentStartToken: "linux:parent-original",
    };
    await writeDevProcessState(sandbox, state);
    const signals = [];
    let termSent = false;

    await expect(
      stopCompleteDev({
        repoRoot: sandbox,
        platform: "linux",
        gracePeriodMs: 0,
        readStartToken(pid) {
          if (pid === state.pid && termSent) return "linux:child-reused";
          return pid === state.pid
            ? state.processStartToken
            : state.parentStartToken;
        },
        killProcess(pid, signal) {
          signals.push({ pid, signal });
          if (signal === "SIGTERM") termSent = true;
        },
        log: { log() {} },
      }),
    ).rejects.toThrow(/refusing to force process group/);
    expect(signals).toContainEqual({ pid: -state.pid, signal: "SIGTERM" });
    expect(signals).not.toContainEqual({ pid: -state.pid, signal: "SIGKILL" });
    expect(await readDevProcessState(sandbox)).toMatchObject(state);
  });

  it("keeps process state when a POSIX group survives SIGKILL", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-process-"));
    const state = {
      pid: 4242,
      parentPid: 3131,
      platform: "linux",
      processStartToken: "linux:child-original",
      parentStartToken: "linux:parent-original",
    };
    await writeDevProcessState(sandbox, state);
    const signals = [];

    await expect(
      stopCompleteDev({
        repoRoot: sandbox,
        platform: "linux",
        gracePeriodMs: 0,
        readStartToken(pid) {
          return pid === state.pid
            ? state.processStartToken
            : state.parentStartToken;
        },
        killProcess(pid, signal) {
          signals.push({ pid, signal });
        },
        log: { log() {} },
      }),
    ).rejects.toThrow(/remained alive after SIGKILL/);
    expect(signals).toContainEqual({ pid: -state.pid, signal: "SIGTERM" });
    expect(signals).toContainEqual({ pid: -state.pid, signal: "SIGKILL" });
    expect(await readDevProcessState(sandbox)).toMatchObject(state);
  });

  it("refuses to clear state recorded on another platform", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-process-"));
    const state = {
      pid: 4242,
      parentPid: 3131,
      platform: "win32",
      processStartToken: "win32:child-original",
      parentStartToken: "win32:parent-original",
    };
    await writeDevProcessState(sandbox, state);

    await expect(
      stopCompleteDev({
        repoRoot: sandbox,
        platform: "linux",
        readStartToken() {
          throw new Error("must not query a mismatched platform");
        },
        killProcess() {
          throw new Error("must not signal a mismatched platform");
        },
        log: { log() {} },
      }),
    ).rejects.toThrow(/recorded platform win32 does not match linux/);
    expect(await readDevProcessState(sandbox)).toMatchObject(state);
  });

  it("keeps POSIX state when the leader exited but its process group remains", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-process-"));
    const state = {
      pid: 4242,
      parentPid: 3131,
      platform: "linux",
      processStartToken: "linux:child-original",
      parentStartToken: "linux:parent-original",
    };
    await writeDevProcessState(sandbox, state);
    const signals = [];

    await expect(
      stopCompleteDev({
        repoRoot: sandbox,
        platform: "linux",
        readStartToken: () => null,
        killProcess(pid, signal) {
          signals.push({ pid, signal });
        },
        log: { log() {} },
      }),
    ).rejects.toThrow(/full process tree cannot be confirmed stopped/);
    expect(signals).toEqual([{ pid: -state.pid, signal: 0 }]);
    expect(await readDevProcessState(sandbox)).toMatchObject(state);
  });

  it("clears stale POSIX state only after both leader and group exit", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-process-"));
    const state = {
      pid: 4242,
      parentPid: 3131,
      platform: "linux",
      processStartToken: "linux:child-original",
      parentStartToken: "linux:parent-original",
    };
    await writeDevProcessState(sandbox, state);

    await expect(
      stopCompleteDev({
        repoRoot: sandbox,
        platform: "linux",
        readStartToken: () => null,
        killProcess() {
          const error = new Error("missing");
          error.code = "ESRCH";
          throw error;
        },
        log: { log() {} },
      }),
    ).resolves.toEqual({ stopped: false, stale: true });
    expect(await readDevProcessState(sandbox)).toBeNull();
  });

  it("keeps stale Windows state when descendants cannot be proven absent", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-process-"));
    const state = {
      pid: 4242,
      parentPid: 3131,
      platform: "win32",
      processStartToken: "win32:child-original",
      parentStartToken: "win32:parent-original",
    };
    await writeDevProcessState(sandbox, state);

    await expect(
      stopCompleteDev({
        repoRoot: sandbox,
        platform: "win32",
        readStartToken: () => null,
        killProcess() {
          throw new Error("must not rely on a reused Windows root PID");
        },
        log: { log() {} },
      }),
    ).rejects.toThrow(/full process tree cannot be confirmed stopped/);
    expect(await readDevProcessState(sandbox)).toMatchObject(state);
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
        const processStartToken = readProcessStartToken(child.pid);
        const parentStartToken = readProcessStartToken(process.pid);
        await writeDevProcessState(sandbox, {
          pid: child.pid,
          parentPid: process.pid,
          platform: process.platform,
          startedAt: new Date().toISOString(),
          processStartToken,
          parentStartToken,
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
