import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

import { chromium, expect } from "@playwright/test";
import { clerk, setupClerkTestingToken } from "@clerk/testing/playwright";

import {
  buildProductionSmokeDependencyPlan,
  buildProductionSmokeGraphIdempotencyKey,
  buildGoogleOAuthProbeUrl,
  decodeClerkFrontendApi,
  findProductionSmokeGraph,
  findProductionSmokeParentIssue,
  isExpectedBrowserRequestCancellation,
  PRODUCTION_SMOKE_DEPENDENT_ACCEPTANCE,
  PRODUCTION_SMOKE_DEPENDENT_TASK_TITLE,
  PRODUCTION_SMOKE_PARENT_TITLE,
  requiredString,
  requireBrowserReceipt,
  requireGoogleOAuthNavigation,
  requireNoDefaultExecutionAgent,
  requireProductionSmokeGraph,
  requireProductionSmokeGraphContract,
  requireProtectedNavigation,
} from "./verify-production-browser-contract.mjs";

const SLUG_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/u;
const BASE_URL = "https://patchbay.aspectlylabs.com";
const ACCOUNTS_BASE_URL = "https://accounts.aspectlylabs.com";
const SMOKE_EMAIL = "production-smoke@aspectlylabs.com";
const SMOKE_WORKSPACE = "production-smoke";
const SCREENSHOT_PATH = "/tmp/production-browser-failure.png";
const FIRST_PARTY_HOSTS = new Set([
  "api.aspectlylabs.com",
  "patchbay.aspectlylabs.com",
]);

function isFirstPartyUrl(rawUrl) {
  try {
    return FIRST_PARTY_HOSTS.has(new URL(rawUrl).hostname);
  } catch {
    return false;
  }
}

async function ticketSignIn(page, ticket) {
  const result = await page.evaluate(async (signInTicket) => {
    const attempt = await window.Clerk.client.signIn.create({
      strategy: "ticket",
      ticket: signInTicket,
    });
    if (attempt.status !== "complete" || !attempt.createdSessionId) {
      return { status: attempt.status, sessionId: null };
    }
    await window.Clerk.setActive({ session: attempt.createdSessionId });
    return { status: attempt.status, sessionId: attempt.createdSessionId };
  }, ticket);
  if (result.status !== "complete" || !result.sessionId) {
    throw new Error(`Clerk ticket sign-in did not complete (${result.status})`);
  }
}

async function waitForPatchbayExchange(page, action) {
  const exchange = page.waitForResponse(
    (response) => {
      const url = new URL(response.url());
      return (
        FIRST_PARTY_HOSTS.has(url.hostname) &&
        url.pathname === "/auth/clerk" &&
        response.request().method() === "POST"
      );
    },
    { timeout: 30_000 },
  );
  await action();
  const response = await exchange;
  if (response.status() !== 200) {
    throw new Error(
      `Clerk to Patchbay session exchange returned HTTP ${response.status()}`,
    );
  }
  await page.waitForFunction(() =>
    document.cookie
      .split("; ")
      .some((entry) => entry === "patchbay_logged_in=1"),
  );
}

function observeApplicationFailures(page) {
  const failures = [];
  page.on("pageerror", (error) =>
    failures.push(`page error: ${error.message}`),
  );
  page.on("requestfailed", (request) => {
    const failure = request.failure();
    if (isExpectedBrowserRequestCancellation(failure?.errorText)) return;
    if (isFirstPartyUrl(request.url())) {
      failures.push(
        `request failed: ${request.method()} ${request.url()} (${failure?.errorText ?? "unknown"})`,
      );
    }
  });
  page.on("response", (response) => {
    if (isFirstPartyUrl(response.url()) && response.status() >= 400) {
      failures.push(`HTTP ${response.status()}: ${response.url()}`);
    }
  });
  page.on("console", (message) => {
    const text = message.text();
    if (
      message.type() === "warning" &&
      text.includes("API response failed schema validation")
    ) {
      failures.push(`schema validation warning: ${text}`);
      return;
    }
    if (message.type() !== "error") return;
    const source = message.location().url;
    if (!source || isFirstPartyUrl(source))
      failures.push(`console error: ${text}`);
  });
  return failures;
}

