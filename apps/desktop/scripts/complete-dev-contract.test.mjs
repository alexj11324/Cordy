import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
const launcher = readFileSync(resolve(repoRoot, "scripts", "dev.sh"), "utf8");
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
    const readiness = launcher.indexOf("/healthz");
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
  });

  it("returns a complete runtime cache hit before resolving Cargo", () => {
    const completeCacheHit = runtimePreparer.indexOf("cached.every(Boolean)");
    const resolveCargo = runtimePreparer.indexOf("resolveCargoCommand(env");
    expect(completeCacheHit).toBeGreaterThan(-1);
    expect(resolveCargo).toBeGreaterThan(completeCacheHit);
  });
});
