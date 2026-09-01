import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
const launcher = readFileSync(resolve(repoRoot, "scripts", "dev.sh"), "utf8");
const platformLauncher = readFileSync(
  resolve(repoRoot, "scripts", "dev-launcher.mjs"),
  "utf8",
);
const stopLauncher = readFileSync(
  resolve(repoRoot, "scripts", "stop-dev.mjs"),
  "utf8",
);
const makefile = readFileSync(resolve(repoRoot, "Makefile"), "utf8");
const runtimePreparer = readFileSync(
  resolve(import.meta.dirname, "prepare-dev-runtime.mjs"),
  "utf8",
);

describe("complete development launcher contract", () => {
  it("prepares DB, starts the backend, waits for readiness, then opens Electron", () => {
    const prepareRuntime = launcher.indexOf("prepare-dev-runtime.mjs");
    const ensureDatabase = launcher.indexOf("ensure-postgres.sh");
    const migrate = launcher.indexOf('"$dev_migrate" up');
    const backend = launcher.indexOf('"$dev_backend") &');
    const readiness = launcher.indexOf("until curl", backend);
    const electron = launcher.indexOf("node apps/desktop/scripts/dev.mjs");

    expect(prepareRuntime).toBeGreaterThan(-1);
    expect(ensureDatabase).toBeGreaterThan(prepareRuntime);
    expect(ensureDatabase).toBeGreaterThan(-1);
    expect(migrate).toBeGreaterThan(ensureDatabase);
    expect(backend).toBeGreaterThan(migrate);
    expect(readiness).toBeGreaterThan(backend);
    expect(electron).toBeGreaterThan(readiness);
  });

  it("pins Electron to this worktree backend and forbids CLI fallback", () => {
    expect(launcher).toContain('export VITE_API_URL="http://127.0.0.1:');
    expect(launcher).toContain('export VITE_WS_URL="ws://127.0.0.1:');
    expect(launcher).toContain("export PATCHBAY_REQUIRE_SOURCE_CLI=1");
    expect(launcher).not.toContain("pnpm dev:web");
    expect(launcher).not.toContain("run-rust.sh run");
    expect(launcher).toContain('dev.mjs "$@"');
  });

  it("starts the live web origin used for browser, share, and login links", () => {
    const backendReadiness = launcher.indexOf("until curl");
    const web = launcher.indexOf("cd apps/web");
    const webReadiness = launcher.indexOf(
      'until curl --fail --silent --show-error "$frontend_ready_url"',
    );
    const electron = launcher.indexOf("node apps/desktop/scripts/dev.mjs");

    expect(web).toBeGreaterThan(backendReadiness);
    expect(webReadiness).toBeGreaterThan(web);
    expect(electron).toBeGreaterThan(webReadiness);
    expect(launcher).toContain(
      'export FRONTEND_ORIGIN="http://localhost:${FRONTEND_PORT:-3000}"',
    );
    expect(launcher).toContain('configured_app_url="${PATCHBAY_APP_URL:-}"');
    expect(launcher).toContain('export PATCHBAY_APP_URL="$configured_app_url"');
    expect(launcher).toContain('export VITE_APP_URL="$FRONTEND_ORIGIN"');
    expect(launcher).toContain("export VITE_ACCOUNTS_URL=");
    expect(launcher).toContain("NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY");
  });

  it("rejects an already occupied backend port and keeps a native Windows path", () => {
    const occupiedCheck = launcher.indexOf(
      "is already serving another Patchbay",
    );
    const spawn = launcher.indexOf('"$dev_backend") &');
    expect(occupiedCheck).toBeGreaterThan(-1);
    expect(spawn).toBeGreaterThan(occupiedCheck);
    expect(launcher).toContain('kill -0 "$backend_pid"');
    expect(platformLauncher).toContain('platform === "win32"');
    expect(platformLauncher).toContain('"dev.ps1"');
  });

  it("tracks and stops the complete Electron process tree per checkout", () => {
    expect(platformLauncher).toContain("writeDevProcessState(repoRoot, state)");
    expect(platformLauncher).toContain('detached: platform !== "win32"');
    expect(stopLauncher).toContain("signalDevProcessTree(state");
    expect(makefile).toContain("@node scripts/stop-dev.mjs");
    expect(makefile).not.toContain("lsof -ti:$(PORT)");
  });

  it("derives the cache toolchain from the same Cargo used for a miss", () => {
    const resolveCargo = runtimePreparer.indexOf("resolveCargoCommand(env");
    const identifyToolchain = runtimePreparer.indexOf(
      "rustToolchainIdentity(env, cargoCommand",
    );
    const completeCacheHit = runtimePreparer.indexOf("cached.every(Boolean)");
    const requireCargo = runtimePreparer.indexOf("if (!cargoCommand)");
    expect(resolveCargo).toBeGreaterThan(-1);
    expect(identifyToolchain).toBeGreaterThan(resolveCargo);
    expect(completeCacheHit).toBeGreaterThan(-1);
    expect(requireCargo).toBeGreaterThan(completeCacheHit);
  });
});
