import { describe, expect, it } from "vitest";

import {
  INTEGRATION_SECRET_KEYS,
  ensureLocalIntegrationSecrets,
} from "../../../scripts/ensure-dev-integration-secrets.mjs";

describe("local integration secret preparation", () => {
  it("fills only missing local encryption keys without replacing existing values", () => {
    const existing = Buffer.alloc(32, 3).toString("base64");
    const missing = INTEGRATION_SECRET_KEYS.at(-1);
    const contents = INTEGRATION_SECRET_KEYS.map((key) =>
      key === missing ? `${key}=` : `${key}=${existing}`,
    ).join("\n");
    const result = ensureLocalIntegrationSecrets(
      `PORT=8080\n${contents}\n`,
      () => Buffer.alloc(32, 4).toString("base64"),
    );

    expect(result.generated).toEqual([missing]);
    for (const key of INTEGRATION_SECRET_KEYS.slice(0, -1)) {
      expect(result.contents).toContain(`${key}=${existing}`);
    }
    expect(result.contents).toContain(
      `PATCHBAY_WEIXIN_SECRET_KEY=${Buffer.alloc(32, 4).toString("base64")}`,
    );
  });

  it("refuses to overwrite an invalid non-empty existing secret", () => {
    expect(() =>
      ensureLocalIntegrationSecrets(
        "PATCHBAY_TELEGRAM_SECRET_KEY=do-not-overwrite\nPATCHBAY_WEIXIN_SECRET_KEY=\n",
      ),
    ).toThrow(/invalid non-empty value.*fix or clear it explicitly/i);
  });
});
