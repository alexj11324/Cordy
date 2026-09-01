#!/usr/bin/env node

/**
 * Credentialed end-to-end acceptance for the complete Desktop development
 * environment. This is intentionally opt-in and separate from `pnpm dev`:
 * it starts the same launcher, attaches to the real Electron renderer through
 * a random loopback-only CDP port, and asks the renderer to use its normal
 * authenticated API/daemon path. No token is passed to or returned through
 * the CDP hook.
 */

import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { createServer } from "node:net";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { setTimeout as delay } from "node:timers/promises";

import { stopCompleteDev } from "./stop-dev.mjs";
import {
  devProcessLauncherIsRunning,
  devProcessTreeIsRunning,
  inspectDevProcessIdentity,
  readDevProcessState,
  readProcessStartToken,
} from "./dev-process.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const defaultRepoRoot = resolve(here, "..");
const DEFAULT_STARTUP_TIMEOUT_MS = 180_000;
const DEFAULT_ACCEPTANCE_TIMEOUT_MS = 5 * 60 * 1000;
const DEFAULT_CDP_PORT_MIN = 42_000;
const DEFAULT_CDP_PORT_MAX = 52_000;

function usage() {
  return [
    "Usage: pnpm dev:acceptance [--provider telegram|weixin] [--runtime-provider PROVIDER] [--marker MARKER] [--timeout-ms N]",
    "",
    "Starts the complete Electron dev stack and waits for normal user login.",
    "The runner then verifies Electron → daemon → backend → agent in the same renderer.",
    "Provider verification is opt-in and requires a real Settings connection plus a passed test message.",
  ].join("\n");
}

export function parseDevAcceptanceArgs(argv) {
  let provider;
  let runtimeProvider;
  let marker;
  let timeoutMs = DEFAULT_ACCEPTANCE_TIMEOUT_MS;
  let startupTimeoutMs = DEFAULT_STARTUP_TIMEOUT_MS;
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];
    if (token === "--help" || token === "-h") return { help: true };
    if (token === "--provider") {
      provider = argv[++index];
      if (provider !== "telegram" && provider !== "weixin") {
        throw new Error("--provider must be telegram or weixin");
      }
      continue;
    }
    if (token === "--runtime-provider") {
      runtimeProvider = argv[++index];
      if (!runtimeProvider || !/^[a-z0-9][a-z0-9_-]{0,63}$/i.test(runtimeProvider)) {
        throw new Error(
          "--runtime-provider must be a local CLI provider name (letters, numbers, `_` or `-`)",
        );
      }
      runtimeProvider = runtimeProvider.toLowerCase();
      continue;
    }
    if (token === "--marker") {
      marker = argv[++index];
      if (!marker) throw new Error("--marker requires a value");
      continue;
    }
    if (token === "--timeout-ms") {
      timeoutMs = Number(argv[++index]);
      if (!Number.isInteger(timeoutMs) || timeoutMs < 10_000 || timeoutMs > 15 * 60 * 1000) {
        throw new Error("--timeout-ms must be an integer between 10000 and 900000");
      }
      continue;
    }
    if (token === "--startup-timeout-ms") {
      startupTimeoutMs = Number(argv[++index]);
      if (!Number.isInteger(startupTimeoutMs) || startupTimeoutMs < 10_000 || startupTimeoutMs > 10 * 60 * 1000) {
        throw new Error("--startup-timeout-ms must be an integer between 10000 and 600000");
      }
      continue;
    }
    throw new Error(`unknown option: ${token}`);
  }
  return {
    provider,
    runtimeProvider,
    marker,
    timeoutMs,
    startupTimeoutMs,
    help: false,
  };
}

export function randomLoopbackPort({ min = DEFAULT_CDP_PORT_MIN, max = DEFAULT_CDP_PORT_MAX, random = Math.random } = {}) {
  if (!Number.isInteger(min) || !Number.isInteger(max) || min < 1024 || max > 65535 || min > max) {
    throw new Error("invalid loopback port range");
  }
  return min + Math.floor(random() * (max - min + 1));
}

async function listenLoopback(server, port) {
  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen({ host: "127.0.0.1", port }, resolveListen);
  });
}

function closeServer(server) {
  if (!server?.listening) return Promise.resolve();
  return new Promise((resolveClose) => {
    server.close(() => resolveClose());
  });
}

