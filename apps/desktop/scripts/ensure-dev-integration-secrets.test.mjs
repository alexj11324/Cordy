import { describe, expect, it } from "vitest";

import { ensureLocalIntegrationSecrets } from "../../../scripts/ensure-dev-integration-secrets.mjs";

describe("local integration secret preparation", () => {
  it("fills only missing local encryption keys without replacing existing values", () => {
    const existing = Buffer.alloc(32, 3).toString("base64");
    const generated = [
      Buffer.alloc(32, 4).toString("base64"),
      Buffer.alloc(32, 5).toString("base64"),
    ];
    const result = ensureLocalIntegrationSecrets(
      `PORT=8080\nPATCHBAY_TELEGRAM_SECRET_KEY=${existing}\nPATCHBAY_WEIXIN_SECRET_KEY=\n`,
      () => generated.shift(),
    );

    expect(result.generated).toEqual(["PATCHBAY_WEIXIN_SECRET_KEY"]);
    expect(result.contents).toContain(
      `PATCHBAY_TELEGRAM_SECRET_KEY=${existing}`,
    );
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