async function visibleReactQueryErrors(page) {
  return page.locator('[role="alert"]').evaluateAll((alerts) => {
    const summaries = [];
    const describeError = (error) => {
      if (error == null) return "null";
      if (typeof error === "string") return error;
      return `${error.name ?? error.constructor?.name ?? "Object"}: ${
        error.message ?? String(error)
      }`;
    };
    for (const alert of alerts) {
      const fiberKey = Object.keys(alert).find((key) =>
        key.startsWith("__reactFiber$"),
      );
      let fiber = fiberKey ? alert[fiberKey] : null;
      for (let fiberDepth = 0; fiber && fiberDepth < 100; fiberDepth += 1) {
        for (const branch of [fiber, fiber.alternate]) {
          const client = branch?.memoizedProps?.client;
          if (typeof client?.getQueryCache !== "function") continue;
          for (const cachedQuery of client.getQueryCache().getAll()) {
            if (!JSON.stringify(cachedQuery.queryKey).includes("dependency-graphs")) {
              continue;
            }
            const state = cachedQuery.state;
            summaries.push(
              `cache ${JSON.stringify(cachedQuery.queryKey)} status=${String(
                state.status,
              )} fetchStatus=${String(
                state.fetchStatus,
              )} error=${describeError(
                state.error,
              )} fetchFailureCount=${String(
                state.fetchFailureCount,
              )} fetchFailureReason=${describeError(state.fetchFailureReason)}`,
            );
          }
        }
        const component =
          fiber.elementType?.name ?? fiber.type?.name ?? `tag-${fiber.tag}`;
        for (const [branchName, branch] of [
          ["fiber", fiber],
          ["alternate", fiber.alternate],
        ]) {
          let hook = branch?.memoizedState;
          for (let hookDepth = 0; hook && hookDepth < 60; hookDepth += 1) {
            const value = hook.memoizedState;
            if (
              value &&
              typeof value === "object" &&
              Object.prototype.hasOwnProperty.call(value, "isError")
            ) {
              summaries.push(
                `${component}.${branchName}[${hookDepth}] status=${String(
                  value.status,
                )} fetchStatus=${String(value.fetchStatus)} isError=${String(
                  value.isError,
                )} error=${describeError(
                  value.error,
                )} failureCount=${String(
                  value.failureCount,
                )} failureReason=${describeError(value.failureReason)}`,
              );
            }
            hook = hook.next;
          }
        }
        fiber = fiber.return;
      }
    }
    return summaries;
  });
}

async function verifyProtectedRoute(
  page,
  failures,
  { path, heading, landmark, expectedBuild, verifyContent },
) {
  const failureStart = failures.length;
  const response = await page.goto(`${BASE_URL}${path}`, {
    waitUntil: "domcontentloaded",
    timeout: 30_000,
  });
  if (!response)
    throw new Error(`${path} did not return a navigation response`);
  requireProtectedNavigation({
    url: page.url(),
    status: response.status(),
    actualBuild: response.headers()["x-patchbay-build"],
    expectedBuild,
    expectedPath: path,
  });
  await expect(
    page.getByRole("heading", { name: heading, exact: true }),
  ).toBeVisible({
    timeout: 30_000,
  });
  if (landmark) {
    try {
      await expect(page.getByLabel(landmark, { exact: true })).toBeVisible({
        timeout: 30_000,
      });
    } catch (error) {
      const routeFailures = failures.slice(failureStart);
      const alerts = await page
        .getByRole("alert")
        .allTextContents()
        .catch(() => []);
      const queryErrors = await visibleReactQueryErrors(page).catch(() => []);
      const details = [
        ...routeFailures,
        ...queryErrors.map((message) => `query error: ${message}`),
        ...alerts
          .map((alert) => alert.trim())
          .filter(Boolean)
          .map((alert) => `visible alert: ${alert}`),
      ];
      if (details.length === 0) throw error;
      throw new Error(
        `${path} did not render ${landmark}:\n${details.join("\n")}`,
        { cause: error },
      );
    }
  }
  if (verifyContent) await verifyContent();
  const routeFailures = failures.slice(failureStart);
  if (routeFailures.length > 0) {
    throw new Error(
      `${path} emitted runtime failures:\n${routeFailures.join("\n")}`,
    );
  }
}