export async function createLoopbackPortReservation({
  createServerImpl = createServer,
} = {}) {
  const cdpServer = createServerImpl();
  // A probe or an accidental local connection must not keep the reservation
  // open or turn it into a second control plane.
  cdpServer.on("connection", (socket) => socket.destroy());
  await listenLoopback(cdpServer, 0);
  const address = cdpServer.address();
  const port = typeof address === "object" && address ? address.port : null;
  if (!Number.isInteger(port) || port < 1024 || port > 65535) {
    await closeServer(cdpServer);
    throw new Error("could not reserve a loopback CDP port");
  }

  const controlServer = createServerImpl();
  const token = randomUUID();
  let released = false;
  let releasePromise;
  const release = async () => {
    if (releasePromise) return releasePromise;
    released = true;
    releasePromise = Promise.all([
      closeServer(cdpServer),
      closeServer(controlServer),
    ]).then(() => undefined);
    return releasePromise;
  };

  controlServer.on("connection", (socket) => {
    let request = "";
    socket.setEncoding("utf8");
    socket.on("data", (chunk) => {
      request += chunk;
      if (!request.includes("\n")) return;
      if (request.trim() !== token || released) {
        socket.end("invalid\n");
        return;
      }
      released = true;
      closeServer(cdpServer)
        .then(() => {
          socket.end("released\n", () => {
            void closeServer(controlServer);
          });
        })
        .catch(() => socket.end("error\n"));
    });
    socket.on("error", () => {});
  });
  try {
    await listenLoopback(controlServer, 0);
  } catch (error) {
    await closeServer(cdpServer);
    throw error;
  }
  const controlAddress = controlServer.address();
  const controlPort =
    typeof controlAddress === "object" && controlAddress
      ? controlAddress.port
      : null;
  if (!Number.isInteger(controlPort)) {
    await release();
    throw new Error("could not reserve the dev acceptance release port");
  }
  return { port, controlPort, token, release };
}

export async function reserveLoopbackPort({ createServerImpl = createServer } = {}) {
  const reservation = await createLoopbackPortReservation({ createServerImpl });
  await reservation.release();
  return reservation.port;
}

function safeError(error) {
  return String(error instanceof Error ? error.message : error)
    .replace(/Bearer\s+[^\s)]+/gi, "Bearer [redacted]")
    .replace(/(token|secret|password|bot[_-]?token)\s*[:=]\s*[^\s,;)}]+/gi, "$1=[redacted]")
    .slice(0, 800);
}

function childExitPromise(child) {
  return new Promise((resolveExit) => {
    child.once("close", (code, signal) => resolveExit({ code, signal }));
  });
}

async function waitForCdp({ endpoint, childExit, timeoutMs, log }) {
  const deadline = Date.now() + timeoutMs;
  let lastLog = 0;
  while (Date.now() < deadline) {
    const exited = await Promise.race([
      childExit.then((value) => ({ exited: true, value })),
      delay(100).then(() => ({ exited: false })),
    ]);
    if (exited.exited) {
      throw new Error(
        `complete dev launcher exited before Electron CDP became ready (code=${exited.value.code ?? "null"}, signal=${exited.value.signal ?? "none"}); stop any existing complete-dev process with \`make stop\` and rerun this command`,
      );
    }
    try {
      const response = await fetch(`${endpoint}/json/version`);
      if (response.ok) return;
    } catch {
      // Electron/Vite is still booting.
    }
    if (Date.now() - lastLog > 10_000) {
      log("waiting for Electron's loopback CDP endpoint...");
      lastLog = Date.now();
    }
    await delay(250);
  }
  throw new Error(`Electron CDP endpoint did not become ready within ${timeoutMs}ms`);
}

async function findAcceptancePage(browser) {
  for (const context of browser.contexts()) {
    for (const page of context.pages()) {
      try {
        if (await page.evaluate(() => Boolean(window.__PATCHBAY_DEV_ACCEPTANCE__))) {
          return page;
        }
      } catch {
        // Page is navigating; try it again on the next poll.
      }
    }
  }
  return null;
}

async function waitForAcceptancePage({ browser, childExit, timeoutMs, log }) {
  const deadline = Date.now() + timeoutMs;
  let lastLog = 0;
  while (Date.now() < deadline) {
    const exited = await Promise.race([
      childExit.then((value) => ({ exited: true, value })),
      delay(100).then(() => ({ exited: false })),
    ]);
    if (exited.exited) {
      throw new Error(
        `complete dev launcher exited before the authenticated main renderer was ready (code=${exited.value.code ?? "null"}, signal=${exited.value.signal ?? "none"}); stop any existing complete-dev process with \`make stop\` and rerun this command`,
      );
    }
    const page = await findAcceptancePage(browser);
    if (page) return page;
    if (Date.now() - lastLog > 10_000) {
      log("waiting for normal Electron login and workspace resolution (no auth bypass is used)...");
      lastLog = Date.now();
    }
    await delay(500);
  }
  throw new Error(
    "the authenticated main Electron renderer was not ready before the acceptance timeout; sign in normally, finish onboarding, then rerun",
  );
}

