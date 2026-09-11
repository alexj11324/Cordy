import assert from "node:assert/strict";
import test from "node:test";

import {
  ACCOUNTS_ORIGIN,
  API_ORIGIN,
  buildAccountsLoginProbeUrl,
  buildGoogleOAuthProbeUrl,
  buildPkceChallenge,
  DESKTOP_CALLBACK_PROTOCOL,
  hostedBrowserOrigins,
  PRODUCT_COOKIE_DOMAIN,
  PRODUCT_ORIGIN,
  requireBrowserReceipt,
  requireBuildHeaders,
  requireClerkPublishableKey,
  requireDesktopCompletion,
  requireGoogleOAuthNavigation,
  requireRedeemedSession,
} from "./verify-production-browser-contract.mjs";

const SOURCE_SHA = "a".repeat(40);

test("uses only the three public Aspectly Labs product origins", () => {
  assert.equal(API_ORIGIN, "https://api.aspectlylabs.com");
  assert.equal(PRODUCT_ORIGIN, "https://orvilo.aspectlylabs.com");
  assert.equal(ACCOUNTS_ORIGIN, "https://accounts.aspectlylabs.com");
  assert.equal(PRODUCT_COOKIE_DOMAIN, "orvilo.aspectlylabs.com");
  assert.equal(DESKTOP_CALLBACK_PROTOCOL, "orvilo");
});

test("staging browser origins stay off the public product hosts", () => {
  const staging = hostedBrowserOrigins("staging");
  assert.equal(staging.product, "https://staging.aspectlylabs.com");
  assert.equal(staging.api, "https://api.staging.aspectlylabs.com");
  assert.equal(staging.accounts, "https://accounts.staging.aspectlylabs.com");
  assert.equal(staging.cookieDomain, "staging.aspectlylabs.com");
  assert.equal(
    staging.desktopCallbackProtocol,
    "orvilo-staging-aaaaaaaaaaaaaaaa",
  );
  assert.match(
    staging.desktopCallbackProtocol,
    /^orvilo-staging-[a-f0-9]{16}$/u,
  );
  assert.notEqual(staging.cookieDomain, PRODUCT_COOKIE_DOMAIN);
  assert.notEqual(staging.desktopCallbackProtocol, DESKTOP_CALLBACK_PROTOCOL);
  assert.throws(
    () => hostedBrowserOrigins("canary"),
    /unsupported browser verification environment/u,
  );
});

test("builds a PKCE-bound direct Accounts Google OAuth entry", () => {
  const verifier = "v".repeat(43);
  const challenge = buildPkceChallenge(verifier);
  const url = new URL(
    buildGoogleOAuthProbeUrl({
      codeChallenge: challenge,
      state: "s".repeat(43),
    }),
  );
  assert.equal(url.origin, ACCOUNTS_ORIGIN);
  assert.equal(url.pathname, "/oauth/google");
  assert.equal(url.searchParams.get("platform"), "desktop");
  assert.equal(url.searchParams.get("code_challenge"), challenge);
  assert.equal(url.searchParams.get("state"), "s".repeat(43));
  assert.equal(url.searchParams.has("app_origin"), false);
  assert.equal(url.href.includes("localhost"), false);
});

test("builds the PKCE-bound Accounts login page used by Desktop", () => {
  const url = new URL(
    buildAccountsLoginProbeUrl({
      codeChallenge: "c".repeat(43),
      state: "s".repeat(43),
    }),
  );
  assert.equal(url.origin, ACCOUNTS_ORIGIN);
  assert.equal(url.pathname, "/login");
  assert.equal(url.searchParams.get("platform"), "desktop");
  assert.equal(url.searchParams.has("app_origin"), false);
});