async function openBrowser() {
  const channel = process.env.PLAYWRIGHT_CHANNEL?.trim();
  const browser = await chromium.launch({
    headless: true,
    ...(channel ? { channel } : {}),
  });
  const context = await browser.newContext({ locale: "en-US" });
  await context.addCookies([
    {
      name: "patchbay-locale",
      value: "en",
      domain: "patchbay.aspectlylabs.com",
      path: "/",
      secure: true,
      sameSite: "Lax",
    },
  ]);
  return { browser, context, page: await context.newPage() };
}

async function verifyGoogleOAuthStart(browser) {
  const context = await browser.newContext({ locale: "en-US" });
  const page = await context.newPage();
  const codeChallenge = randomBytes(32).toString("base64url");
  const state = randomBytes(32).toString("base64url");
  const entryUrl = buildGoogleOAuthProbeUrl({ codeChallenge, state });
  const attemptResponse = page.waitForResponse(
    (response) => {
      const url = new URL(response.url());
      return (
        url.origin === ACCOUNTS_BASE_URL &&
        url.pathname === "/v1/desktop/google/attempt" &&
        response.request().method() === "POST"
      );
    },
    { timeout: 30_000 },
  );
  const downstreamNavigation = page.waitForURL(
    (url) =>
      url.protocol === "https:" && url.hostname === "accounts.google.com",
    { timeout: 30_000 },
  );

  try {
    await setupClerkTestingToken({ context });
    await page.goto(entryUrl, {
      waitUntil: "domcontentloaded",
      timeout: 30_000,
    });
    const response = await attemptResponse;
    if (response.status() !== 200) {
      throw new Error(
        `Google OAuth handoff registration returned HTTP ${response.status()}`,
      );
    }
    const attempt = await response.json().catch(() => null);
    if (attempt?.registered !== true) {
      throw new Error(
        "Google OAuth handoff registration returned an invalid response",
      );
    }
    // Playwright's waitForURL resolves to the navigation response (or null),
    // not the URL object passed to its predicate. Read the settled page URL
    // after the navigation so the OAuth assertion checks the actual browser
    // location instead of dereferencing an undefined response property.
    await downstreamNavigation;
    requireGoogleOAuthNavigation(page.url());
  } catch (error) {
    await page
      .screenshot({ path: SCREENSHOT_PATH, fullPage: true })
      .catch(() => {});
    throw error;
  } finally {
    await context.close();
  }
}

export async function verifyProductionBrowser(sourceSha, receipt) {
  const { signInTicket, testingToken } = requireBrowserReceipt(
    receipt,
    sourceSha,
  );
  const expectedBuild = `sha-${sourceSha}`;
  process.env.CLERK_TESTING_TOKEN = testingToken;
  process.env.CLERK_FAPI = decodeClerkFrontendApi(
    process.env.CLERK_PUBLISHABLE_KEY,
  );

  const { browser, context, page } = await openBrowser();
  try {
    await verifyGoogleOAuthStart(browser);
    await setupClerkTestingToken({ context });
    await page.goto(`${BASE_URL}/login`, { waitUntil: "domcontentloaded" });
    await clerk.loaded({ page });
    await waitForPatchbayExchange(page, () => ticketSignIn(page, signInTicket));

    const workspace = await ensureSmokeWorkspace(page);
    assert.equal(workspace.slug, SMOKE_WORKSPACE);
    const smokeGraph = await ensureSmokeDependencyGraph(page, workspace);

    const failures = observeApplicationFailures(page);
    await verifyProtectedRoute(page, failures, {
      path: `/${SMOKE_WORKSPACE}/issues`,
      heading: "Issues",
      expectedBuild,
    });
    await verifyProtectedRoute(page, failures, {
      path: `/${SMOKE_WORKSPACE}/task-graph`,
      heading: "Task Graph",
      landmark: "Dependency graph canvas",
      expectedBuild,
      verifyContent: () =>
        verifyProductionSmokeTaskGraph(page, smokeGraph.graph),
    });
  } catch (error) {
    await page
      .screenshot({ path: SCREENSHOT_PATH, fullPage: true })
      .catch(() => {});
    throw error;
  } finally {
    await browser.close();
  }
}

