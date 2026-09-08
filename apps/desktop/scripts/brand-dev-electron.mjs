#!/usr/bin/env node
// Prepare this worktree/channel's Electron.app before launch. macOS discovers URL
// schemes and application identity from Info.plist, not app.setName().
// Every development checkout declares only its path-derived callback scheme.
// A development build must never claim production orvilo:// or another
// checkout's callback.
// https://www.electronjs.org/docs/latest/api/app#appsetasdefaultprotocolclientprotocol-path-args
import { createRequire } from "node:module";
import { execFileSync, spawnSync } from "node:child_process";
import { constants, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  callbackProtocolForPath,
  identityHashForPath,
} from "./worktree-dev-env.mjs";

// Keep names, bundle-id prefixes, and callback-scheme prefixes aligned with
// apps/desktop/src/shared/desktop-app-identity.ts. This script runs before
// Electron boots, so it cannot import that TS module. Staging must declare
// orvilo-staging-<hash>:// and never the production orvilo:// handler.
export function devBundleIdentity(appRoot, suffix, channel = "development") {
  const hash = identityHashForPath(appRoot);
  const staging = channel === "staging";
  const prefix = staging ? "ai.orvilo.desktop.staging" : "ai.orvilo.desktop.canary";
  const baseName = staging ? "Orvilo Staging" : "Orvilo Canary";
  const bundleId = `${prefix}.${hash}`;
  const callbackProtocol = callbackProtocolForPath(appRoot, channel);
  return {
    name: suffix ? `${baseName} ${suffix}` : baseName,
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

const VENDOR_BUNDLE_ID = "com.github.Electron";
const VENDOR_BUNDLE_NAME = "Electron";
const DEV_BUNDLE_ID_PATTERN =
  /^ai\.orvilo\.desktop\.(canary|staging)\.[a-f0-9]{16}$/;
const DEV_CALLBACK_SCHEME_PATTERN =
  /^(orvilo-canary|orvilo-staging)-[a-f0-9]{16}$/;
const LSREGISTER =
  "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

function detachPlistForWrite(plistPath) {
  const original = readFileSync(plistPath);
  unlinkSync(plistPath);
  writeFileSync(plistPath, original);
}

export function restoreVendorElectronPlist(plistPath) {
  if (!existsSync(plistPath)) return false;
  const id = plistGet(plistPath, "CFBundleIdentifier");
  const urlName = plistGet(plistPath, "CFBundleURLTypes:0:CFBundleURLName");
  const scheme = plistGet(plistPath, "CFBundleURLTypes:0:CFBundleURLSchemes:0");
  const ours =
    DEV_BUNDLE_ID_PATTERN.test(id) ||
    DEV_BUNDLE_ID_PATTERN.test(urlName.replace(/\.callback$/u, "")) ||
    DEV_CALLBACK_SCHEME_PATTERN.test(scheme);
  if (!ours) return false;

  detachPlistForWrite(plistPath);
  plistSet(plistPath, "CFBundleName", VENDOR_BUNDLE_NAME);
  plistSet(plistPath, "CFBundleDisplayName", VENDOR_BUNDLE_NAME);
  plistSet(plistPath, "CFBundleIdentifier", VENDOR_BUNDLE_ID);
  if (plistGet(plistPath, "CFBundleURLTypes")) {
    execFileSync("/usr/libexec/PlistBuddy", ["-c", "Delete :CFBundleURLTypes", plistPath]);
  }
  return true;
}

function unregisterBundle(path) {
  const result = spawnSync(LSREGISTER, ["-u", path], { encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0 && !`${result.stdout}${result.stderr}`.includes("-10814")) {
    throw new Error(`Unable to unregister vendor Electron.app: ${result.stderr}`);
  }
}

export function prepareDevBundle(electronBin, appRoot, version, suffix, channel = "development") {
  const identity = devBundleIdentity(appRoot, suffix, channel);
  const cacheRoot = resolve(appRoot, "../../.orvilo-dev/electron", `${version}-${process.arch}`);
  const channelRoot = join(cacheRoot, channel === "staging" ? "staging" : "development");
  const bundle = join(channelRoot, "Electron.app");
  if (!existsSync(bundle)) {
    mkdirSync(cacheRoot, { recursive: true });
    const temporary = mkdtempSync(join(cacheRoot, ".prepare-"));
    try {
      cpSync(resolve(electronBin, "../../.."), join(temporary, "Electron.app"), {
        recursive: true,
        verbatimSymlinks: true,
        mode: constants.COPYFILE_FICLONE,
      });
      renameSync(temporary, channelRoot);
    } finally {
      rmSync(temporary, { recursive: true, force: true });
    }
  }
  configureDevPlist(join(bundle, "Contents", "Info.plist"), identity);
  return join(bundle, "Contents", "MacOS", "Electron");
}

export function brandDevElectron(env = process.env) {
  const require = createRequire(import.meta.url);
  const moduleRoot = dirname(require.resolve("electron/package.json"));
  const version = require("electron/package.json").version;
  const executable = readFileSync(join(moduleRoot, "path.txt"), "utf8").trim();
  const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const identity = devBundleIdentity(appRoot, env.DESKTOP_APP_SUFFIX, env.ORVILO_DESKTOP_CHANNEL);
  const sourceBin = join(moduleRoot, "dist", executable);
  const sourceApp = resolve(sourceBin, "../../..");
  const electronBin = prepareDevBundle(
    sourceBin, appRoot, version,
    env.DESKTOP_APP_SUFFIX, env.ORVILO_DESKTOP_CHANNEL,
  );
  // Earlier checkouts branded the shared dependency Electron.app. After the
  // channel copies exist, that leftover identity would still own the callback
  // scheme in Launch Services.
  if (restoreVendorElectronPlist(join(sourceApp, "Contents", "Info.plist"))) {
    unregisterBundle(sourceApp);
  }
  // Publish the build-time declaration before Electron selects itself as the
  // protocol handler. Each worktree/channel has its own bundle ID and callback
  // scheme, despite the common Electron.app filename.
  execFileSync(LSREGISTER, ["-f", resolve(electronBin, "../../..")]);
  console.log(
    `[brand-dev-electron] ${identity.name} (${identity.bundleId}) declares ${identity.callbackProtocol}://`,
  );
  return electronBin;
}

if (process.platform === "darwin" && process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  brandDevElectron();
}
