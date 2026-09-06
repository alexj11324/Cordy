import { execFileSync, spawnSync } from "node:child_process";
import { cpSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { envWithLocalBins } from "./package.mjs";
import { applyWorktreeDevEnv, repoRootFromScriptDir } from "./worktree-dev-env.mjs";
import { configureDevPlist, devBundleIdentity } from "./brand-dev-electron.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = repoRootFromScriptDir(here);
const appRoot = join(root, "apps", "desktop");
applyWorktreeDevEnv(process.env, { root, log: true });
process.env.PATCHBAY_REQUIRE_SOURCE_CLI = "1";
const env = envWithLocalBins(process.env);
function run(command, args) {
  const result = spawnSync(command, args, { cwd: appRoot, stdio: "inherit", env, shell: process.platform === "win32" });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
run(process.execPath, [join(here, "prepare-dev-runtime.mjs")]);
run("electron-vite", ["build"]);
if (process.platform !== "darwin") {
  run("electron-vite", ["preview", "--skipBuild", ...process.argv.slice(2)]);
  process.exit(0);
}
// A real app bundle gives Launch Services a cold-start entry point. Keep the
// dependency's Electron.app untouched, and reuse the release packager/signing.
const require = createRequire(import.meta.url);
const { build, Platform, Arch } = require("electron-builder");
const identity = devBundleIdentity(appRoot, process.env.DESKTOP_APP_SUFFIX);
const output = join(root, ".patchbay-dev", "preview");
await build({
  projectDir: appRoot,
  targets: Platform.MAC.createTarget("dir", process.arch === "arm64" ? Arch.arm64 : Arch.x64),
  config: {
    extends: null,
    appId: identity.bundleId,
    productName: identity.name,
    protocols: [{ name: identity.callbackUrlName, schemes: identity.callbackSchemes }],
    directories: { output },
    extraMetadata: { desktopPreview: { bundleId: identity.bundleId, name: identity.name,
      dataName: identity.name.replace("Orvilo", "Patchbay"), callbackProtocol: identity.callbackProtocol } },
    mac: { identity: "-", notarize: false },
    // Config arrays can inherit the release protocol. Replace the generated
    // bundle declaration before signing so this app owns only its Canary URL.
    afterPack: async ({ appOutDir, packager }) => {
      configureDevPlist(join(appOutDir, `${packager.appInfo.productFilename}.app`, "Contents", "Info.plist"), identity);
    },
    publish: null,
  },
});
const bundle = join(output, process.arch === "arm64" ? "mac-arm64" : "mac", `${identity.name}.app`);
run("/usr/bin/codesign", ["--verify", "--deep", "--strict", bundle]);
const lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
// Retire only this checkout's earlier modified Electron declaration, if present.
const oldBundle = join(dirname(require("electron")), "..", "..");
const oldPlist = join(oldBundle, "Contents", "Info.plist");
function unregisterOwnedBundle(path) {
  const result = spawnSync(lsregister, ["-u", path], { encoding: "utf8" });
  if (result.error) throw result.error;
  // Launch Services reports application-not-found when a prior preview was
  // already unregistered. That is the desired state, not an install failure.
  if (result.status !== 0 && !`${result.stdout}${result.stderr}`.includes("-10814")) {
    throw new Error(`Unable to unregister prior preview: ${result.stderr}`);
  }
}
if (readFileSync(oldPlist, "utf8").includes(identity.bundleId)) unregisterOwnedBundle(oldBundle);
// Launch Services excludes apps in temporary directories from URL dispatch.
// Worktrees may live under /tmp; install only the generated preview bundle in
// the user's Applications directory, retaining the same per-worktree identity.
const installRoot = join(homedir(), "Applications", "Orvilo Development");
const installed = join(installRoot, `${identity.name}.app`);
const running = execFileSync("/usr/bin/osascript", ["-l", "JavaScript", "-e",
  `ObjC.import("AppKit"); $.NSRunningApplication.runningApplicationsWithBundleIdentifier($(${JSON.stringify(identity.bundleId)})).count;`,
], { encoding: "utf8" }).trim();
if (Number(running) > 0) throw new Error(`Quit ${identity.name} before replacing its installed preview.`);
const schemes = JSON.parse(execFileSync("/usr/bin/plutil", ["-extract", "CFBundleURLTypes", "json", "-o", "-", join(bundle, "Contents", "Info.plist")], { encoding: "utf8" }))
  .flatMap(entry => entry.CFBundleURLSchemes ?? []);
if (schemes.length !== 1 || schemes[0] !== identity.callbackProtocol) {
  throw new Error("Preview must declare only its worktree callback protocol");
}
mkdirSync(installRoot, { recursive: true });
rmSync(installed, { recursive: true, force: true });
cpSync(bundle, installed, { recursive: true, verbatimSymlinks: true });
run("/usr/bin/codesign", ["--verify", "--deep", "--strict", installed]);
unregisterOwnedBundle(bundle);
run(lsregister, ["-f", installed]);
console.log(`[preview] signed app: ${installed}`);
run("/usr/bin/open", [installed, "--args", ...process.argv.slice(2).filter(arg => arg !== "--")]);

const resolved = execFileSync("/usr/bin/osascript", ["-l", "JavaScript", "-e",
  `ObjC.import("AppKit"); var app=$.NSWorkspace.sharedWorkspace.URLForApplicationToOpenURL($.NSURL.URLWithString(${JSON.stringify(identity.callbackProtocol + "://auth/callback")})); ObjC.unwrap(app.absoluteString)||"";`,
], { encoding: "utf8" }).trim();
if (!resolved || fileURLToPath(resolved).replace(/\/$/, "") !== installed) {
  throw new Error("macOS did not register this preview as its callback handler");
}
console.log(`[preview] callback handler verified: ${identity.callbackProtocol}`);
