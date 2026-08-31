import assert from "node:assert/strict";
import test from "node:test";

import worker from "./index.js";

const TOKEN = "a".repeat(64);

test("health remains local to the Worker", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = () => {
    throw new Error("health must not call the origin");
  };

  try {
    const response = await worker.fetch(
      new Request("https://accounts.aspectlylabs.com/health"),
      {},
    );
    assert.equal(response.status, 200);
    assert.deepEqual(await response.json(), { status: "ok" });
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("fails closed when the origin secret is missing", async () => {
  const response = await worker.fetch(
    new Request("https://accounts.aspectlylabs.com/login"),
    { ORIGIN: "https://accounts-origin.aspectlylabs.com" },
  );
  assert.equal(response.status, 500);
});

test("fails closed when the broker origin is missing", async () => {
  const response = await worker.fetch(
    new Request("https://accounts.aspectlylabs.com/login"),
    { ORIGIN_AUTH_TOKEN: TOKEN },
  );
  assert.equal(response.status, 500);
});

test("routes to the configured origin and overwrites spoofable proxy headers", async () => {
  const originalFetch = globalThis.fetch;
  let forwarded;
  globalThis.fetch = async (request) => {
    forwarded = request;
    return new Response("ok", { status: 200 });
  };

  try {
    const response = await worker.fetch(
      new Request(
        "https://accounts.aspectlylabs.com/oauth/google?platform=desktop&state=s1",
        {
          headers: {
            "x-forwarded-host": "attacker.invalid",
            "x-patchbay-origin-auth": "attacker-controlled",
          },
        },
      ),
      {
        ORIGIN: "https://accounts-origin.aspectlylabs.com",
        ORIGIN_AUTH_TOKEN: TOKEN,
      },
    );

    assert.equal(response.status, 200);
    assert.equal(
      forwarded.url,
      "https://accounts-origin.aspectlylabs.com/oauth/google?platform=desktop&state=s1",
    );
    assert.equal(
      forwarded.headers.get("x-forwarded-host"),
      "accounts.aspectlylabs.com",
    );
    assert.equal(forwarded.headers.get("x-patchbay-origin-auth"), TOKEN);
  } finally {
    globalThis.fetch = originalFetch;
  }
});