test("production browser acceptance includes the standalone broker and Go Clerk exchange", async () => {
  const source = await import("node:fs/promises").then(({ readFile }) =>
    readFile(
      new URL("./verify-production-browser.mjs", import.meta.url),
      "utf8",
    ),
  );
  assert.match(source, /ACCOUNTS_ORIGIN\}\/login`/u);
  assert.match(source, /url\.pathname === "\/auth\/clerk"/u);
  assert.match(source, /Web Clerk session exchange/u);
  assert.match(source, /user\?\.is_guest/u);
  assert.match(
    source,
    /headers: \{\s*origin: ACCOUNTS_ORIGIN,\s*"x-orvilo-auth-contract-version": "1",\s*\}/u,
  );
});

test("production browser acceptance treats root as an app entry", async () => {
  const source = await import("node:fs/promises").then(({ readFile }) =>
    readFile(
      new URL("./verify-production-browser.mjs", import.meta.url),
      "utf8",
    ),
  );
  assert.match(source, /Web app entry/u);
  assert.match(source, /url\.pathname === "\/login"/u);
  assert.match(source, /url\.pathname === target/u);
  assert.doesNotMatch(source, /a\[href="\/login"\]/u);
  assert.doesNotMatch(source, /product landing page/u);
});

test("authenticated Web acceptance carries the real cookie session", async () => {
  const source = await import("node:fs/promises").then(({ readFile }) =>
    readFile(
      new URL("./verify-production-browser.mjs", import.meta.url),
      "utf8",
    ),
  );
  assert.match(source, /storageState: await context\.storageState\(\)/u);
  assert.match(source, /storageState: auth\.storageState/u);
  assert.doesNotMatch(source, /localStorage\.setItem\("orvilo_token"/u);
});

test("requires Google and not a lookalike OAuth destination", () => {
  assert.equal(
    requireGoogleOAuthNavigation("https://accounts.google.com/o/oauth2/auth")
      .hostname,
    "accounts.google.com",
  );
  assert.throws(
    () => requireGoogleOAuthNavigation("https://accounts.google.example/auth"),
    /did not reach accounts\.google\.com/u,
  );
});

test("requires matching build and commit headers", () => {
  const headers = new Headers({
    "x-orvilo-build": `sha-${SOURCE_SHA}`,
    "x-orvilo-commit": SOURCE_SHA,
  });
  assert.doesNotThrow(() =>
    requireBuildHeaders(headers, SOURCE_SHA, "runtime"),
  );
  headers.set("x-orvilo-commit", "b".repeat(40));
  assert.throws(
    () => requireBuildHeaders(headers, SOURCE_SHA, "runtime"),
    /reported commit/u,
  );
});

test("accepts credentials only from the matching deployment receipt", () => {
  const receipt = {
    ok: true,
    action: "deploy",
    source_sha: SOURCE_SHA,
    browser_auth: {
      sign_in_ticket: "ticket",
      testing_token: "testing",
    },
  };
  assert.deepEqual(requireBrowserReceipt(receipt, SOURCE_SHA), {
    signInTicket: "ticket",
    testingToken: "testing",
  });
  assert.throws(
    () => requireBrowserReceipt(receipt, "b".repeat(40)),
    /does not match/u,
  );
});

test("validates one-time broker completion and redemption payloads", () => {
  const code = `ovd_${"c".repeat(43)}`;
  assert.equal(
    requireDesktopCompletion({
      callback_protocol: "orvilo",
      code,
    }),
    code,
  );
  assert.throws(
    () =>
      requireDesktopCompletion({
        callback_protocol: "http",
        code,
      }),
    /invalid desktop completion/u,
  );
  assert.throws(
    () =>
      requireDesktopCompletion({
        callback_protocol: "orvilo-staging-aaaaaaaaaaaaaaaa",
        code,
      }),
    /invalid desktop completion/u,
  );
  assert.equal(
    requireDesktopCompletion(
      {
        callback_protocol: "orvilo-staging-aaaaaaaaaaaaaaaa",
        code,
      },
      "orvilo-staging-aaaaaaaaaaaaaaaa",
    ),
    code,
  );
  assert.equal(requireRedeemedSession({ token: "jwt-value" }), "jwt-value");
  assert.throws(
    () => requireRedeemedSession({ token: "bad\nvalue" }),
    /invalid/u,
  );
});

test("login shell probe matches Pulse auth-shell classes, not zinc", async () => {
  const source = await import("node:fs/promises").then(({ readFile }) =>
    readFile(
      new URL("./verify-production-browser.mjs", import.meta.url),
      "utf8",
    ),
  );
  const shell = await import("node:fs/promises").then(({ readFile }) =>
    readFile(
      new URL("../apps/web/components/auth-shell.tsx", import.meta.url),
      "utf8",
    ),
  );
  assert.match(shell, /className="dark grid min-h-dvh w-full bg-background md:grid-cols-2"/u);
  assert.match(source, /authShell\)\.toHaveClass\(\/\\bbg-background\\b\/u\)/u);
  assert.match(source, /formPanel\)\.toHaveClass\(\/\\bbg-background\\b\/u\)/u);
  assert.match(source, /brandPanel\)\.toHaveClass\(\/\\bbg-card\\b\/u\)/u);
  assert.doesNotMatch(source, /bg-zinc-950/u);
});

test("browser acceptance uses the environment cookie domain and desktop scheme", async () => {
  const source = await import("node:fs/promises").then(({ readFile }) =>
    readFile(
      new URL("./verify-production-browser.mjs", import.meta.url),
      "utf8",
    ),
  );
  assert.match(source, /callback_protocol: DESKTOP_CALLBACK_PROTOCOL/u);
  assert.match(source, /domain: PRODUCT_COOKIE_DOMAIN/u);
  assert.doesNotMatch(source, /callback_protocol: "orvilo"/u);
  assert.doesNotMatch(source, /domain: "orvilo\.aspectlylabs\.com"/u);
});

test("refuses to open a browser without a real Clerk publishable key", () => {
  assert.equal(
    requireClerkPublishableKey("pk_live_Y2xlcmsuZXhhbXBsZS5jb20k"),
    "pk_live_Y2xlcmsuZXhhbXBsZS5jb20k",
  );
  for (const value of [undefined, "", "   ", "sk_live_secret", "pk_live_"]) {
    assert.throws(
      () => requireClerkPublishableKey(value),
      /CLERK_PUBLISHABLE_KEY/u,
      `expected ${JSON.stringify(value)} to be rejected`,
    );
  }
});
