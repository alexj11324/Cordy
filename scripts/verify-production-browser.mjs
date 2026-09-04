import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { chromium, expect } from "@playwright/test";
import {
  clerk,
  clerkSetup,
  setupClerkTestingToken,
} from "@clerk/testing/playwright";

import {
  ACCOUNTS_ORIGIN,
  API_ORIGIN,
  buildAccountsLoginProbeUrl,
  buildGoogleOAuthProbeUrl,
  buildPkceChallenge,
  PRODUCT_ORIGIN,
  requireBrowserReceipt,
  requireBuildHeaders,
  requireClerkPublishableKey,
  requireDesktopCompletion,
  requireGoogleOAuthNavigation,
  requireRedeemedSession,
} from "./verify-production-browser-contract.mjs";

const SCREENSHOT_PATH = path.join(
  process.env.RUNNER_TEMP ?? process.env.TMPDIR ?? ".",
  "production-browser-failure.png",
);

async function verifyAccountsLoginSurface(browser, sourceSha) {
  const context = await browser.newContext({
    locale: "en-US",
    viewport: { width: 1440, height: 900 },
  });
  const page = await context.newPage();
  const state = randomBytes(32).toString("base64url");
  const codeChallenge = randomBytes(32).toString("base64url");
  try {
    const publicHandoff = await context.request.post(
      `${API_ORIGIN}/api/desktop-handoff/initiate`,
      {
        data: {
          state,
          code_challenge: codeChallenge,
          callback_protocol: "patchbay",
        },
      },
    );
    assert.equal(
      publicHandoff.status(),
      200,
      "public desktop handoff registration",
    );

    const brokerRegistration = page.waitForResponse((response) => {
      const url = new URL(response.url());
      return (
        url.origin === ACCOUNTS_ORIGIN &&
        url.pathname === "/v1/desktop/google/attempt" &&
        response.request().method() === "POST"
      );
    });
    const response = await page.goto(
      buildAccountsLoginProbeUrl({ codeChallenge, state }),
      { waitUntil: "domcontentloaded" },
    );
    assert.ok(response, "Accounts login page must return a response");
    assert.equal(response.status(), 200, "Accounts login page status");
    requireBuildHeaders(response.headers(), sourceSha, "Accounts login");
    assert.equal(
      (await brokerRegistration).status(),
      200,
      "Accounts desktop attempt registration",
    );

    await clerk.loaded({ page });
    const authShell = page.getByTestId("accounts-auth-shell");
    const formPanel = page.getByTestId("accounts-auth-form-panel");
    const brandPanel = page.getByTestId("accounts-auth-brand-panel");
    await expect(authShell).toBeVisible();
    await expect(formPanel).toBeVisible();
    await expect(brandPanel).toBeVisible();
    await expect(page.getByTestId("patchbay-mark")).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Create an account", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Sign In with Email", exact: true }),
    ).toBeVisible();

    const [shellBox, formBox, brandBox] = await Promise.all([
      authShell.boundingBox(),
      formPanel.boundingBox(),
      brandPanel.boundingBox(),
    ]);
    assert.ok(shellBox, "Accounts split shell must have a rendered box");
    assert.ok(formBox, "Accounts black form panel must have a rendered box");
    assert.ok(brandBox, "Accounts charcoal brand panel must have a rendered box");
    assert.ok(
      brandBox.x < formBox.x &&
        formBox.width >= shellBox.width * 0.45 &&
        brandBox.width >= shellBox.width * 0.45,
      "Accounts login must render charcoal-left and black-right panels",
    );
  } finally {
    await context.close();
  }
}

async function verifyStandaloneAccountsLoginSurface(browser, sourceSha) {
  const context = await browser.newContext({
    locale: "en-US",
    viewport: { width: 1440, height: 900 },
  });
  const page = await context.newPage();
  try {
    const response = await page.goto(`${ACCOUNTS_ORIGIN}/login`, {
      waitUntil: "domcontentloaded",
    });
    assert.ok(response, "standalone Accounts login page must return a response");
    assert.equal(response.status(), 200, "standalone Accounts login page status");
    requireBuildHeaders(response.headers(), sourceSha, "standalone Accounts login");
    await clerk.loaded({ page });
    await expect(page.getByTestId("accounts-auth-shell")).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Create an account", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Continue with Google", exact: true }),
    ).toBeVisible();
  } finally {
    await context.close();
  }
}

