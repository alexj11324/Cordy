import assert from "node:assert/strict";
import test from "node:test";
import worker from "./index.js";
const token = "a".repeat(64);
test("rate limits auth routes and sanitizes origin headers", async () => {
  let forwarded;
  globalThis.fetch = async (request) => { forwarded = request; return new Response("ok"); };
  const limiter = { limit: async () => ({ success: true }) };
  const request = new Request("https://accounts.aspectlylabs.com/v1/desktop/google/attempt", { method: "POST", headers: { "cf-connecting-ip": "192.0.2.1", "x-forwarded-host": "evil.example", "x-patchbay-origin-auth": "attacker" }, body: "{}" });
  const result = await worker.fetch(request, { ORIGIN: "https://accounts-origin.aspectlylabs.com", ORIGIN_AUTH_TOKEN: token, DESKTOP_ATTEMPT_RATE_LIMITER: limiter });
  assert.equal(result.status, 200); assert.equal(forwarded.headers.get("x-patchbay-origin-auth"), token); assert.equal(forwarded.headers.get("x-forwarded-host"), "accounts.aspectlylabs.com"); assert.equal(forwarded.headers.get("cf-connecting-ip"), null);
});
test("fails closed when the limiter is unavailable", async () => { const result = await worker.fetch(new Request("https://accounts.aspectlylabs.com/v1/desktop/google/complete", { method: "POST", headers: { "cf-connecting-ip": "192.0.2.1" } }), { ORIGIN: "https://accounts-origin.aspectlylabs.com", ORIGIN_AUTH_TOKEN: token }); assert.equal(result.status, 503); });
