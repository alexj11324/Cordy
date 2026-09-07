import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { verifyStagingOnce } from "./verify-staging-deployment.mjs";

test("staging verifier probes only staging hosts", async () => {
  const urls = [];
  const fetchImpl = async (url) => {
    urls.push(String(url));
    return {
      status: 200,
      headers: new Headers({
        "x-patchbay-build": `sha-${"a".repeat(40)}`,
        "x-patchbay-commit": "a".repeat(40),
      }),
    };
  };
  await verifyStagingOnce("a".repeat(40), fetchImpl);
  assert.deepEqual(urls, [
    "https://api.staging.aspectlylabs.com/api/config",
    "https://staging.aspectlylabs.com/login",
    "https://staging.aspectlylabs.com/docs",
    "https://accounts.staging.aspectlylabs.com/readyz",
  ]);
  assert.equal(
    urls.some((url) => url.includes("patchbay.aspectlylabs.com")),
    false,
  );
  assert.equal(urls.some((url) => url.includes("api.aspectlylabs.com/")), false);
});
