import { mkdtempSync, rmSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  appSuffixForPath,
  applyWorktreeDevEnv,
  callbackProtocolForPath,
  cksum,
  identityHashForPath,
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
  if (kind === "file") writeFileSync(join(root, ".git"), "gitdir: /elsewhere\n");
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
    // POSIX cksum("/tmp/patchbay-3030") === 241176000, % 1000 === 0.
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

  it("suffix is '<folder>-<path-hash>' so data and locks stay unique", () => {
    expect(appSuffixForPath("/work/MUL-3724_Desktop")).toBe(
      `mul-3724-desktop-${identityHashForPath("/work/MUL-3724_Desktop").slice(0, 12)}`,
    );
    expect(appSuffixForPath("/work/feat/some thing")).toBe(
      `some-thing-${identityHashForPath("/work/feat/some thing").slice(0, 12)}`,
    );
    // empty/non-ascii slug falls back to "worktree", still disambiguated by hash
    expect(appSuffixForPath("/work/___")).toBe(
      `worktree-${identityHashForPath("/work/___").slice(0, 12)}`,
    );
  });

  it("disambiguates worktrees that share a folder name at different paths", () => {
    // Same basename "patchbay", different parent dirs → different suffixes,
    // so each gets its own userData and single-instance lock.
    expect(appSuffixForPath("/tmp/a/patchbay")).not.toBe(
      appSuffixForPath("/tmp/b/patchbay"),
    );
  });

  it("derives a stable callback protocol from the full app path", () => {
    expect(callbackProtocolForPath("/tmp/a/patchbay/apps/desktop")).toMatch(
      /^patchbay-canary-[a-f0-9]{16}$/,
    );
    expect(callbackProtocolForPath("/tmp/a/patchbay/apps/desktop")).toBe(
      callbackProtocolForPath("/tmp/a/patchbay/apps/desktop"),
    );
    expect(callbackProtocolForPath("/tmp/a/patchbay/apps/desktop")).not.toBe(
      callbackProtocolForPath("/tmp/b/patchbay/apps/desktop"),
    );
  });

  it("auto-isolates a linked worktree (.git is a file)", () => {
    const root = tmpRoot("file");
    const env = {};
    applyWorktreeDevEnv(env, { root });
    expect(env.DESKTOP_RENDERER_PORT).toBe(String(rendererPortForPath(root)));
    expect(env.DESKTOP_APP_SUFFIX).toBe(appSuffixForPath(root));
    expect(env.DESKTOP_CALLBACK_PROTOCOL).toBe(
      callbackProtocolForPath(join(root, "apps", "desktop")),
    );
  });

  it("keeps primary port/name defaults while isolating its callback protocol", () => {
    const root = tmpRoot("dir");
    const env = {};
    applyWorktreeDevEnv(env, { root });
    expect(env.DESKTOP_RENDERER_PORT).toBeUndefined();
    expect(env.DESKTOP_APP_SUFFIX).toBeUndefined();
    expect(env.DESKTOP_CALLBACK_PROTOCOL).toBe(
      callbackProtocolForPath(join(root, "apps", "desktop")),
    );
  });

  it("respects port/name overrides but keeps callback ownership path-bound", () => {
    const root = tmpRoot("file");
    const env = {
      DESKTOP_RENDERER_PORT: "9999",
      DESKTOP_APP_SUFFIX: "manual",
      DESKTOP_CALLBACK_PROTOCOL: "patchbay-canary-0000000000000000",
    };
    applyWorktreeDevEnv(env, { root });
    expect(env.DESKTOP_RENDERER_PORT).toBe("9999");
    expect(env.DESKTOP_APP_SUFFIX).toBe("manual");
    expect(env.DESKTOP_CALLBACK_PROTOCOL).toBe(
      callbackProtocolForPath(join(root, "apps", "desktop")),
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
