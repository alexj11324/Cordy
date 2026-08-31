import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

import { chromium, expect } from "@playwright/test";
import { clerk, setupClerkTestingToken } from "@clerk/testing/playwright";

import {
  buildGoogleOAuthProbeUrl,
  decodeClerkFrontendApi,
  isExpectedBrowserRequestCancellation,
  requiredString,
  requireBrowserReceipt,
  requireGoogleOAuthNavigation,
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
    if (message.type() !== "error") return;
    const source = message.location().url;
    if (!source || isFirstPartyUrl(source))
      failures.push(`console error: ${message.text()}`);
  });
  return failures;
}

async function verifyProtectedRoute(
  page,
  failures,
  { path, heading, landmark, emptyState, expectedBuild },
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
    const landmarkLocator = page.getByLabel(landmark, { exact: true });
    const surface = emptyState
      ? landmarkLocator.or(page.getByText(emptyState, { exact: true }))
      : landmarkLocator;
    await expect(surface).toBeVisible({ timeout: 30_000 });
  }
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
      emptyState: "No active dependency graphs",
      expectedBuild,
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

async function ensureSmokeWorkspace(page) {
  return page.evaluate(
    async ({ workspaceSlug }) => {
      const csrfToken = document.cookie
        .split("; ")
        .find((entry) => entry.startsWith("patchbay_csrf="))
        ?.split("=")
        .slice(1)
        .join("=");
      if (!csrfToken) {
        throw new Error("Patchbay session did not issue a readable CSRF token");
      }
      const request = async (path, init) => {
        const response = await fetch(path, {
          ...init,
          credentials: "include",
          headers: {
            "content-type": "application/json",
            "x-csrf-token": csrfToken,
            ...init?.headers,
          },
        });
        if (!response.ok) {
          const body = await response.json().catch(() => ({}));
          const detail =
            typeof body.message === "string"
              ? body.message
              : typeof body.error === "string"
                ? body.error
                : "";
          const message = detail ? `: ${detail}` : "";
          throw new Error(`${path} returned HTTP ${response.status}${message}`);
        }
        return response.json();
      };
      const workspaces = await request("/api/workspaces");
      let workspace = workspaces.find(
        (candidate) => candidate.slug === workspaceSlug,
      );
      let created = false;
      if (!workspace) {
        workspace = await request("/api/workspaces", {
          method: "POST",
          body: JSON.stringify({
            name: "Production Smoke",
            slug: workspaceSlug,
            issue_prefix: "SMOKE",
          }),
        });
        created = true;
      }
      await request("/api/me/onboarding/complete", {
        method: "POST",
        body: JSON.stringify({
          completion_path: "skip_existing",
          workspace_id: workspace.id,
        }),
      });
      return { created, slug: workspace.slug };
    },
    { workspaceSlug: SMOKE_WORKSPACE },
  );
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
    return {
      userState,
      workspaceState: workspace.created ? "created" : "existing",
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
      `production browser fixture ready (user: ${result.userState}, workspace: ${result.workspaceState})`,
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
