import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import worker from "./index.js";

const TOKEN = "a".repeat(64);

test("the source-controlled accounts origin enforces both network and token gates", () => {
  const nginx = readFileSync(
    new URL("../../../origin/nginx/aspectlylabs-origin.conf", import.meta.url),
    "utf8",
  );

  assert.match(
    nginx,
    /server_name accounts-origin\.aspectlylabs\.com;[\s\S]*?include \/etc\/nginx\/snippets\/cloudflare-only\.conf;[\s\S]*?include \/etc\/nginx\/snippets\/patchbay-accounts-origin-auth\.conf;/,
  );
  assert.match(
    nginx,
    /server_name accounts-origin\.aspectlylabs\.com;[\s\S]*?proxy_pass http:\/\/127\.0\.0\.1:43100;/,
  );
  assert.match(nginx, /proxy_set_header X-Patchbay-Origin-Auth "";/);
});

function allowLimiter(calls = []) {
  return {
    async limit(input) {
      calls.push(input);
      return { success: true };
    },
  };
}

function configuredEnv(overrides = {}) {
  return {
    ORIGIN: "https://accounts-origin.aspectlylabs.com",
    ORIGIN_AUTH_TOKEN: TOKEN,
    DESKTOP_ATTEMPT_RATE_LIMITER: allowLimiter(),
    DESKTOP_COMPLETE_RATE_LIMITER: allowLimiter(),
    ...overrides,
  };
}

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
            "x-forwarded-for": "198.51.100.2",
            forwarded: "for=198.51.100.3",
            "x-real-ip": "198.51.100.4",
            "true-client-ip": "198.51.100.5",
            "cf-connecting-ip": "198.51.100.6",
            "x-patchbay-origin-auth": "attacker-controlled",
            "x-patchbay-desktop-broker-auth": "attacker-controlled",
          },
        },
      ),
      configuredEnv(),
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
    assert.equal(forwarded.headers.get("x-patchbay-desktop-broker-auth"), null);
    assert.equal(forwarded.headers.get("forwarded"), null);
    assert.equal(forwarded.headers.get("x-forwarded-for"), null);
    assert.equal(forwarded.headers.get("x-real-ip"), null);
    assert.equal(forwarded.headers.get("true-client-ip"), null);
    assert.equal(forwarded.headers.get("cf-connecting-ip"), null);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("rate limits attempt and complete independently without forwarding the client IP", async () => {
  const originalFetch = globalThis.fetch;
  const forwarded = [];
  const attemptCalls = [];
  const completeCalls = [];
  globalThis.fetch = async (request) => {
    forwarded.push(request);
    return new Response("ok", { status: 200 });
  };

  try {
    const env = configuredEnv({
      DESKTOP_ATTEMPT_RATE_LIMITER: allowLimiter(attemptCalls),
      DESKTOP_COMPLETE_RATE_LIMITER: allowLimiter(completeCalls),
    });
    for (const [path, ip, spoofedForwardedFor] of [
      ["/v1/desktop/google/attempt", "203.0.113.7", "198.51.100.1"],
      ["/v1/desktop/google/attempt", "203.0.113.7", "198.51.100.2"],
      ["/v1/desktop/google/attempt", "203.0.113.8", "198.51.100.1"],
      ["/v1/desktop/google/complete", "203.0.113.7", "198.51.100.1"],
    ]) {
      const response = await worker.fetch(
        new Request(`https://accounts.aspectlylabs.com${path}`, {
          method: "POST",
          headers: {
            "cf-connecting-ip": ip,
            "x-forwarded-for": spoofedForwardedFor,
          },
        }),
        env,
      );
      assert.equal(response.status, 200);
    }

    assert.deepEqual(attemptCalls, [
      { key: "attempt:v1:203.0.113.7" },
      { key: "attempt:v1:203.0.113.7" },
      { key: "attempt:v1:203.0.113.8" },
    ]);
    assert.deepEqual(completeCalls, [{ key: "complete:v1:203.0.113.7" }]);
    assert.deepEqual(
      forwarded.map((request) => request.headers.get("cf-connecting-ip")),
      [null, null, null, null],
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("desktop Google edge fails closed before origin fetch", async () => {
  const originalFetch = globalThis.fetch;
  let fetches = 0;
  globalThis.fetch = async () => {
    fetches += 1;
    return new Response("unexpected");
  };

  try {
    const cases = [
      configuredEnv(),
      configuredEnv({ DESKTOP_ATTEMPT_RATE_LIMITER: undefined }),
      configuredEnv(),
      configuredEnv({
        DESKTOP_ATTEMPT_RATE_LIMITER: {
          async limit() {
            throw new Error("binding unavailable");
          },
        },
      }),
      configuredEnv({
        DESKTOP_ATTEMPT_RATE_LIMITER: {
          async limit() {
            return {};
          },
        },
      }),
    ];
    const headers = [
      {},
      { "cf-connecting-ip": "203.0.113.7" },
      { "cf-connecting-ip": "not-an-ip" },
      { "cf-connecting-ip": "203.0.113.7" },
      { "cf-connecting-ip": "203.0.113.7" },
    ];
    for (let index = 0; index < cases.length; index += 1) {
      const response = await worker.fetch(
        new Request("https://accounts.aspectlylabs.com/v1/desktop/google/attempt", {
          method: "POST",
          headers: headers[index],
        }),
        cases[index],
      );
      assert.equal(response.status, 503);
      assert.equal(response.headers.get("cache-control"), "no-store");
      assert.equal(response.headers.get("content-type"), "application/json");
      assert.equal(response.headers.get("x-patchbay-auth-contract-version"), "1");
    }

    const limited = await worker.fetch(
      new Request("https://accounts.aspectlylabs.com/v1/desktop/google/complete", {
        method: "POST",
        headers: { "cf-connecting-ip": "203.0.113.7" },
      }),
      configuredEnv({
        DESKTOP_COMPLETE_RATE_LIMITER: {
          async limit() {
            return { success: false };
          },
        },
      }),
    );
    assert.equal(limited.status, 429);
    assert.equal(limited.headers.get("retry-after"), "60");
    assert.equal(limited.headers.get("cache-control"), "no-store");
    assert.deepEqual(await limited.json(), { error: "rate_limited" });
    assert.equal(fetches, 0);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("a denied attempt never consumes the complete budget and vice versa", async () => {
  const originalFetch = globalThis.fetch;
  let fetches = 0;
  globalThis.fetch = async () => {
    fetches += 1;
    return new Response("ok");
  };

  try {
    const clientHeaders = { "cf-connecting-ip": "203.0.113.9" };
    const attemptDenied = configuredEnv({
      DESKTOP_ATTEMPT_RATE_LIMITER: {
        async limit() {
          return { success: false };
        },
      },
      DESKTOP_COMPLETE_RATE_LIMITER: allowLimiter(),
    });
    const deniedAttempt = await worker.fetch(
      new Request("https://accounts.aspectlylabs.com/v1/desktop/google/attempt", {
        method: "POST",
        headers: clientHeaders,
      }),
      attemptDenied,
    );
    const allowedComplete = await worker.fetch(
      new Request("https://accounts.aspectlylabs.com/v1/desktop/google/complete", {
        method: "POST",
        headers: clientHeaders,
      }),
      attemptDenied,
    );

    const completeDenied = configuredEnv({
      DESKTOP_ATTEMPT_RATE_LIMITER: allowLimiter(),
      DESKTOP_COMPLETE_RATE_LIMITER: {
        async limit() {
          return { success: false };
        },
      },
    });
    const allowedAttempt = await worker.fetch(
      new Request("https://accounts.aspectlylabs.com/v1/desktop/google/attempt", {
        method: "POST",
        headers: clientHeaders,
      }),
      completeDenied,
    );
    const deniedComplete = await worker.fetch(
      new Request("https://accounts.aspectlylabs.com/v1/desktop/google/complete", {
        method: "POST",
        headers: clientHeaders,
      }),
      completeDenied,
    );

    assert.equal(deniedAttempt.status, 429);
    assert.equal(allowedComplete.status, 200);
    assert.equal(allowedAttempt.status, 200);
    assert.equal(deniedComplete.status, 429);
    assert.equal(fetches, 2);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("rejects non-canonical desktop Google paths and methods before origin fetch", async () => {
  const originalFetch = globalThis.fetch;
  let fetches = 0;
  globalThis.fetch = async () => {
    fetches += 1;
    return new Response("unexpected");
  };

  try {
    for (const path of [
      "/v1/desktop/google/attempt/",
      "/v1//desktop/google/attempt",
      "/v1%2Fdesktop/google/attempt",
      "/V1/Desktop/Google/Attempt",
      "/v1/desktop/google/complete/",
    ]) {
      const response = await worker.fetch(
        new Request(`https://accounts.aspectlylabs.com${path}`, { method: "POST" }),
        configuredEnv(),
      );
      assert.equal(response.status, 404, path);
    }

    const wrongMethod = await worker.fetch(
      new Request("https://accounts.aspectlylabs.com/v1/desktop/google/attempt"),
      configuredEnv(),
    );
    assert.equal(wrongMethod.status, 405);
    assert.equal(wrongMethod.headers.get("allow"), "POST");
    assert.equal(fetches, 0);
  } finally {
    globalThis.fetch = originalFetch;
  }
});
