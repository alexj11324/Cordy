#!/usr/bin/env node
// Stage and rebrand Electron.app so `pnpm dev:desktop`
// shows "Patchbay Canary" in the menu bar, Cmd+Tab switcher, and
// Activity Monitor. On macOS these titles come from CFBundleName at
// launch time — `app.setName()` cannot override them at runtime.
//
// The staged app lives under ~/Applications because macOS LaunchServices does
// not route custom URL schemes to an Electron bundle inside pnpm's hidden
// `.pnpm` directory. The destination is isolated by callback protocol,
// Electron version, and architecture, and is reused across incremental runs.
//
// In a worktree, scripts/dev.mjs sets DESKTOP_APP_SUFFIX so the name becomes
// "Patchbay Canary <suffix>" — distinguishable in Cmd+Tab and matching the app
// name src/main/index.ts derives from the same env var.

import { createRequire } from "node:module";
import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";

import {
  authCallbackProtocolForSuffix,
  devElectronDistPath,
} from "./worktree-dev-env.mjs";

if (process.platform !== "darwin") process.exit(0);

const DESIRED_NAME = process.env.DESKTOP_APP_SUFFIX
  ? `Patchbay Canary ${process.env.DESKTOP_APP_SUFFIX}`
  : "Patchbay Canary";
const DESIRED_PROTOCOL =
  process.env.DESKTOP_AUTH_CALLBACK_PROTOCOL ??
  authCallbackProtocolForSuffix(process.env.DESKTOP_APP_SUFFIX);
if (
  !/^patchbay-canary(?:-[a-z0-9](?:[a-z0-9-]{0,46}[a-z0-9])?)?$/.test(
    DESIRED_PROTOCOL,
  )
) {
  throw new Error(
    `Invalid DESKTOP_AUTH_CALLBACK_PROTOCOL: ${DESIRED_PROTOCOL}`,
  );
}
const protocolSuffix = DESIRED_PROTOCOL.replace(/^patchbay-canary-?/, "");
const DESIRED_BUNDLE_IDENTIFIER = protocolSuffix
  ? `ai.patchbay.desktop.canary.${protocolSuffix.replace(/-/g, ".")}`
  : "ai.patchbay.desktop.canary";

const require = createRequire(import.meta.url);
const electronPackagePath = require.resolve("electron/package.json");
const electronPackage = JSON.parse(readFileSync(electronPackagePath, "utf8"));
const explicitSourceElectronDistPath =
  process.env.ELECTRON_OVERRIDE_DIST_PATH;
const sourceElectronDistPath =
  explicitSourceElectronDistPath ??
  join(dirname(electronPackagePath), "dist");
const sourceAppBundlePath = join(sourceElectronDistPath, "Electron.app");
const electronDistPath =
  process.env.PATCHBAY_DEV_ELECTRON_DIST_PATH ??
  devElectronDistPath({
    home: homedir(),
    authCallbackProtocol: DESIRED_PROTOCOL,
    electronVersion: electronPackage.version,
    arch: process.arch,
  });
const appBundlePath = join(electronDistPath, "Electron.app");
const plistPath = resolve(appBundlePath, "Contents/Info.plist");

