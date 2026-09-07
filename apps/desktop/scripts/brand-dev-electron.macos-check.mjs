import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { devBundleIdentity, prepareDevBundle } from "./brand-dev-electron.mjs";

test("Canary and Staging keep independent native bundle registrations", { skip: process.platform !== "darwin" }, () => {
  const root = mkdtempSync(join(tmpdir(), "desktop-bundles-"));
  try {
    const source = join(root, "dependency", "Electron.app", "Contents");
    mkdirSync(join(source, "MacOS"), { recursive: true });
    const xml = '<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.github.Electron</string></dict></plist>';
    writeFileSync(join(source, "Info.plist"), xml);
    const executable = join(source, "MacOS", "Electron");
    writeFileSync(executable, "fixture executable");
    const appRoot = join(root, "checkout", "apps", "desktop");
    const canary = prepareDevBundle(executable, appRoot, "41.0.0", undefined);
    const canaryPlist = resolve(canary, "../../Info.plist");
    const before = readFileSync(canaryPlist, "utf8");
    const staging = prepareDevBundle(executable, appRoot, "41.0.0", undefined, "staging");
    assert.notEqual(canary, staging);
    assert.equal(readFileSync(canaryPlist, "utf8"), before);
    assert.equal(readFileSync(join(source, "Info.plist"), "utf8"), xml);
    for (const [binary, channel] of [[canary, "development"], [staging, "staging"]]) {
      const identity = devBundleIdentity(appRoot, undefined, channel);
      const plist = JSON.parse(execFileSync("/usr/bin/plutil", ["-convert", "json", "-o", "-", resolve(binary, "../../Info.plist")], { encoding: "utf8" }));
      assert.equal(plist.CFBundleIdentifier, identity.bundleId);
      assert.deepEqual(plist.CFBundleURLTypes[0].CFBundleURLSchemes, [identity.callbackProtocol]);
      assert.equal(readFileSync(binary, "utf8"), "fixture executable");
    }
    assert.equal(prepareDevBundle(executable, appRoot, "41.0.0", undefined), canary);
    assert.notEqual(prepareDevBundle(executable, appRoot, "42.0.0", undefined), canary);
    const launcher = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "dev.mjs"), "utf8");
    assert.match(launcher, /process\.env\.ELECTRON_EXEC_PATH = brandDevElectron\(\)/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
