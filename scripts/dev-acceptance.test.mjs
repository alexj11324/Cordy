import { readFileSync } from "node:fs";
import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";

import {
  parseDevAcceptanceArgs,
  randomLoopbackPort,
  waitForAssistantReply,
} from "./dev-acceptance.mjs";

const repoRoot = resolve(import.meta.dirname, "..");
const runnerSource = readFileSync(resolve(repoRoot, "scripts", "dev-acceptance.mjs"), "utf8");

test("complete Electron development acceptance runner parses explicit options", () => {
  assert.deepEqual(
    parseDevAcceptanceArgs([
      "--provider",
      "telegram",
      "--marker",
      "PATCHBAY_DEV_ACCEPTANCE_TEST",
      "--timeout-ms",
      "20000",
      "--startup-timeout-ms",
      "30000",
    ]),
    {
      provider: "telegram",
      runtimeProvider: undefined,
      marker: "PATCHBAY_DEV_ACCEPTANCE_TEST",
      timeoutMs: 20000,
      startupTimeoutMs: 30000,
      help: false,
    },
  );
  assert.throws(
    () => parseDevAcceptanceArgs(["--provider", "slack"]),
    /--provider must be telegram or weixin/,
  );
  assert.throws(
    () => parseDevAcceptanceArgs(["--timeout-ms", "999"]),
    /--timeout-ms must be an integer between 10000 and 900000/,
  );
  assert.equal(
    parseDevAcceptanceArgs(["--runtime-provider", "Codex"]).runtimeProvider,
    "codex",
  );
  assert.throws(
    () => parseDevAcceptanceArgs(["--runtime-provider", "bad provider"]),
    /--runtime-provider must be a local CLI provider name/,
  );
});

test("complete Electron development acceptance runner keeps CDP port non-privileged", () => {
  assert.equal(randomLoopbackPort({ random: () => 0 }), 42000);
  assert.equal(randomLoopbackPort({ random: () => 0.999999 }), 52000);
});

test("complete Electron development acceptance waits for delayed assistant DOM paint", async () => {
  let now = 0;
  let attempts = 0;
  await waitForAssistantReply({
    deadline: 250,
    now: () => now,
    delayImpl: async (milliseconds) => {
      now += milliseconds;
    },
    markerVisible: async () => {
      attempts += 1;
      return attempts === 3;
    },
  });
  assert.equal(attempts, 3);
});

test("complete Electron development acceptance runner uses the renderer hook without credentials", () => {
  assert.match(runnerSource, /connectOverCDP/);
  assert.match(runnerSource, /__PATCHBAY_DEV_ACCEPTANCE__/);
  assert.match(runnerSource, /stopCompleteDev/);
  assert.doesNotMatch(runnerSource, /PATCHBAY_DEV_ACCEPTANCE_TOKEN/);
  assert.doesNotMatch(runnerSource, /PATCHBAY_DEV_ACCEPTANCE_SECRET/);
});

test("complete Electron development acceptance hands off its reserved CDP port", () => {
  assert.match(runnerSource, /PATCHBAY_DEV_ACCEPTANCE_RELEASE_PORT/);
  assert.match(runnerSource, /acceptanceOwnsChild/);
  assert.match(runnerSource, /assertAcceptanceCheckoutIsIdle/);
  assert.match(
    readFileSync(resolve(repoRoot, "apps", "desktop", "scripts", "dev.mjs"), "utf8"),
    /release-dev-acceptance-port\.mjs[\s\S]*electron-vite/,
  );
});
