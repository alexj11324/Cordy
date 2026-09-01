import { mkdtempSync, rmSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  appSuffixForPath,
  authCallbackProtocolForSuffix,
  applyMacOSDevElectronEnv,
  applyWorktreeDevEnv,
  cksum,
  devElectronDistPath,
  offsetForPath,
  rendererPortForPath,
} from "./worktree-dev-env.mjs";

const cleanups = [];
afterEach(() => {
  while (cleanups.length) cleanups.pop()();
});

function tmpRoot(kind /* "file" | "dir" | "none" */) {
  const root = mkdtempSync(join(tmpdir(), "wt-"));
  cleanups.push(() => rmSync(root, { recursive: true, force: true }));
  if (kind === "file")
    writeFileSync(join(root, ".git"), "gitdir: /elsewhere\n");
  else if (kind === "dir") mkdirSync(join(root, ".git"));
  return root;
}

describe("worktree-dev-env", () => {
  it("cksum is byte-compatible with coreutils cksum(1)", () => {
    // `printf '%s' "/tmp/foo" | cksum` → 427878967 8
    expect(cksum(Buffer.from("/tmp/foo"))).toBe(427878967);
    // `printf '' | cksum` → 4294967295 0
    expect(cksum(Buffer.from(""))).toBe(4294967295);
  });

  it("derives the offset from the path, mod 1000", () => {
    expect(offsetForPath("/tmp/foo")).toBe(427878967 % 1000);
  });

  it("renderer port is 5174 + offset (5173 reserved for the primary checkout)", () => {
    expect(rendererPortForPath("/tmp/foo")).toBe(5174 + (427878967 % 1000));
  });

  it("never reuses 5173 even when the offset is 0", () => {
    expect(offsetForPath("/tmp/patchbay-3030")).toBe(0);
    expect(rendererPortForPath("/tmp/patchbay-3030")).toBe(5174);
    expect(rendererPortForPath("/tmp/patchbay-3030")).not.toBe(5173);
  });

  it("skips 6000, which Chromium refuses to load (ERR_UNSAFE_PORT)", () => {
    // POSIX cksum("/tmp/wt-570") === 109908826, % 1000 === 826 → 5174 + 826 === 6000
    expect(offsetForPath("/tmp/wt-570")).toBe(826);
    expect(rendererPortForPath("/tmp/wt-570")).toBe(6174);
  });

  it("stays collision-free across every offset while skipping restricted ports", () => {
    // The remap must stay injective: two worktrees sharing a port means the
    // second Electron dies on EADDRINUSE. Cover all 1000 offsets with real
    // paths so this exercises rendererPortForPath rather than restating it.
    const pathForOffset = new Map();
    for (let i = 0; pathForOffset.size < 1000; i++) {
      const path = `/tmp/wt-${i}`;
      const offset = offsetForPath(path);
      if (!pathForOffset.has(offset)) pathForOffset.set(offset, path);
    }

    const ports = new Set(
      [...pathForOffset.values()].map((path) => rendererPortForPath(path)),
    );
    expect(ports.size).toBe(1000);
    expect(ports.has(6000)).toBe(false);
    expect(ports.has(5173)).toBe(false);
  });

  it("suffix is '<folder>-<offset>' so it stays recognizable and unique", () => {
    expect(appSuffixForPath("/work/PB-3724_Desktop")).toBe(
      `pb-3724-desktop-${offsetForPath("/work/PB-3724_Desktop")}`,
    );
    expect(appSuffixForPath("/work/feat/some thing")).toBe(
      `some-thing-${offsetForPath("/work/feat/some thing")}`,
    );
    // empty/non-ascii slug falls back to "worktree", still disambiguated by offset
    expect(appSuffixForPath("/work/___")).toBe(
      `worktree-${offsetForPath("/work/___")}`,
    );
  });

  it("derives an isolated callback protocol without losing the path offset", () => {
    expect(authCallbackProtocolForSuffix("login-fix-123")).toBe(
      "patchbay-canary-login-fix-123",
    );
    expect(authCallbackProtocolForSuffix()).toBe("patchbay-canary");
    expect(
      authCallbackProtocolForSuffix(
        `${"very-long-worktree-name-".repeat(4)}987`,
      ),
    ).toMatch(/^patchbay-canary-[a-z0-9-]+-987$/);
  });

  it("stages macOS Electron in a visible per-protocol Applications path", () => {
    expect(
      devElectronDistPath({
        home: "/Users/tester",
        authCallbackProtocol: "patchbay-canary-login-fix-123",
        electronVersion: "39.8.7",
        arch: "arm64",
      }),
    ).toBe(
      "/Users/tester/Applications/Patchbay Development/patchbay-canary-login-fix-123/39.8.7-arm64",
    );

    const env = {
      DESKTOP_AUTH_CALLBACK_PROTOCOL: "patchbay-canary-login-fix-123",
    };
    applyMacOSDevElectronEnv(env, {
      home: "/Users/tester",
      electronVersion: "39.8.7",
      arch: "arm64",
      platform: "darwin",
    });
    expect(env.PATCHBAY_DEV_ELECTRON_DIST_PATH).toBe(
      "/Users/tester/Applications/Patchbay Development/patchbay-canary-login-fix-123/39.8.7-arm64",
    );
  });

  it("leaves non-macOS alone and keeps an explicit Electron override as the source", () => {
    const linuxEnv = {
      DESKTOP_AUTH_CALLBACK_PROTOCOL: "patchbay-canary-linux-123",
    };
    applyMacOSDevElectronEnv(linuxEnv, {
      home: "/home/tester",
      electronVersion: "39.8.7",
      platform: "linux",
    });
    expect(linuxEnv.PATCHBAY_DEV_ELECTRON_DIST_PATH).toBeUndefined();

    const explicitEnv = {
      DESKTOP_AUTH_CALLBACK_PROTOCOL: "patchbay-canary-login-fix-123",
      ELECTRON_OVERRIDE_DIST_PATH: "/custom/electron",
    };
    applyMacOSDevElectronEnv(explicitEnv, {
      home: "/Users/tester",
      electronVersion: "39.8.7",
      arch: "arm64",
      platform: "darwin",
    });
    expect(explicitEnv.ELECTRON_OVERRIDE_DIST_PATH).toBe("/custom/electron");
    expect(explicitEnv.PATCHBAY_DEV_ELECTRON_DIST_PATH).toBe(
      "/Users/tester/Applications/Patchbay Development/patchbay-canary-login-fix-123/39.8.7-arm64",
    );
  });

  it("disambiguates worktrees that share a folder name at different paths", () => {
    // Same basename "patchbay", different parent dirs → different offsets/suffixes,
    // so each gets its own single-instance lock.
    expect(offsetForPath("/tmp/a/patchbay")).not.toBe(
      offsetForPath("/tmp/b/patchbay"),
    );
    expect(appSuffixForPath("/tmp/a/patchbay")).not.toBe(
      appSuffixForPath("/tmp/b/patchbay"),
    );
  });

  it("auto-isolates a linked worktree (.git is a file)", () => {
    const root = tmpRoot("file");
    const env = {};
    applyWorktreeDevEnv(env, { root });
    expect(env.DESKTOP_RENDERER_PORT).toBe(String(rendererPortForPath(root)));
    expect(env.DESKTOP_APP_SUFFIX).toBe(appSuffixForPath(root));
    expect(env.DESKTOP_AUTH_CALLBACK_PROTOCOL).toBe(
      authCallbackProtocolForSuffix(appSuffixForPath(root)),
    );
  });

  it("leaves the primary checkout untouched (.git is a dir)", () => {
    const root = tmpRoot("dir");
    const env = {};
    applyWorktreeDevEnv(env, { root });
    expect(env.DESKTOP_RENDERER_PORT).toBeUndefined();
    expect(env.DESKTOP_APP_SUFFIX).toBeUndefined();
    expect(env.DESKTOP_AUTH_CALLBACK_PROTOCOL).toBe("patchbay-canary");
  });

  it("respects explicit env overrides", () => {
    const root = tmpRoot("file");
    const env = { DESKTOP_RENDERER_PORT: "9999", DESKTOP_APP_SUFFIX: "manual" };
    applyWorktreeDevEnv(env, { root });
    expect(env.DESKTOP_RENDERER_PORT).toBe("9999");
    expect(env.DESKTOP_APP_SUFFIX).toBe("manual");
    expect(env.DESKTOP_AUTH_CALLBACK_PROTOCOL).toBe("patchbay-canary-manual");
  });

  it("does not replace an explicit callback protocol", () => {
    const root = tmpRoot("file");
    const env = {
      DESKTOP_AUTH_CALLBACK_PROTOCOL: "patchbay-canary-explicit-777",
    };
    applyWorktreeDevEnv(env, { root });
    expect(env.DESKTOP_AUTH_CALLBACK_PROTOCOL).toBe(
      "patchbay-canary-explicit-777",
    );
  });

  it("fills only the missing knob when one is set explicitly", () => {
    const root = tmpRoot("file");
    const env = { DESKTOP_RENDERER_PORT: "9999" };
    applyWorktreeDevEnv(env, { root });
    expect(env.DESKTOP_RENDERER_PORT).toBe("9999");
    expect(env.DESKTOP_APP_SUFFIX).toBe(appSuffixForPath(root));
  });
});
