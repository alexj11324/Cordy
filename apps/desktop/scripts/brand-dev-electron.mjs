#!/usr/bin/env node
// Prepare this worktree's Electron.app before launch. macOS discovers URL
// schemes and application identity from Info.plist, not app.setName().
// Every development checkout declares only its path-derived callback scheme.
// A development build must never claim production patchbay:// or another
// checkout's callback.
// https://www.electronjs.org/docs/latest/api/app#appsetasdefaultprotocolclientprotocol-path-args
import { createRequire } from "node:module";
import { execFileSync } from "node:child_process";
import { readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  callbackProtocolForPath,
  identityHashForPath,
} from "./worktree-dev-env.mjs";

export function devBundleIdentity(appRoot, suffix) {
  const hash = identityHashForPath(appRoot);
  const bundleId = `ai.patchbay.desktop.canary.${hash}`;
  const callbackProtocol = callbackProtocolForPath(appRoot);
  return {
    name: suffix ? `Orvilo Canary ${suffix}` : "Orvilo Canary",
    bundleId,
    callbackProtocol,
    callbackSchemes: [callbackProtocol],
    callbackUrlName: `${bundleId}.callback`,
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

function declaredCallbackSchemesMatch(plistPath, schemes) {
  return (
    schemes.every(
      (scheme, index) =>
        plistGet(plistPath, `CFBundleURLTypes:0:CFBundleURLSchemes:${index}`) === scheme,
    ) &&
    plistGet(plistPath, `CFBundleURLTypes:0:CFBundleURLSchemes:${schemes.length}`) === ""
  );
}

export function configureDevPlist(plistPath, identity) {
  if (
    plistGet(plistPath, "CFBundleName") === identity.name &&
    plistGet(plistPath, "CFBundleDisplayName") === identity.name &&
    plistGet(plistPath, "CFBundleIdentifier") === identity.bundleId &&
    plistGet(plistPath, "CFBundleURLTypes:0:CFBundleURLName") ===
      identity.callbackUrlName &&
    declaredCallbackSchemesMatch(plistPath, identity.callbackSchemes) &&
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
  const schemeCommands = identity.callbackSchemes.map(
    (scheme, index) =>
      `Add :CFBundleURLTypes:0:CFBundleURLSchemes:${index} string ${scheme}`,
  );
  for (const command of [
    "Add :CFBundleURLTypes array",
    "Add :CFBundleURLTypes:0 dict",
    `Add :CFBundleURLTypes:0:CFBundleURLName string ${identity.callbackUrlName}`,
    "Add :CFBundleURLTypes:0:CFBundleURLSchemes array",
    ...schemeCommands,
  ]) execFileSync("/usr/libexec/PlistBuddy", ["-c", command, plistPath]);
  return true;
}

if (process.platform === "darwin" && process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const require = createRequire(import.meta.url);
  const electronBin = require("electron");
  const plistPath = resolve(electronBin, "../../Info.plist");
  const identity = devBundleIdentity(resolve(dirname(fileURLToPath(import.meta.url)), ".."), process.env.DESKTOP_APP_SUFFIX);
  configureDevPlist(plistPath, identity);
  // Publish the build-time declaration before Electron selects itself as the
  // protocol handler. Each worktree has its own bundle ID and callback scheme,
  // despite the common Electron.app filename.
  execFileSync("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister", ["-f", resolve(plistPath, "../..")]);
  console.log(
    `[brand-dev-electron] ${identity.name} (${identity.bundleId}) declares ${identity.callbackProtocol}://`,
  );
}
