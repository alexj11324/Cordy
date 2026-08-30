import { afterEach, describe, expect, it } from "vitest";
import { installWebDesktopBridge } from "./web-bridge";

const bridgeKeys = [
  "desktopAPI",
  "daemonAPI",
  "updater",
  "electron",
  "__PATCHBAY_VITE_DESKTOP_PREVIEW__",
] as const;

function clearWebBridge(): void {
  for (const key of bridgeKeys) {
    Reflect.deleteProperty(window, key);
  }
}

afterEach(clearWebBridge);

describe("Vite browser Desktop bridge", () => {
  it("lets shared views localize unsupported directory controls", async () => {
    clearWebBridge();

    expect(installWebDesktopBridge()).toBe(true);

    await expect(window.desktopAPI.pickDirectory()).resolves.toEqual({
      ok: false,
      reason: "unsupported",
    });
    await expect(
      window.desktopAPI.validateLocalDirectory("/preview-only"),
    ).resolves.toEqual({
      ok: false,
      reason: "unsupported",
    });
  });
});