async function assertAcceptanceCheckoutIsIdle(repoRoot) {
  const state = await readDevProcessState(repoRoot);
  if (!state) return;
  let identity;
  try {
    identity = inspectDevProcessIdentity(state);
  } catch (error) {
    throw new Error(
      `could not verify the existing complete-dev process state: ${safeError(error)}; inspect ${repoRoot}/.patchbay-dev/dev-process.json before running acceptance`,
    );
  }
  if (
    identity.matches &&
    identity.childRunning === true &&
    devProcessTreeIsRunning(state) &&
    devProcessLauncherIsRunning(state)
  ) {
    throw new Error(
      `complete development is already running for this checkout (PID ${state.pid}); run \`make stop\` first, then rerun \`pnpm dev:acceptance\``,
    );
  }
  if (identity.childRunning === true && !identity.matches) {
    throw new Error(
      `the checkout has a running process with an unverified complete-dev identity; inspect ${repoRoot}/.patchbay-dev/dev-process.json and stop it safely before running acceptance`,
    );
  }
}

async function acceptanceOwnsChild({ repoRoot, child, childStartToken }) {
  if (!child?.pid || !childStartToken) return false;
  try {
    const state = await readDevProcessState(repoRoot);
    return Boolean(
      state &&
        state.parentPid === child.pid &&
        state.parentStartToken === childStartToken,
    );
  } catch {
    return false;
  }
}

async function readHookStatus(page) {
  return page.evaluate(() => window.__PATCHBAY_DEV_ACCEPTANCE__?.getStatus() ?? null);
}

async function clickConversationTrigger(page) {
  const trigger = page
    .locator("[data-issue-agent-working]")
    .locator("xpath=../../..")
    .locator("button")
    .first();
  if ((await trigger.count()) === 0 || !(await trigger.isVisible())) return false;
  await trigger.click({ timeout: 3_000 });
  return true;
}

async function assertMarkerInAssistantReply(page, marker) {
  const hasMarker = await page.locator('[role="dialog"] article').evaluateAll(
    (articles, expected) =>
      articles.some(
        (article) =>
          article.className.includes("items-start") &&
          (article.textContent ?? "").includes(expected),
      ),
    marker,
  );
  if (!hasMarker) {
    throw new Error(
      "the task completed, but the expected marker was not visible in an assistant reply in the same Electron window",
    );
  }
}

/**
 * React Query/realtime state can report the completed task before the open
 * conversation dialog has painted its assistant message. Keep the DOM check
 * inside the existing acceptance deadline so that a legitimate render delay
 * is not reported as a broken Electron round trip.
 */
export async function waitForAssistantReply({
  markerVisible,
  deadline,
  now = Date.now,
  delayImpl = delay,
  pollMs = 100,
}) {
  let lastError;
  while (now() < deadline) {
    try {
      if (await markerVisible()) return;
    } catch (error) {
      lastError = error;
    }
    const remaining = deadline - now();
    if (remaining <= 0) break;
    await delayImpl(Math.min(pollMs, remaining));
  }
  if (lastError instanceof Error) throw lastError;
  throw new Error(
    "the expected marker was not visible in an assistant reply before the acceptance timeout",
  );
}

