// @vitest-environment node
import { mkdtempSync, writeFileSync, readFileSync, linkSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { describe, expect, it } from "vitest";
import { configureDevPlist, devBundleIdentity } from "./brand-dev-electron.mjs";

describe("macOS development bundle identity", () => {
  it("keeps stable worktree identities distinct from Electron and production", () => {
    const a = devBundleIdentity("/worktrees/first/apps/desktop", "first");
    expect(a).toEqual(devBundleIdentity("/worktrees/first/apps/desktop", "first"));
    expect(a.bundleId).not.toBe(devBundleIdentity("/worktrees/second/apps/desktop", "first").bundleId);
    expect(a.bundleId).toMatch(/^ai\.patchbay\.desktop\.canary\.[a-f0-9]{16}$/);
    expect(a.name).toBe("Patchbay Canary first");
  });

  it("keeps staging identities off the Canary and production prefixes", () => {
    const staging = devBundleIdentity("/worktrees/first/apps/desktop", "first", "staging");
    const canary = devBundleIdentity("/worktrees/first/apps/desktop", "first");
    expect(staging.name).toBe("Patchbay Staging first");
    expect(staging.bundleId).toMatch(/^ai\.patchbay\.desktop\.staging\.[a-f0-9]{16}$/);
    expect(staging.bundleId).not.toBe(canary.bundleId);
    expect(staging.name).not.toBe(canary.name);
  });

  it.runIf(process.platform === "darwin")("repairs an already-branded app missing its native callback without changing its shared inode", () => {
    const dir = mkdtempSync(join(tmpdir(), "patchbay-dev-plist-"));
    try {
      const original = join(dir, "store.plist");
      const target = join(dir, "Info.plist");
      const xml = `<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleName</key><string>Patchbay Canary first</string><key>CFBundleDisplayName</key><string>Patchbay Canary first</string><key>CFBundleIdentifier</key><string>com.github.Electron</string><key>NSPrincipalClass</key><string>AtomApplication</string></dict></plist>`;
      writeFileSync(original, xml);
      linkSync(original, target);
      const identity = devBundleIdentity("/worktrees/first/apps/desktop", "first");
      expect(configureDevPlist(target, identity)).toBe(true);
      const plist = JSON.parse(execFileSync("plutil", ["-convert", "json", "-o", "-", target], { encoding:"utf8" }));
      expect(plist.CFBundleIdentifier).toBe(identity.bundleId);
      expect(plist.CFBundleURLTypes[0].CFBundleURLSchemes).toEqual(["patchbay"]);
      expect(plist.NSPrincipalClass).toBe("AtomApplication");
      expect(readFileSync(original, "utf8")).toBe(xml);
      expect(configureDevPlist(target, identity)).toBe(false);
    } finally { rmSync(dir, { recursive:true, force:true }); }
  });
});