async function createSmokeClerkUser(secretKey) {
  const response = await fetch("https://api.clerk.com/v1/users", {
    method: "POST",
    headers: {
      authorization: `Bearer ${secretKey}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      email_address: [SMOKE_EMAIL],
      first_name: "Production",
      last_name: "Smoke",
      // This production Clerk instance requires a password at user creation.
      // The verifier never stores or uses it: every run signs in through a
      // single-use Backend API ticket instead.
      password: `${randomBytes(32).toString("base64url")}!Aa9`,
      private_metadata: { purpose: "production-browser-acceptance" },
    }),
  });
  if (response.ok) return "created";
  const body = await response.json().catch(() => ({}));
  const codes = Array.isArray(body.errors)
    ? body.errors.map((error) => error?.code).filter(Boolean)
    : [];
  if (response.status === 422 && codes.includes("form_identifier_exists")) {
    return "existing";
  }
  throw new Error(
    `failed to provision Clerk smoke user (HTTP ${response.status})`,
  );
}

async function clerkApiRequest(secretKey, path, { method = "GET", body } = {}) {
  const response = await fetch(`https://api.clerk.com/v1/${path}`, {
    method,
    headers: {
      authorization: `Bearer ${secretKey}`,
      accept: "application/json",
      ...(body ? { "content-type": "application/json" } : {}),
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  const payload = await response.json().catch(() => null);
  if (!response.ok) {
    throw new Error(`Clerk API request returned HTTP ${response.status}`);
  }
  return payload;
}

async function smokeUserId(secretKey) {
  const query = new URLSearchParams({
    email_address: SMOKE_EMAIL,
    limit: "2",
  });
  const value = await clerkApiRequest(secretKey, `users?${query}`);
  const users = Array.isArray(value) ? value : value?.data;
  if (!Array.isArray(users)) {
    throw new Error("Clerk returned an invalid smoke-user list");
  }
  const exact = users.filter((user) =>
    user?.email_addresses?.some(
      (address) => address?.email_address === SMOKE_EMAIL,
    ),
  );
  if (exact.length !== 1 || typeof exact[0]?.id !== "string") {
    throw new Error(
      "the dedicated production browser-acceptance Clerk user is missing or ambiguous",
    );
  }
  return exact[0].id;
}

async function createSmokeSignInTicket(secretKey) {
  const userId = await smokeUserId(secretKey);
  const value = await clerkApiRequest(secretKey, "sign_in_tokens", {
    method: "POST",
    body: { user_id: userId, expires_in_seconds: 300 },
  });
  return requiredString(value?.token, "browser sign-in ticket");
}

async function createTestingToken(secretKey) {
  const value = await clerkApiRequest(secretKey, "testing_tokens", {
    method: "POST",
    body: {},
  });
  return requiredString(value?.token, "Clerk testing token");
}

async function authenticatedBrowserResponse(page, path, init = {}) {
  return page.evaluate(
    async ({ requestPath, requestInit }) => {
      const csrfToken = document.cookie
        .split("; ")
        .find((entry) => entry.startsWith("patchbay_csrf="))
        ?.split("=")
        .slice(1)
        .join("=");
      if (!csrfToken) {
        throw new Error("Patchbay session did not issue a readable CSRF token");
      }
      const response = await fetch(requestPath, {
        ...requestInit,
        credentials: "include",
        headers: {
          "content-type": "application/json",
          "x-csrf-token": csrfToken,
          ...requestInit.headers,
        },
      });
      return {
        ok: response.ok,
        status: response.status,
        body: await response.json().catch(() => ({})),
      };
    },
    { requestPath: path, requestInit: init },
  );
}

function requireAuthenticatedBrowserResponse(path, response) {
  if (response?.ok === true) return response.body;
  const detail =
    typeof response?.body?.message === "string"
      ? response.body.message
      : typeof response?.body?.error === "string"
        ? response.body.error
        : "";
  const message = detail ? `: ${detail}` : "";
  throw new Error(
    `${path} returned HTTP ${response?.status ?? "unknown"}${message}`,
  );
}

async function authenticatedBrowserRequest(page, path, init = {}) {
  const response = await authenticatedBrowserResponse(page, path, init);
  return requireAuthenticatedBrowserResponse(path, response);
}

async function ensureSmokeWorkspace(page) {
  const workspaces = await authenticatedBrowserRequest(page, "/api/workspaces");
  if (!Array.isArray(workspaces)) {
    throw new Error("workspace API returned an invalid response");
  }
  let workspace = workspaces.find(
    (candidate) => candidate?.slug === SMOKE_WORKSPACE,
  );
  let created = false;
  if (!workspace) {
    workspace = await authenticatedBrowserRequest(page, "/api/workspaces", {
      method: "POST",
      body: JSON.stringify({
        name: "Production Smoke",
        slug: SMOKE_WORKSPACE,
        issue_prefix: "SMOKE",
      }),
    });
    created = true;
  }
  const id = requiredString(workspace?.id, "production smoke workspace id");
  const slug = requiredString(
    workspace?.slug,
    "production smoke workspace slug",
  );
  await authenticatedBrowserRequest(page, "/api/me/onboarding/complete", {
    method: "POST",
    body: JSON.stringify({
      completion_path: "skip_existing",
      workspace_id: id,
    }),
  });
  return { created, id, slug };
}

async function ensureSmokeDependencyGraph(page, workspace) {
  const workspaceHeaders = { "x-workspace-slug": workspace.slug };
  const existing = await findProductionSmokeGraph(async (cursor) => {
    const query = new URLSearchParams({ limit: "64" });
    if (cursor) query.set("cursor", cursor);
    return authenticatedBrowserRequest(
      page,
      `/api/dependency-graphs?${query}`,
      { headers: workspaceHeaders },
    );
  });
  if (existing) {
    requireProductionSmokeGraphContract(existing);
    return { created: false, graph: existing };
  }

  const policies = await authenticatedBrowserRequest(
    page,
    "/api/issue-category-policies",
    { headers: workspaceHeaders },
  );
  requireNoDefaultExecutionAgent(policies);

  const parent = await ensureSmokeGraphParentIssue(page, workspaceHeaders);
  const parentIssueId = requiredString(
    parent?.id,
    "production smoke graph parent issue id",
  );
  const graph = await authenticatedBrowserRequest(
    page,
    `/api/issues/${encodeURIComponent(parentIssueId)}/dependency-graph/apply`,
    {
      method: "POST",
      headers: {
        ...workspaceHeaders,
        "idempotency-key": buildProductionSmokeGraphIdempotencyKey(
          parentIssueId,
          randomBytes(16).toString("hex"),
        ),
      },
      body: JSON.stringify(buildProductionSmokeDependencyPlan(parentIssueId)),
    },
  );
  requireProductionSmokeGraphContract(graph);
  return { created: true, graph };
}

async function findSmokeGraphParentIssue(page, workspaceHeaders) {
  const query = new URLSearchParams({
    q: PRODUCTION_SMOKE_PARENT_TITLE,
    top_level_only: "true",
    open_only: "true",
    limit: "100",
  });
  const response = await authenticatedBrowserRequest(
    page,
    `/api/issues?${query}`,
    { headers: workspaceHeaders },
  );
  return findProductionSmokeParentIssue(response);
}

async function ensureSmokeGraphParentIssue(page, workspaceHeaders) {
  const existing = await findSmokeGraphParentIssue(page, workspaceHeaders);
  if (existing) return existing;

  const path = "/api/issues";
  const response = await authenticatedBrowserResponse(page, path, {
    method: "POST",
    headers: workspaceHeaders,
    body: JSON.stringify({
      title: PRODUCTION_SMOKE_PARENT_TITLE,
      description:
        "Stable parent issue for automated production task graph acceptance.",
      status: "todo",
      priority: "none",
    }),
  });
  if (response.ok) return response.body;
  if (response.status === 409) {
    const racedParent = await findSmokeGraphParentIssue(page, workspaceHeaders);
    if (racedParent) return racedParent;
  }
  return requireAuthenticatedBrowserResponse(path, response);
}

async function verifyProductionSmokeTaskGraph(page, graph) {
  const contract = requireProductionSmokeGraphContract(graph);
  const nodes = page.locator("[data-graph-node]");
  const edges = page.locator(
    '[data-graph-edge][data-edge-state="blocked"], [data-graph-edge][data-edge-state="satisfied"]',
  );
  requireProductionSmokeGraph({
    nodeCount: await nodes.count(),
    edgeCount: await edges.count(),
  });
  for (const edge of contract.edges) {
    const accessibleName = `Dependency from ${edge.fromIdentifier} to ${edge.toIdentifier} — ${edge.stateLabel}`;
    const renderedEdge = page.getByRole("button", {
      name: accessibleName,
      exact: true,
    });
    await expect(renderedEdge).toHaveCount(1, { timeout: 30_000 });
    await expect(renderedEdge).toHaveAttribute("data-edge-state", edge.state, {
      timeout: 30_000,
    });
  }
  const dependentNode = nodes.filter({
    hasText: PRODUCTION_SMOKE_DEPENDENT_TASK_TITLE,
  });
  await expect(dependentNode).toHaveCount(1, { timeout: 30_000 });
  await expect(dependentNode.first()).toBeVisible({ timeout: 30_000 });
  await expect(dependentNode.first()).toHaveAttribute(
    "href",
    `/${SMOKE_WORKSPACE}/issues/${encodeURIComponent(contract.dependentIdentifier)}`,
  );
  await dependentNode.first().click();
  await expect(
    page.getByText(PRODUCTION_SMOKE_DEPENDENT_ACCEPTANCE, { exact: true }),
  ).toBeVisible({ timeout: 30_000 });
}

async function provisionProductionSmokeFixture() {
  const raw = await new Promise((resolve, reject) => {
    let value = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (chunk) => {
      value += chunk;
      if (value.length > 65_536)
        reject(new Error("credential input exceeds 64 KiB"));
    });
    process.stdin.on("end", () => resolve(value));
    process.stdin.on("error", reject);
  });
  const credentials = JSON.parse(raw);
  process.env.CLERK_SECRET_KEY = requiredString(
    credentials.clerk_secret_key,
    "clerk_secret_key",
  );
  process.env.CLERK_PUBLISHABLE_KEY = requiredString(
    credentials.clerk_publishable_key,
    "clerk_publishable_key",
  );
  const userState = await createSmokeClerkUser(process.env.CLERK_SECRET_KEY);
  process.env.CLERK_FAPI = decodeClerkFrontendApi(
    process.env.CLERK_PUBLISHABLE_KEY,
  );
  process.env.CLERK_TESTING_TOKEN = await createTestingToken(
    process.env.CLERK_SECRET_KEY,
  );
  const signInTicket = await createSmokeSignInTicket(
    process.env.CLERK_SECRET_KEY,
  );

  const { browser, context, page } = await openBrowser();
  try {
    await setupClerkTestingToken({ context });
    await page.goto(`${BASE_URL}/login`, { waitUntil: "domcontentloaded" });
    await clerk.loaded({ page });
    await waitForPatchbayExchange(page, () => ticketSignIn(page, signInTicket));
    const workspace = await ensureSmokeWorkspace(page);
    assert.equal(workspace.slug, SMOKE_WORKSPACE);
    const graph = await ensureSmokeDependencyGraph(page, workspace);
    return {
      userState,
      workspaceState: workspace.created ? "created" : "existing",
      graphState: graph.created ? "created" : "existing",
    };
  } finally {
    await browser.close();
  }
}

async function main() {
  const [first, second] = process.argv.slice(2);
  if (first === "--provision") {
    const result = await provisionProductionSmokeFixture();
    console.log(
      `production browser fixture ready (user: ${result.userState}, workspace: ${result.workspaceState}, graph: ${result.graphState})`,
    );
    return;
  }
  if (!first || !second || !SLUG_PATTERN.test(SMOKE_WORKSPACE)) {
    throw new Error(
      "usage: verify-production-browser.mjs <source-sha> <deployment-receipt.json>",
    );
  }
  const receipt = JSON.parse(await readFile(second, "utf8"));
  await verifyProductionBrowser(first, receipt);
  console.log(
    "production Google OAuth start, Issues, and Task Graph browser acceptance passed",
  );
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}