async function verifyGoogleOAuthStart(browser) {
  const context = await browser.newContext({ locale: "en-US" });
  const page = await context.newPage();
  const state = randomBytes(32).toString("base64url");
  const codeChallenge = randomBytes(32).toString("base64url");
  const entryUrl = buildGoogleOAuthProbeUrl({ codeChallenge, state });
  const registration = page.waitForResponse((response) => {
    const url = new URL(response.url());
    return (
      url.origin === ACCOUNTS_ORIGIN &&
      url.pathname === "/v1/desktop/google/attempt" &&
      response.request().method() === "POST"
    );
  });
  const google = page.waitForURL(
    (url) => url.protocol === "https:" && url.hostname === "accounts.google.com",
  );
  try {
    await page.goto(entryUrl, { waitUntil: "domcontentloaded" });
    const response = await registration;
    assert.equal(response.status(), 200, "desktop OAuth attempt registration");
    await google;
    requireGoogleOAuthNavigation(page.url());
  } finally {
    await context.close();
  }
}

async function redeemSyntheticLogin(browser, credentials, publishableKey) {
  const verifier = randomBytes(32).toString("base64url");
  const state = randomBytes(32).toString("base64url");
  const codeChallenge = buildPkceChallenge(verifier);
  const query = new URLSearchParams({
    platform: "desktop",
    state,
    code_challenge: codeChallenge,
  }).toString();
  const context = await browser.newContext({ locale: "en-US" });
  // Publish the deployment-issued testing token first so clerkSetup adopts it
  // instead of reaching for a Clerk secret key: the runner deliberately never
  // holds one. clerkSetup's only remaining job here is resolving CLERK_FAPI
  // from the publishable key, which setupClerkTestingToken requires.
  process.env.CLERK_TESTING_TOKEN = credentials.testingToken;
  await clerkSetup({ publishableKey, dotenv: false });
  await setupClerkTestingToken({ context });
  const page = await context.newPage();
  try {
    const registered = await context.request.post(
      `${ACCOUNTS_ORIGIN}/v1/desktop/google/attempt`,
      {
        data: { state, code_challenge: codeChallenge },
        headers: { "x-patchbay-auth-contract-version": "1" },
      },
    );
    assert.equal(registered.status(), 200, "desktop login attempt registration");

    await page.goto(`${ACCOUNTS_ORIGIN}/oauth/google/callback?${query}`, {
      waitUntil: "domcontentloaded",
    });
    await clerk.loaded({ page });
    const signedIn = await page.evaluate(async (ticket) => {
      const attempt = await window.Clerk.client.signIn.create({
        strategy: "ticket",
        ticket,
      });
      if (attempt.status !== "complete" || !attempt.createdSessionId) {
        return false;
      }
      await window.Clerk.setActive({ session: attempt.createdSessionId });
      return true;
    }, credentials.signInTicket);
    assert.equal(signedIn, true, "synthetic Clerk ticket sign-in");

    // This is the browser-side identity boundary: the Web app obtains the
    // active Clerk token, exchanges it at Go /auth/clerk, and only then can
    // authenticated frontend requests use the Patchbay session cookie.
    const clerkExchangePromise = page.waitForResponse((response) => {
      const url = new URL(response.url());
      return (
        (url.origin === PRODUCT_ORIGIN || url.origin === API_ORIGIN) &&
        url.pathname === "/auth/clerk" &&
        response.request().method() === "POST"
      );
    });
    await page.goto(`${PRODUCT_ORIGIN}/login`, {
      waitUntil: "domcontentloaded",
    });
    await clerk.loaded({ page });
    const clerkExchange = await clerkExchangePromise;
    assert.equal(clerkExchange.status(), 200, "Web Clerk session exchange");
    const clerkPayload = await clerkExchange.json();
    assert.equal(clerkPayload?.user?.is_guest, false, "Web session is formal");

    const completionPromise = page.waitForResponse((response) => {
      const url = new URL(response.url());
      return (
        url.origin === ACCOUNTS_ORIGIN &&
        url.pathname === "/v1/desktop/google/complete" &&
        response.request().method() === "POST"
      );
    });
    await page.goto(`${ACCOUNTS_ORIGIN}/login?${query}`, {
      waitUntil: "domcontentloaded",
    });
    const completion = await completionPromise;
    assert.equal(completion.status(), 200, "desktop login completion");
    const code = requireDesktopCompletion(await completion.json());

    const redeemed = await fetch(`${API_ORIGIN}/api/desktop-handoff/redeem`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ code, code_verifier: verifier }),
      redirect: "error",
    });
    assert.equal(redeemed.status, 200, "one-time desktop handoff redemption");
    return requireRedeemedSession(await redeemed.json());
  } finally {
    await context.close();
  }
}

