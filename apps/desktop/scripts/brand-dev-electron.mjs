#!/usr/bin/env node
// Prepare this worktree's Electron.app before launch. macOS discovers URL
// schemes and application identity from Info.plist, not app.setName().
// https://www.electronjs.org/docs/latest/api/app#appsetasdefaultprotocolclientprotocol-path-args
import { createRequire } from "node:module";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Keep names and bundle-id prefixes aligned with
// apps/desktop/src/shared/desktop-app-identity.ts. This script runs before
// Electron boots, so it cannot import that TS module.
export function devBundleIdentity(appRoot, suffix, channel = "development") {
  const hash = createHash("sha256").update(resolve(appRoot)).digest("hex").slice(0, 16);
  const staging = channel === "staging";
  const baseName = staging ? "Patchbay Staging" : "Patchbay Canary";
  const prefix = staging ? "ai.patchbay.desktop.staging" : "ai.patchbay.desktop.canary";
  return {
    name: suffix ? `${baseName} ${suffix}` : baseName,
    bundleId: `${prefix}.${hash}`,
  };
}

function plistGet(plistPath, key) {
  try {
    return execFileSync("/usr/libexec/PlistBuddy", ["-c", `Print :${key}`, plistPath], {
      encoding: "utf8", stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return "";
  }
}

function plistSet(plistPath, key, value) {
  try {
    execFileSync("/usr/libexec/PlistBuddy", ["-c", `Set :${key} ${value}`, plistPath]);
  } catch {
    execFileSync("/usr/libexec/PlistBuddy", ["-c", `Add :${key} string ${value}`, plistPath]);
  }
}

export function configureDevPlist(plistPath, identity) {
  if (
    plistGet(plistPath, "CFBundleName") === identity.name &&
    plistGet(plistPath, "CFBundleDisplayName") === identity.name &&
    plistGet(plistPath, "CFBundleIdentifier") === identity.bundleId &&
    plistGet(plistPath, "CFBundleURLTypes:0:CFBundleURLSchemes:0") === "patchbay" &&
    plistGet(plistPath, "NSPrincipalClass") === "AtomApplication"
  ) return false;

  // Detach the pnpm-store inode before modifying this worktree's app bundle.
  const original = readFileSync(plistPath);
  unlinkSync(plistPath);
  writeFileSync(plistPath, original);
  plistSet(plistPath, "CFBundleName", identity.name);
  plistSet(plistPath, "CFBundleDisplayName", identity.name);
  plistSet(plistPath, "CFBundleIdentifier", identity.bundleId);
  plistSet(plistPath, "NSPrincipalClass", "AtomApplication");
  if (plistGet(plistPath, "CFBundleURLTypes")) {
    execFileSync("/usr/libexec/PlistBuddy", ["-c", "Delete :CFBundleURLTypes", plistPath]);
  }
  for (const command of [
    "Add :CFBundleURLTypes array",
    "Add :CFBundleURLTypes:0 dict",
    "Add :CFBundleURLTypes:0:CFBundleURLName string Patchbay",
    "Add :CFBundleURLTypes:0:CFBundleURLSchemes array",
    "Add :CFBundleURLTypes:0:CFBundleURLSchemes:0 string patchbay",
  ]) execFileSync("/usr/libexec/PlistBuddy", ["-c", command, plistPath]);
  return true;
}

if (process.platform === "darwin" && process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const require = createRequire(import.meta.url);
  const electronBin = require("electron");
  const plistPath = resolve(electronBin, "../../Info.plist");
  const identity = devBundleIdentity(
    resolve(dirname(fileURLToPath(import.meta.url)), ".."),
    process.env.DESKTOP_APP_SUFFIX,
    process.env.PATCHBAY_DESKTOP_CHANNEL,
  );
  configureDevPlist(plistPath, identity);
  // Publish the build-time declaration before Electron selects itself as the
  // protocol handler. Each worktree has its own bundle ID, despite the common
  // Electron.app filename. Only this app is registered; no global cache reset.
  execFileSync("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister", ["-f", resolve(plistPath, "../..")]);
  console.log(`[brand-dev-electron] ${identity.name} (${identity.bundleId}) declares patchbay://`);
}