export async function runDevAcceptance({
  repoRoot = defaultRepoRoot,
  argv = process.argv.slice(2),
  env = process.env,
  spawnImpl = spawn,
  reservePort = createLoopbackPortReservation,
  chromiumImpl,
  stopDev = stopCompleteDev,
  log = (message) => console.log(`[dev:acceptance] ${message}`),
} = {}) {
  const options = parseDevAcceptanceArgs(argv);
  if (options.help) {
    log(usage());
    return 0;
  }
  const chromiumApi =
    chromiumImpl ?? (await import("@playwright/test")).chromium;
  await assertAcceptanceCheckoutIsIdle(repoRoot);
  const reserved = await reservePort();
  const reservation =
    typeof reserved === "number"
      ? { port: reserved, release: async () => {} }
      : reserved;
  const cdpPort = reservation.port;
  const endpoint = `http://127.0.0.1:${cdpPort}`;
  const marker = options.marker ?? `PATCHBAY_DEV_ACCEPTANCE_${Date.now()}`;
  let child;
  let childExit;
  let childStartToken;
  let browser;
  let page;
  let cleanupRequested = false;
  let failure;
  try {
    child = spawnImpl(
      process.execPath,
      [resolve(repoRoot, "scripts", "dev-launcher.mjs")],
      {
        cwd: repoRoot,
        env: {
          ...env,
          VITE_PATCHBAY_DEV_ACCEPTANCE: "1",
          VITE_PATCHBAY_DEV_ACCEPTANCE_CDP_PORT: String(cdpPort),
          ...(reservation.controlPort && reservation.token
            ? {
                PATCHBAY_DEV_ACCEPTANCE_RELEASE_PORT: String(
                  reservation.controlPort,
                ),
                PATCHBAY_DEV_ACCEPTANCE_RELEASE_TOKEN: reservation.token,
              }
            : {}),
        },
        stdio: "inherit",
        detached: process.platform !== "win32",
      },
    );
    childExit = childExitPromise(child);
    try {
      childStartToken = readProcessStartToken(child.pid);
    } catch {
      childStartToken = undefined;
    }
    await waitForCdp({ endpoint, childExit, timeoutMs: options.startupTimeoutMs, log });
    browser = await chromiumApi.connectOverCDP(endpoint);
    page = await waitForAcceptancePage({
      browser,
      childExit,
      timeoutMs: options.startupTimeoutMs,
      log,
    });
    const start = await page.evaluate(
      ({ marker: requestedMarker, provider, runtimeProvider, timeoutMs }) =>
        window.__PATCHBAY_DEV_ACCEPTANCE__?.start({
          marker: requestedMarker,
          provider,
          runtimeProvider,
          timeoutMs,
        }) ?? { started: false, message: "acceptance bridge is unavailable" },
      {
        marker,
        provider: options.provider,
        runtimeProvider: options.runtimeProvider,
        timeoutMs: options.timeoutMs,
      },
    );
    if (!start.started) throw new Error(start.message);
    log(`started run ${start.runId}; waiting for the renderer/daemon/backend round trip`);

    const deadline = Date.now() + options.timeoutMs + 30_000;
    let triggerClicked = false;
    let domVerified = false;
    let lastPhase;
    while (Date.now() < deadline) {
      const status = await readHookStatus(page);
      if (!status) throw new Error("Electron acceptance bridge disappeared during the run");
      if (status.phase !== lastPhase) {
        log(`${status.phase}${status.message ? ` — ${status.message}` : ""}`);
        lastPhase = status.phase;
      }
      if (
        !triggerClicked &&
        (["issue-open", "waiting-task"].includes(status.phase) ||
          status.result?.ok === true)
      ) {
        try {
          triggerClicked = await clickConversationTrigger(page);
          if (triggerClicked) log("opened the agent conversation in the same Electron renderer");
        } catch {
          // The issue route may still be loading; retry on the next poll.
        }
      }
      if (status.result?.ok === false) {
        throw new Error(`${status.result.message} Fix: ${status.result.fix}`);
      }
      if (status.result?.ok === true) {
        if (!triggerClicked) {
          await delay(100);
          continue;
        }
        if (!domVerified) {
          await waitForAssistantReply({
            markerVisible: async () => {
              try {
                await assertMarkerInAssistantReply(page, status.result.marker);
                return true;
              } catch {
                return false;
              }
            },
            deadline,
          });
          domVerified = true;
          log("verified the expected marker in an assistant reply DOM node");
        }
        break;
      }
      await delay(500);
    }
    if (!domVerified) throw new Error("Electron acceptance did not reach a verified assistant reply before the timeout");
  } catch (error) {
    failure = error;
  } finally {
    if (page) {
      try {
        const cleanup = await page.evaluate(() =>
          window.__PATCHBAY_DEV_ACCEPTANCE__?.cleanup() ?? {
            ok: true,
            leftovers: [],
          },
        );
        cleanupRequested = true;
        if (!cleanup.ok) {
          const detail = cleanup.message ?? `leftovers: ${cleanup.leftovers.join(", ")}`;
          failure = failure ?? new Error(detail);
          log(`cleanup incomplete: ${detail}`);
        } else {
          log("disposable issue and agent cleaned up");
        }
      } catch (error) {
        failure = failure ?? new Error(`could not request renderer cleanup: ${safeError(error)}`);
      }
    }
    if (browser) await browser.close().catch(() => {});
    await reservation.release().catch((error) => {
      failure = failure ?? new Error(`could not release the acceptance CDP reservation: ${safeError(error)}`);
    });
    if (await acceptanceOwnsChild({ repoRoot, child, childStartToken })) {
      try {
        await stopDev({ repoRoot });
      } catch (error) {
        failure = failure ?? new Error(`could not stop complete dev process tree: ${safeError(error)}`);
      }
    }
  }
  if (failure) {
    throw failure;
  }
  if (!cleanupRequested) throw new Error("acceptance completed without a cleanup request");
  log("complete Electron development acceptance passed");
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  runDevAcceptance().catch((error) => {
    console.error(`[dev:acceptance] ✗ ${safeError(error)}`);
    process.exitCode = 1;
  });
}
