import assert from "node:assert/strict";
import test from "node:test";

import {
  requireHealthyResponse,
  verifyProductionOnce,
} from "./verify-production-deployment.mjs";

const SOURCE_SHA = "a".repeat(40);
const EXPECTED_BUILD = `sha-${SOURCE_SHA}`;

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

test("public pages must render instead of redirecting", () => {
  const response = new Response(null, {
    status: 307,
    headers: { "x-patchbay-build": EXPECTED_BUILD },
  });
  assert.throws(
    () =>
      requireHealthyResponse(response, {
        url: "https://patchbay.aspectlylabs.com/login",
        expectedBuild: EXPECTED_BUILD,
        exactStatus: 200,
      }),
    /expected 200/u,
  );
});

test("verifies backend, Web, Docs, and Auth Broker from one source SHA", async () => {
  const seen = [];
  const fakeFetch = async (url) => {
    seen.push(url);
    if (url.endsWith("/api/config")) {
      return Response.json(
        {},
        {
          headers: {
            "x-patchbay-build": EXPECTED_BUILD,
            "x-patchbay-commit": SOURCE_SHA,
          },
        },
      );
    }
    return new Response("ok", {
      status: 200,
      headers: {
        "x-patchbay-build": EXPECTED_BUILD,
        "x-patchbay-commit": SOURCE_SHA,
      },
    });
  };

  await verifyProductionOnce(SOURCE_SHA, fakeFetch);
  assert.deepEqual(seen, [
    "https://api.aspectlylabs.com/api/config",
    "https://patchbay.aspectlylabs.com/login",
    "https://patchbay.aspectlylabs.com/docs",
    "https://accounts.aspectlylabs.com/readyz",
  ]);
});