function plistGet(path, key) {
  try {
    return execFileSync(
      "/usr/libexec/PlistBuddy",
      ["-c", `Print :${key}`, path],
      { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
    ).trim();
  } catch {
    return "";
  }
}

function plistSet(path, key, value) {
  try {
    execFileSync("/usr/libexec/PlistBuddy", [
      "-c",
      `Set :${key} ${value}`,
      path,
    ]);
  } catch {
    execFileSync("/usr/libexec/PlistBuddy", [
      "-c",
      `Add :${key} string ${value}`,
      path,
    ]);
  }
}

function plistCommand(path, command, { quiet = false } = {}) {
  execFileSync("/usr/libexec/PlistBuddy", ["-c", command, path], {
    stdio: quiet ? "ignore" : undefined,
  });
}

function resetUrlSchemes(path, protocol) {
  try {
    plistCommand(path, "Delete :CFBundleURLTypes", { quiet: true });
  } catch {
    // The stock Electron development bundle has no URL types.
  }
  plistCommand(path, "Add :CFBundleURLTypes array");
  plistCommand(path, "Add :CFBundleURLTypes:0 dict");
  plistCommand(path, "Add :CFBundleURLTypes:0:CFBundleTypeRole string Editor");
  plistCommand(
    path,
    `Add :CFBundleURLTypes:0:CFBundleURLName string ${DESIRED_BUNDLE_IDENTIFIER}`,
  );
  plistCommand(path, "Add :CFBundleURLTypes:0:CFBundleURLSchemes array");
  plistCommand(
    path,
    `Add :CFBundleURLTypes:0:CFBundleURLSchemes:0 string ${protocol}`,
  );
}

function codeSignatureIsValid(path) {
  try {
    execFileSync(
      "/usr/bin/codesign",
      ["--verify", "--deep", "--strict", path],
      { stdio: "ignore" },
    );
    return true;
  } catch {
    return false;
  }
}

function appBundleIsReady(bundlePath, bundlePlistPath) {
  return (
    existsSync(bundlePlistPath) &&
    plistGet(bundlePlistPath, "CFBundleName") === DESIRED_NAME &&
    plistGet(bundlePlistPath, "CFBundleDisplayName") === DESIRED_NAME &&
    plistGet(bundlePlistPath, "CFBundleIdentifier") ===
      DESIRED_BUNDLE_IDENTIFIER &&
    plistGet(bundlePlistPath, "CFBundleURLTypes:0:CFBundleTypeRole") ===
      "Editor" &&
    plistGet(bundlePlistPath, "CFBundleURLTypes:0:CFBundleURLSchemes:0") ===
      DESIRED_PROTOCOL &&
    codeSignatureIsValid(bundlePath)
  );
}

if (!existsSync(sourceAppBundlePath)) {
  throw new Error(`Electron source bundle is missing: ${sourceAppBundlePath}`);
}

// An explicit source is commonly a locally rebuilt Electron bundle. Its
// contents can change without the version, protocol, or architecture changing,
// so never reuse an earlier staged bundle for that path.
if (
  explicitSourceElectronDistPath === undefined &&
  appBundleIsReady(appBundlePath, plistPath)
) {
  process.exit(0);
}

const destinationParent = dirname(electronDistPath);
mkdirSync(destinationParent, { recursive: true });
const temporaryDistPath = mkdtempSync(
  join(destinationParent, ".electron-staging-"),
);
const temporaryAppBundlePath = join(temporaryDistPath, "Electron.app");
const temporaryPlistPath = resolve(
  temporaryAppBundlePath,
  "Contents/Info.plist",
);
const previousDistPath = `${electronDistPath}.previous-${process.pid}`;

try {
  execFileSync("/bin/cp", ["-cR", sourceAppBundlePath, temporaryAppBundlePath]);

  // Replace the cloned plist inode before editing. This keeps the source
  // package or caller-supplied override untouched when clone-on-write storage
  // is unavailable.
  const original = readFileSync(temporaryPlistPath);
  unlinkSync(temporaryPlistPath);
  writeFileSync(temporaryPlistPath, original);

  plistSet(temporaryPlistPath, "CFBundleName", DESIRED_NAME);
  plistSet(temporaryPlistPath, "CFBundleDisplayName", DESIRED_NAME);
  plistSet(temporaryPlistPath, "CFBundleIdentifier", DESIRED_BUNDLE_IDENTIFIER);
  resetUrlSchemes(temporaryPlistPath, DESIRED_PROTOCOL);
  execFileSync("/usr/bin/xattr", ["-cr", temporaryAppBundlePath]);
  execFileSync("/usr/bin/codesign", [
    "--force",
    "--deep",
    "--sign",
    "-",
    "--identifier",
    DESIRED_BUNDLE_IDENTIFIER,
    temporaryAppBundlePath,
  ]);
  if (!appBundleIsReady(temporaryAppBundlePath, temporaryPlistPath)) {
    throw new Error("Staged Electron bundle failed validation");
  }

  rmSync(previousDistPath, { force: true, recursive: true });
  if (existsSync(electronDistPath)) {
    renameSync(electronDistPath, previousDistPath);
  }
  try {
    renameSync(temporaryDistPath, electronDistPath);
  } catch (error) {
    if (!existsSync(electronDistPath) && existsSync(previousDistPath)) {
      renameSync(previousDistPath, electronDistPath);
    }
    throw error;
  }
  rmSync(previousDistPath, { force: true, recursive: true });
} finally {
  rmSync(temporaryDistPath, { force: true, recursive: true });
}

console.log(
  `[brand-dev-electron] ${plistPath} → name="${DESIRED_NAME}", bundle="${DESIRED_BUNDLE_IDENTIFIER}", callback="${DESIRED_PROTOCOL}://"`,
);
