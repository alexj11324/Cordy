import assert from "node:assert/strict";
import test from "node:test";

import { parseReleaseEndpoint } from "./release-dev-acceptance-port.mjs";

test("acceptance release endpoint is optional for normal development", () => {
  assert.equal(parseReleaseEndpoint({}), null);
});

test("acceptance release endpoint validates the loopback handoff contract", () => {
  const token = "01234567-89ab-cdef-0123-456789abcdef";
  assert.deepEqual(
    parseReleaseEndpoint({
      PATCHBAY_DEV_ACCEPTANCE_RELEASE_PORT: "43123",
      PATCHBAY_DEV_ACCEPTANCE_RELEASE_TOKEN: token,
    }),
    { port: 43123, token },
  );
  assert.throws(
    () => parseReleaseEndpoint({ PATCHBAY_DEV_ACCEPTANCE_RELEASE_PORT: "43123" }),
    /invalid dev acceptance release endpoint/,
  );
  assert.throws(
    () => parseReleaseEndpoint({
      PATCHBAY_DEV_ACCEPTANCE_RELEASE_PORT: "80",
      PATCHBAY_DEV_ACCEPTANCE_RELEASE_TOKEN: token,
    }),
    /invalid dev acceptance release endpoint/,
  );
});