async function verifyAuthenticatedProduct(browser, sourceSha, token) {
  const workspacesResponse = await fetch(`${API_ORIGIN}/api/workspaces`, {
    headers: { authorization: `Bearer ${token}` },
    redirect: "error",
  });
  assert.equal(workspacesResponse.status, 200, "authenticated workspace list");
  const workspaces = await workspacesResponse.json();
  assert.ok(Array.isArray(workspaces), "workspace API must return an array");
  const target =
    typeof workspaces[0]?.slug === "string"
      ? `/${workspaces[0].slug}/issues`
      : "/workspaces/new";

  const publicContext = await browser.newContext({ locale: "en-US" });
  const publicPage = await publicContext.newPage();
  try {
    const landing = await publicPage.goto(PRODUCT_ORIGIN, {
      waitUntil: "domcontentloaded",
    });
    assert.ok(landing, "product landing page must return a response");
    assert.equal(landing.status(), 200, "product landing page status");
    requireBuildHeaders(landing.headers(), sourceSha, "Web landing page");
    const loginLink = publicPage.locator('a[href="/login"]').first();
    await expect(loginLink).toBeVisible();
    await loginLink.click();
    await publicPage.waitForURL((url) => {
      return url.origin === PRODUCT_ORIGIN && url.pathname === "/login";
    });

    const login = await publicPage.goto(`${PRODUCT_ORIGIN}/login`, {
      waitUntil: "domcontentloaded",
    });
    assert.ok(login, "login page must return a response");
    assert.equal(login.status(), 200, "login page status");
    requireBuildHeaders(login.headers(), sourceSha, "Web login");
    await expect(
      publicPage.getByRole("heading", {
        name: "Sign in to Patchbay",
        exact: true,
      }),
    ).toBeVisible();
    const authShell = publicPage.getByTestId("clerk-auth-shell");
    const formPanel = authShell.locator(":scope > section");
    const brandPanel = publicPage.getByTestId("clerk-auth-brand-panel");
    await expect(authShell).toHaveClass(/\bbg-white\b/u);
    await expect(authShell).toHaveClass(/\bmd:grid-cols-2\b/u);
    await expect(formPanel).toBeVisible();
    await expect(formPanel).toHaveClass(/\bbg-white\b/u);
    await expect(brandPanel).toBeVisible();
    await expect(brandPanel).toHaveClass(/\bbg-zinc-950\b/u);

    const [shellBox, formBox, brandBox] = await Promise.all([
      authShell.boundingBox(),
      formPanel.boundingBox(),
      brandPanel.boundingBox(),
    ]);
    assert.ok(shellBox, "split login shell must have a rendered box");
    assert.ok(formBox, "white login form panel must have a rendered box");
    assert.ok(brandBox, "black login brand panel must have a rendered box");
    assert.ok(
      formBox.width >= shellBox.width * 0.45 &&
        brandBox.width >= shellBox.width * 0.45,
      "login must render as white-left / black-right split panels",
    );
  } finally {
    await publicContext.close();
  }

  const context = await browser.newContext({ locale: "en-US" });
  await context.addInitScript((session) => {
    window.localStorage.setItem("patchbay_token", session);
  }, token);
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
  const page = await context.newPage();
  try {
    const protectedResponse = await page.goto(`${PRODUCT_ORIGIN}${target}`, {
      waitUntil: "domcontentloaded",
    });
    assert.ok(protectedResponse, "protected page must return a response");
    assert.equal(protectedResponse.status(), 200, "protected page status");
    requireBuildHeaders(
      protectedResponse.headers(),
      sourceSha,
      "authenticated Web",
    );
    await page.waitForURL((url) => url.origin === PRODUCT_ORIGIN);
    assert.notEqual(new URL(page.url()).pathname, "/login");
    await expect(page.locator("main")).toBeVisible();
  } catch (error) {
    await page.screenshot({ path: SCREENSHOT_PATH, fullPage: true }).catch(() => {});
    throw error;
  } finally {
    await context.close();
  }
}

export async function verifyProductionBrowser(sourceSha, receipt) {
  const credentials = requireBrowserReceipt(receipt, sourceSha);
  const publishableKey = requireClerkPublishableKey(
    process.env.CLERK_PUBLISHABLE_KEY,
  );
  const browser = await chromium.launch({ headless: true });
  try {
    await verifyStandaloneAccountsLoginSurface(browser, sourceSha);
    await verifyAccountsLoginSurface(browser, sourceSha);
    await verifyGoogleOAuthStart(browser);
    const token = await redeemSyntheticLogin(
      browser,
      credentials,
      publishableKey,
    );
    await verifyAuthenticatedProduct(browser, sourceSha, token);
  } finally {
    await browser.close();
  }
}

async function main() {
  const [sourceSha, receiptPath] = process.argv.slice(2);
  if (!sourceSha || !receiptPath) {
    throw new Error(
      "usage: verify-production-browser.mjs <source-sha> <deployment-receipt.json>",
    );
  }
  const receipt = JSON.parse(await readFile(receiptPath, "utf8"));
  await verifyProductionBrowser(sourceSha, receipt);
  console.log(
    "production Accounts login UI, Google OAuth start, one-time broker login, and authenticated Web acceptance passed",
  );
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}
