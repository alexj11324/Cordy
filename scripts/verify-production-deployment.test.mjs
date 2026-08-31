import assert from "node:assert/strict";
import test from "node:test";

import {
  requireHealthyResponse,
  verifyProductionOnce,
} from "./verify-production-deployment.mjs";

const SOURCE_SHA = "a".repeat(40);
const EXPECTED_BUILD = `sha-${SOURCE_SHA}`;

test("rejects a protected route server error", () => {
  const response = new Response("error", {
    status: 500,
    headers: { "x-patchbay-build": EXPECTED_BUILD },
  });
  assert.throws(
    () =>
      requireHealthyResponse(response, {
        url: "https://patchbay.aspectlylabs.com/acme/task-graph",
        expectedBuild: EXPECTED_BUILD,
      }),
    /unacceptable HTTP 500/u,
  );
});

test("rejects a missing task graph route", () => {
  const response = new Response("missing", {
    status: 404,
    headers: { "x-patchbay-build": EXPECTED_BUILD },
  });
  assert.throws(
    () =>
      requireHealthyResponse(response, {
        url: "https://patchbay.aspectlylabs.com/acme/task-graph",
        expectedBuild: EXPECTED_BUILD,
      }),
    /unacceptable HTTP 404/u,
  );
});

test("rejects a healthy route served by the wrong Web image", () => {
  const response = new Response("ok", {
    status: 200,
    headers: { "x-patchbay-build": "sha-old" },
  });
  assert.throws(
    () =>
      requireHealthyResponse(response, {
        url: "https://patchbay.aspectlylabs.com/login",
        expectedBuild: EXPECTED_BUILD,
      }),
    /reported build sha-old/u,
  );
});

test("verifies backend, Web, Docs, and Auth Broker from one source SHA", async () => {
  const seen = [];
  const fakeFetch = async (url) => {
    seen.push(url);
    if (url.endsWith("/api/config")) {
      return Response.json({ server_version: EXPECTED_BUILD });
    }
    return new Response("ok", {
      status: 200,
      headers: { "x-patchbay-build": EXPECTED_BUILD },
    });
  };

  await verifyProductionOnce(SOURCE_SHA, fakeFetch);
  assert.deepEqual(seen, [
    "https://api.aspectlylabs.com/api/config",
    "https://patchbay.aspectlylabs.com/login",
    "https://patchbay.aspectlylabs.com/acme/issues",
    "https://patchbay.aspectlylabs.com/acme/task-graph",
    "https://patchbay.aspectlylabs.com/docs",
    "https://accounts.aspectlylabs.com/readyz",
    "https://accounts.aspectlylabs.com/oauth/google",
  ]);
});
